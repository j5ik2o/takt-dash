//! taktDash のバックエンドを起動する。
//!
//! 設定を読んで gRPC サーバーと定期読み取りを一緒に起動し、SIGTERM/SIGINT の終了シグナルで
//! 両方を停止します。gRPC サーバーは設定の listen か `--listen` の待ち受けアドレスで待ち受けます。

use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use takt_dash_backend::{
  activity::ActivityStore,
  config::load_config,
  conversation::{ConversationPaths, ConversationReader},
  grpc::{serve_with_listener, TaktDashServiceImpl},
  poller::{run_until_stopped, Poller},
  store::StateStore,
};

/// bin の引数。
struct Arguments {
  config_path: PathBuf,
  sqlite_path: Option<PathBuf>,
  listen:      Option<SocketAddr>,
}

fn parse_arguments() -> Result<Arguments, String> {
  let mut config_path = PathBuf::from("taktdash.config.json");
  let mut sqlite_path = None;
  let mut listen = None;
  let mut args = std::env::args().skip(1);
  while let Some(argument) = args.next() {
    match argument.as_str() {
      | "--config" => {
        let value = args.next().ok_or_else(|| "--config にはパスが必要です。".to_owned())?;
        config_path = PathBuf::from(value);
      },
      | "--sqlite" => {
        let value = args.next().ok_or_else(|| "--sqlite にはパスが必要です。".to_owned())?;
        sqlite_path = Some(PathBuf::from(value));
      },
      | "--listen" => {
        let value = args.next().ok_or_else(|| "--listen には待ち受けアドレスが必要です。".to_owned())?;
        listen = Some(
          value.parse::<SocketAddr>().map_err(|error| format!("--listen {value} を解釈できませんでした: {error}"))?,
        );
      },
      | other => return Err(format!("未知の引数です: {other}")),
    }
  }
  Ok(Arguments { config_path, sqlite_path, listen })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
  tracing_subscriber::fmt().with_writer(std::io::stderr).init();
  let arguments = parse_arguments().map_err(|error| format!("引数を解釈できませんでした: {error}"))?;
  let config = load_config(&arguments.config_path)?;
  let sqlite_path = arguments.sqlite_path.unwrap_or_else(|| config.sqlite_path.clone());
  if let Some(parent) = sqlite_path.parent() {
    std::fs::create_dir_all(parent)?;
  }
  let listen = arguments.listen.unwrap_or(config.listen);
  // 待ち受けアドレスを先に確保する。確保に失敗したら、
  // 定期読み取りを始める前に即座にエラー終了する。
  let listener = tokio::net::TcpListener::bind(listen).await?;
  let activity = Arc::new(ActivityStore::open(&sqlite_path)?);
  let store = Arc::new(StateStore::new(config.server_meta.clone(), config.projects.clone(), config.thresholds));
  let (changes, _) = tokio::sync::broadcast::channel(16);
  let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
  let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
  let conversation = Arc::new(ConversationReader::new(ConversationPaths::from_home(home)));
  let service = TaktDashServiceImpl::new(
    store.clone(),
    activity.clone(),
    arguments.config_path.clone(),
    conversation,
    changes.clone(),
    stop_rx.clone(),
  );
  let mut poller = Poller::new(arguments.config_path.clone(), activity, store, changes);
  let signal_sender = stop_tx.clone();
  tokio::spawn(async move {
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
      .expect("SIGTERM の受信を用意できませんでした");
    tokio::select! {
      _ = tokio::signal::ctrl_c() => {}
      _ = term.recv() => {}
    }
    let _ = signal_sender.send(true);
  });

  tracing::info!(%listen, "gRPC サーバーを起動します。");
  let server = serve_with_listener(listener, service, stop_rx.clone());
  let poller_stop = stop_rx.clone();
  let poll = async move {
    run_until_stopped(&mut poller, poller_stop).await;
  };
  let (server_result, ()) = tokio::join!(server, poll);
  server_result?;
  Ok(())
}

//! taktDash のバックエンドを起動する。設定を読んで定期読み取りを開始し、終了シグナルで停止する。

use std::{path::PathBuf, sync::Arc};

use takt_dash_backend::{
  activity::ActivityStore,
  config::load_config,
  poller::{run_until_stopped, Poller},
  store::StateStore,
};

/// bin の引数。
struct Arguments {
  config_path: PathBuf,
  sqlite_path: Option<PathBuf>,
}

fn parse_arguments() -> Result<Arguments, String> {
  let mut config_path = PathBuf::from("taktdash.config.json");
  let mut sqlite_path = None;
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
      | other => return Err(format!("未知の引数です: {other}")),
    }
  }
  Ok(Arguments { config_path, sqlite_path })
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
  let activity = Arc::new(ActivityStore::open(&sqlite_path)?);
  let store = Arc::new(StateStore::new(config.server_meta.clone(), config.projects.clone(), config.thresholds));
  let mut poller = Poller::new(arguments.config_path.clone(), activity, store);

  let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
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

  run_until_stopped(&mut poller, || *stop_rx.borrow()).await;
  Ok(())
}

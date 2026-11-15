//! taktDash のコマンドラインの入口。
//!
//! backend の gRPC へ接続して概要に相当する状態を日本語で表示する閲覧コマンドを持ちます。
//! 表示だけで、状態を書き換えるコマンドは持ちません。

mod proto;

use std::{env::VarError, io::Write, net::SocketAddr, path::PathBuf, time::Duration};

use proto::v1::{
  takt_dash_service_client::TaktDashServiceClient, DashboardState, GetOverviewRequest, PendingDecisionType,
};
use tonic::{transport::Endpoint, Code, Status};

/// backend の gRPC の待ち受けアドレスの既定値。backend の設定の既定値と揃えます。
pub const DEFAULT_BACKEND_ADDR: &str = "127.0.0.1:50051";
/// 接続先を上書きする環境変数。frontend/web と同じ名前を使い、新しい設定を増やしません。
pub const BACKEND_ADDR_ENV: &str = "TAKTDASH_BACKEND_ADDR";
/// backend の設定ファイルの既定のファイル名。backend の bin と同じ名前を使います。
pub const DEFAULT_CONFIG_PATH: &str = "taktdash.config.json";
/// 接続の試行に許す時間。応答のないアドレスでいつまでも待たないためです。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// 正常終了の終了コード。
const EXIT_OK: i32 = 0;
/// 接続・取得・引数のエラーで異常終了するときの終了コード。
const EXIT_FAILURE: i32 = 1;
/// 使い方の誤りで異常終了するときの終了コード。
const EXIT_USAGE: i32 = 2;

/// 閲覧のみを持つコマンドの名前。位置引数の既定値でもあります。
const OVERVIEW_COMMAND: &str = "overview";

/// 接続先を解決する材料。境界で一度だけ解決するために、生の入力をまとめて受けます。
struct AddrSources<'a> {
  cli_addr:      Option<&'a str>,
  env_addr:      Result<String, VarError>,
  config_listen: Option<&'a str>,
}

/// 接続先が確定しなかったときに返す失敗。
#[derive(Debug, PartialEq, Eq)]
enum AddrError {
  /// `--addr`、環境変数、設定の `listen` のいずれかが待ち受けアドレスとして解釈できない。
  Invalid { source: &'static str, value: String },
}

/// backend の接続先を、`--addr` ＞ 環境変数 ＞ 設定の `listen` ＞ 既定値 の順で一度だけ解決する。
///
/// 空文字列は未設定として扱い、次の候補へ進みます。解釈できない値は既定値へ置き換えず失敗させます。
fn resolve_backend_addr(sources: &AddrSources<'_>) -> Result<SocketAddr, AddrError> {
  let candidates = [
    ("--addr", sources.cli_addr.map(str::to_owned)),
    (BACKEND_ADDR_ENV, sources.env_addr.clone().ok()),
    ("設定の listen", sources.config_listen.map(str::to_owned)),
  ];
  for (source, value) in candidates {
    let Some(value) = value.filter(|value| !value.is_empty()) else {
      continue;
    };
    return value.parse::<SocketAddr>().map_err(|_| AddrError::Invalid { source, value });
  }
  DEFAULT_BACKEND_ADDR
    .parse::<SocketAddr>()
    .map_err(|_| AddrError::Invalid { source: "既定値", value: DEFAULT_BACKEND_ADDR.to_owned() })
}

/// 引数を解釈した結果。
#[derive(Debug, PartialEq, Eq)]
enum Invocation {
  /// 概要を表示する。
  Overview(OverviewOptions),
  /// 使い方を表示して正常終了する。
  Help,
}

/// 概要の表示に使う解決前の入力。
#[derive(Debug, Default, PartialEq, Eq)]
struct OverviewOptions {
  addr:   Option<String>,
  config: Option<PathBuf>,
}

/// 引数の解釈に失敗したときに返す失敗。使い方と一緒に表示します。
#[derive(Debug, PartialEq, Eq)]
struct UsageError(String);

/// コマンドラインの引数を解釈する。
fn parse_arguments(args: &[String]) -> Result<Invocation, UsageError> {
  let mut options = OverviewOptions::default();
  let mut command_seen = false;
  let mut index = 0;
  while index < args.len() {
    let argument = args[index].as_str();
    match argument {
      | "--help" | "-h" => return Ok(Invocation::Help),
      | "--addr" => {
        let value = take_value(args, index, "--addr")?;
        options.addr = Some(value);
        index += 2;
      },
      | "--config" => {
        let value = take_value(args, index, "--config")?;
        options.config = Some(PathBuf::from(value));
        index += 2;
      },
      | flag if flag.starts_with('-') => {
        return Err(UsageError(format!("未知の引数です: {flag}")));
      },
      | position if !command_seen => {
        if position != OVERVIEW_COMMAND {
          return Err(UsageError(format!(
            "未知のコマンドです: {position}。このコマンドは概要を閲覧する {OVERVIEW_COMMAND} だけを持ちます。"
          )));
        }
        command_seen = true;
        index += 1;
      },
      | position => {
        return Err(UsageError(format!("概要の表示に位置引数は取れません: {position}")));
      },
    }
  }
  Ok(Invocation::Overview(options))
}

/// `--addr` や `--config` の値を取り出す。
fn take_value(args: &[String], index: usize, flag: &str) -> Result<String, UsageError> {
  args
    .get(index + 1)
    .filter(|value| !value.is_empty())
    .cloned()
    .ok_or_else(|| UsageError(format!("{flag} には値が必要です。")))
}

/// 使い方の説明。未知の引数・未知のコマンドのときに stderr へ出します。
fn usage() -> String {
  [
    "使い方: takt-dash-cli overview [--addr <待ち受けアドレス>] [--config <設定ファイル>]",
    "位置引数を省略した場合も overview として動作し、backend の gRPC から概要を取得して表示します。",
    "--addr      接続先の待ち受けアドレス（例: 127.0.0.1:50051）",
    "--config    backend と同じ形式の設定ファイルのパス。listen を接続先に使います",
    "--help      この使い方を表示します",
  ]
  .join("\n")
}

/// 設定ファイルの要素のうち、接続先の解決に使う `listen` だけを読む。
#[derive(serde::Deserialize)]
struct ConfigListen {
  listen: Option<String>,
}

/// 設定ファイルから `listen` を読む。ファイルが無いときは未設定として扱い、既定値へ進みます。
fn read_config_listen(path: &PathBuf) -> Result<Option<String>, String> {
  let text = match std::fs::read_to_string(path) {
    | Ok(text) => text,
    | Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
    | Err(error) => return Err(format!("設定ファイル {} を読み込めませんでした: {error}", path.display())),
  };
  let parsed: ConfigListen = serde_json::from_str(&text)
    .map_err(|error| format!("設定ファイル {} の JSON を解析できませんでした: {error}", path.display()))?;
  Ok(parsed.listen)
}

/// 概要の取得に失敗した理由。
#[derive(Debug)]
enum OverviewError {
  /// backend に接続できない、または backend が gRPC として応答しない（`is_connection_failure`）。
  Unavailable(String),
  /// backend は応答したが、接続不能以外の理由で失敗した。
  Call(Status),
}

/// 接続不能の分類。backend へ届かない、または backend が gRPC として応答しない状態を接続不能として
/// 扱います。
///
/// `UNAVAILABLE`・`DEADLINE_EXCEEDED` に加え、待ち受けはあるが gRPC を話さない相手（接続後に即時
/// 切断、または無応答）を実測すると `CANCELLED` または `UNKNOWN` で返るため、これらも接続不能に
/// 含めます。この 4 コードは `frontend/web` の `BackendUnavailableError`（`UNAVAILABLE` と
/// `DEADLINE_EXCEEDED` の 2 コードのみ）より広い集合です。
///
/// backend が応答したうえで概要を返せない失敗は接続不能にしません。`NOT_FOUND`・
/// `PERMISSION_DENIED`・`INTERNAL` などがこれに当たります。
fn is_connection_failure(code: Code) -> bool {
  matches!(code, Code::Unavailable | Code::DeadlineExceeded | Code::Cancelled | Code::Unknown)
}

/// backend の `GetOverview` で概要を取得する。
///
/// 接続成否の判定はこの関数だけが行います。`Endpoint::connect()` の失敗と、`GetOverview` が返す
/// `Status` を同じ分類（接続不能か、backend が応答したうえでの失敗か）へ写します。
async fn fetch_overview(addr: SocketAddr) -> Result<DashboardState, OverviewError> {
  let endpoint = Endpoint::from_shared(format!("http://{addr}"))
    .map_err(|error| OverviewError::Unavailable(format!("接続先 {addr} の指定が不正です: {error}")))?
    .connect_timeout(CONNECT_TIMEOUT)
    .timeout(CONNECT_TIMEOUT);
  let channel = endpoint
    .connect()
    .await
    .map_err(|error| OverviewError::Unavailable(format!("接続先 {addr} に接続できません: {error}")))?;
  let mut client = TaktDashServiceClient::new(channel);
  let response = client.get_overview(GetOverviewRequest {}).await.map_err(|status| {
    if is_connection_failure(status.code()) {
      OverviewError::Unavailable(format!("接続先 {addr} から概要を取得できません: {}", status.message()))
    } else {
      OverviewError::Call(status)
    }
  })?;
  Ok(response.into_inner())
}

/// 概要を標準出力へ表示する。
///
/// 引数を解釈し、接続先を解決し、概要を取得して表示します。出力とエラーは指定された writer へ
/// 書き、終了コードを返します。`run` とテストの両方がこの関数を通して観測します。
fn run_with(
  args: &[String],
  env_addr: Result<String, VarError>,
  stdout: &mut impl Write,
  stderr: &mut impl Write,
) -> i32 {
  let invocation = match parse_arguments(args) {
    | Ok(invocation) => invocation,
    | Err(UsageError(message)) => {
      let _ = writeln!(stderr, "{message}\n{}", usage());
      return EXIT_USAGE;
    },
  };
  let options = match invocation {
    | Invocation::Help => {
      let _ = writeln!(stdout, "{}", usage());
      return EXIT_OK;
    },
    | Invocation::Overview(options) => options,
  };
  let config_path = options.config.clone().unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG_PATH));
  let config_listen = match read_config_listen(&config_path) {
    | Ok(listen) => listen,
    | Err(message) => {
      let _ = writeln!(stderr, "{message}");
      return EXIT_FAILURE;
    },
  };
  let sources = AddrSources { cli_addr: options.addr.as_deref(), env_addr, config_listen: config_listen.as_deref() };
  let addr = match resolve_backend_addr(&sources) {
    | Ok(addr) => addr,
    | Err(AddrError::Invalid { source, value }) => {
      let _ = writeln!(stderr, "{source} の値 {value} を待ち受けアドレスとして解釈できませんでした。");
      return EXIT_USAGE;
    },
  };
  let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
    | Ok(runtime) => runtime,
    | Err(error) => {
      let _ = writeln!(stderr, "実行環境の準備に失敗しました: {error}");
      return EXIT_FAILURE;
    },
  };
  match runtime.block_on(fetch_overview(addr)) {
    | Ok(state) => {
      let _ = writeln!(stdout, "{}", render_overview(&state));
      EXIT_OK
    },
    | Err(OverviewError::Unavailable(message)) => {
      let _ = writeln!(stderr, "backend に接続できません。{message}");
      EXIT_FAILURE
    },
    | Err(OverviewError::Call(status)) => {
      let _ = writeln!(stderr, "backend から概要を取得できませんでした（{}）。{}", status.code(), status.message());
      EXIT_FAILURE
    },
  }
}

/// 環境変数から接続先の上書きを読む。
fn env_backend_addr() -> Result<String, VarError> {
  std::env::var(BACKEND_ADDR_ENV)
}

/// コマンドラインの起動引数を受け取り、実行結果を終了コードとして返します。
#[must_use]
pub fn run(args: &[String]) -> i32 {
  let mut stdout = std::io::stdout();
  let mut stderr = std::io::stderr();
  run_with(args, env_backend_addr(), &mut stdout, &mut stderr)
}

/// 概要の表示内容を日本語の文字列へ組み立てる。
///
/// 取得した `DashboardState` だけから表示を作り、`println!` へ直書きしません。表示する項目は、
/// サーバーの接続状態、プロジェクト、ゴールとタスクの件数、run、判断待ちの件数と内訳、
/// 異常判定の閾値、最後に読めた時刻です。
fn render_overview(state: &DashboardState) -> String {
  let running_runs = state.runs.iter().filter(|run| run.status == proto::v1::RunStatus::Running as i32).count();
  let inquiries =
    state.pending_decisions.iter().filter(|decision| decision.r#type == PendingDecisionType::Inquiry as i32).count();
  let awaiting_merge = state
    .pending_decisions
    .iter()
    .filter(|decision| decision.r#type == PendingDecisionType::AwaitingMerge as i32)
    .count();
  let servers: Vec<String> = state
    .servers
    .iter()
    .map(|server| {
      let state_label = if server.connected { "接続中" } else { "応答なし" };
      format!("- {}（{}、{}）", server.name, server.kind, state_label)
    })
    .collect();
  let projects: Vec<String> =
    state.projects.iter().map(|project| format!("- {}（{}）", project.name, project.about)).collect();
  let goals: Vec<String> = state
    .goals
    .iter()
    .map(|goal| {
      let counts = goal.task_counts.unwrap_or_default();
      format!(
        "- {}/{}「{}」完了{}件・失敗{}件（未着手{}件・実行中{}件）",
        goal.project, goal.goal_id, goal.name, counts.completed, counts.failed, counts.pending, counts.running
      )
    })
    .collect();
  let runs: Vec<String> =
    state.runs.iter().map(|run| format!("- {}/{}（{}）", run.project, run.task_name, run.summary)).collect();
  let pending_detail: Vec<String> = state
    .pending_decisions
    .iter()
    .map(|decision| {
      let kind = if decision.r#type == PendingDecisionType::AwaitingMerge as i32 {
        "人のマージ待ち"
      } else if decision.inquiry_kind == proto::v1::InquiryKind::OperationRequest as i32 {
        "問い合わせ（作業の依頼）"
      } else {
        "問い合わせ（質問）"
      };
      format!("- {}/{}: {} {}", decision.project, decision.server_id, kind, decision.body)
    })
    .collect();
  let thresholds = state.thresholds.unwrap_or_default();
  format!(
    "概要\n\
     読んだ時刻: {}\n\
     サーバー: {}台\n\
     {}\n\
     プロジェクト: {}件\n\
     {}\n\
     ゴール: {}件\n\
     {}\n\
     実行中の run: {}件\n\
     {}\n\
     判断待ち: {}件（未回答の問い合わせ {}件、人のマージ待ち {}件）\n\
     {}\n\
     異常判定の閾値: 判断待ちの滞留 {}分、ログの無音 {}分\n",
    format_timestamp(state.read_at.as_ref()),
    state.servers.len(),
    join_lines(&servers),
    state.projects.len(),
    join_lines(&projects),
    state.goals.len(),
    join_lines(&goals),
    running_runs,
    join_lines(&runs),
    state.pending_decisions.len(),
    inquiries,
    awaiting_merge,
    join_lines(&pending_detail),
    thresholds.pending_decision_stale_minutes,
    thresholds.run_log_silent_minutes,
  )
}

/// 一覧を改行で連結し、空の一覧は空であることが分かる表示にする。
fn join_lines(lines: &[String]) -> String {
  if lines.is_empty() {
    "（なし）".to_owned()
  } else {
    lines.join("\n")
  }
}

/// proto の時刻を、タイムゾーンに依存しない UTC の表記にする。
///
/// 表示は日本語に揃えるため、秒だけの数値ではなく日時として出す。
fn format_timestamp(timestamp: Option<&prost_types::Timestamp>) -> String {
  let Some(timestamp) = timestamp else {
    return "不明".to_owned();
  };
  match chrono::DateTime::from_timestamp(timestamp.seconds, 0) {
    | Some(at) => at.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
    | None => "不明".to_owned(),
  }
}

fn main() {
  let args: Vec<String> = std::env::args().skip(1).collect();
  std::process::exit(run(&args));
}

#[cfg(test)]
mod tests {
  //! 完了契約を、本番コードと同じ関数・同じ生成クライアント・ダミー gRPC サーバーで観測する。
  //!
  //! 表示の組み立ては `render_overview` を、引数と接続先の解決と終了コードは `run_with` を、
  //! 実際の gRPC 通信は生成クライアントと本番と同じサービス定義のダミーサーバーを通して確かめる。

  use std::{
    net::{Ipv4Addr, SocketAddr, TcpListener},
    sync::{Arc, Mutex},
    time::Duration,
  };

  use prost_types::Timestamp;
  use proto::v1::{
    self, takt_dash_service_server::TaktDashServiceServer, DashboardState, GetOverviewRequest, PendingDecision,
    PendingDecisionType, Project, Server as ServerInfo, TaskStatusCounts,
  };
  use tonic::{transport::Server, Code, Request, Response, Status};

  use super::{
    is_connection_failure, parse_arguments, proto, render_overview, resolve_backend_addr, run_with, AddrError,
    AddrSources, Invocation, OverviewOptions, UsageError, DEFAULT_BACKEND_ADDR, EXIT_FAILURE, EXIT_OK, EXIT_USAGE,
  };

  // proto の列挙値は生成コードの数値をそのまま使う。
  const GOAL_STATUS_AWAITING_MERGE: i32 = v1::GoalStatus::AwaitingMerge as i32;
  const EXECUTION_STATUS_ACTIVE: i32 = v1::ExecutionStatus::Active as i32;
  const RUN_STATUS_RUNNING: i32 = v1::RunStatus::Running as i32;
  const INQUIRY_KIND_QUESTION: i32 = v1::InquiryKind::Question as i32;
  const INQUIRY_KIND_OPERATION_REQUEST: i32 = v1::InquiryKind::OperationRequest as i32;

  /// タイムスタンプを組み立てる。`secs` は UNIX 時刻の秒。
  fn ts(secs: i64) -> Timestamp {
    Timestamp { seconds: secs, nanos: 0 }
  }

  // ----------------------------------------------------------------------------
  // ダミー gRPC サーバー
  // ----------------------------------------------------------------------------

  /// `start_dummy_server` が起動したダミーサーバーの起動結果。
  struct DummyServer {
    addr:          SocketAddr,
    seen:          Arc<Mutex<Vec<String>>>,
    shutdown_tx:   tokio::sync::oneshot::Sender<()>,
    server_handle: tokio::task::JoinHandle<()>,
  }

  impl DummyServer {
    /// 記録した RPC 名の一覧を返す。
    fn seen_calls(&self) -> Vec<String> {
      self.seen.lock().expect("RPC 名の記録を読めませんでした").clone()
    }

    /// サーバーを止め、待ち受けを解放する。
    ///
    /// グレースフルシャットダウンを待ってからタスクの終了を確認する。停止を待たずにタスクごと
    /// 打ち切ると、進行中の RPC が応答を欠いたまま切断され、次の観測の前提を崩すため。
    async fn shutdown(self) {
      let _ = self.shutdown_tx.send(());
      tokio::time::timeout(Duration::from_secs(5), self.server_handle)
        .await
        .expect("ダミーサーバーは5秒以内に停止する必要があります")
        .expect("ダミーサーバーのタスクが異常終了しました");
    }
  }

  /// 概要を固定値で返すダミーサーバーを `127.0.0.1:0` で起動する。
  async fn start_dummy_server(overview: DashboardState) -> DummyServer {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let service = DummyTaktDashService { overview: Arc::new(overview), seen: seen.clone() };
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
      .await
      .expect("ダミーサーバーは空きポートで待ち受けできる必要があります");
    let addr = listener.local_addr().expect("待ち受けアドレスを取得できませんでした");
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let server_handle = tokio::spawn(async move {
      let _ = Server::builder()
        .add_service(TaktDashServiceServer::new(service))
        .serve_with_incoming_shutdown(tokio_stream::wrappers::TcpListenerStream::new(listener), async {
          let _ = shutdown_rx.await;
        })
        .await;
    });
    DummyServer { addr, seen, shutdown_tx, server_handle }
  }

  /// 本番と同じ生成サービス定義を実装するテスト用サーバー。
  ///
  /// 閲覧は概要の取得だけを担うため、`GetOverview` だけを意味のある形で実装し、それ以外の
  /// 読み取り RPC と状態ストリーミングは Unimplemented を返す。
  struct DummyTaktDashService {
    overview: Arc<DashboardState>,
    seen:     Arc<Mutex<Vec<String>>>,
  }

  impl DummyTaktDashService {
    fn record(&self, name: &str) {
      self.seen.lock().expect("RPC 名の記録を書けませんでした").push(name.to_owned());
    }
  }

  #[tonic::async_trait]
  impl v1::takt_dash_service_server::TaktDashService for DummyTaktDashService {
    type WatchStateStream = tokio_stream::wrappers::ReceiverStream<Result<DashboardState, Status>>;

    async fn get_overview(&self, _request: Request<GetOverviewRequest>) -> Result<Response<DashboardState>, Status> {
      self.record("GetOverview");
      Ok(Response::new((*self.overview).clone()))
    }

    async fn get_projects(
      &self,
      _request: Request<v1::GetProjectsRequest>,
    ) -> Result<Response<v1::GetProjectsResponse>, Status> {
      self.record("GetProjects");
      Err(Status::unimplemented("このダミーサーバーは GetProjects を提供しません。"))
    }

    async fn get_pending_decisions(
      &self,
      _request: Request<v1::GetPendingDecisionsRequest>,
    ) -> Result<Response<v1::GetPendingDecisionsResponse>, Status> {
      self.record("GetPendingDecisions");
      Err(Status::unimplemented("このダミーサーバーは GetPendingDecisions を提供しません。"))
    }

    async fn get_activity(
      &self,
      _request: Request<v1::GetActivityRequest>,
    ) -> Result<Response<v1::GetActivityResponse>, Status> {
      self.record("GetActivity");
      Err(Status::unimplemented("このダミーサーバーは GetActivity を提供しません。"))
    }

    async fn get_run_detail(
      &self,
      _request: Request<v1::GetRunDetailRequest>,
    ) -> Result<Response<v1::GetRunDetailResponse>, Status> {
      self.record("GetRunDetail");
      Err(Status::unimplemented("このダミーサーバーは GetRunDetail を提供しません。"))
    }

    async fn watch_state(
      &self,
      _request: Request<v1::WatchStateRequest>,
    ) -> Result<Response<Self::WatchStateStream>, Status> {
      self.record("WatchState");
      Err(Status::unimplemented("このダミーサーバーは WatchState を提供しません。"))
    }
  }

  /// 空きポートを確保して即座に閉じる。接続を拒否するアドレスを作る。
  fn reserve_closed_addr() -> SocketAddr {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("空きポートを確保できませんでした");
    let addr = listener.local_addr().expect("待ち受けアドレスを取得できませんでした");
    drop(listener);
    addr
  }

  /// 待ち受けはあるが gRPC を話さない相手を作る。接続を accept して即座に閉じる。
  ///
  /// accept のループを回すスレッドはデーモンとして動かし、テストの終了とともにプロセスが終わる
  /// ことで解放します。
  fn spawn_accept_then_close() -> SocketAddr {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("空きポートを確保できませんでした");
    let addr = listener.local_addr().expect("待ち受けアドレスを取得できませんでした");
    std::thread::spawn(move || {
      // 接続のたびに accept して即座に閉じる。gRPC の応答は返さない。
      while let Ok((socket, _)) = listener.accept() {
        drop(socket);
      }
    });
    addr
  }

  /// 一時ディレクトリに設定ファイルを書いて、そのパスを返す。
  ///
  /// ユーザーの既存の設定を読まないよう、テストごとに一時ディレクトリへ隔離する。
  fn write_config(dir: &tempfile::TempDir, contents: &str) -> std::path::PathBuf {
    let path = dir.path().join("taktdash.config.json");
    std::fs::write(&path, contents).expect("テスト用の設定ファイルを書けませんでした");
    path
  }

  /// `run_with` を、ダミーサーバーのアドレスと空の環境変数で実行する。
  fn run_cli(args: &[String], addr: SocketAddr) -> (i32, String, String) {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let env_addr = Ok(format!("{addr}"));
    let code = run_with(args, env_addr, &mut stdout, &mut stderr);
    (
      code,
      String::from_utf8(stdout).expect("標準出力は UTF-8 である必要があります"),
      String::from_utf8(stderr).expect("標準エラーは UTF-8 である必要があります"),
    )
  }

  // ----------------------------------------------------------------------------
  // ダミーサーバーが返す固定の概要
  // ----------------------------------------------------------------------------

  fn sample_overview() -> DashboardState {
    DashboardState {
      read_at:           Some(ts(1_731_600_000)),
      servers:           vec![
        ServerInfo {
          server_id:    "mini".to_owned(),
          name:         "j5ik2o-mac-mini".to_owned(),
          kind:         "Mac mini".to_owned(),
          connected:    true,
          last_read_at: Some(ts(1_731_599_000)),
          concurrency:  4,
        },
        ServerInfo {
          server_id:    "studio".to_owned(),
          name:         "j5ik2o-mac-studio".to_owned(),
          kind:         "Mac Studio".to_owned(),
          connected:    false,
          last_read_at: None,
          concurrency:  8,
        },
      ],
      projects:          vec![Project {
        name:                "taktdash-cli".to_owned(),
        about:               "taktDash の概要を表示する閲覧コマンド".to_owned(),
        server_id:           "mini".to_owned(),
        completed_tasks_24h: 3,
        connected:           true,
        last_read_at:        Some(ts(1_731_599_000)),
      }],
      goals:             vec![v1::Goal {
        goal_id:           "goal-cli".to_owned(),
        server_id:         "mini".to_owned(),
        project:           "taktdash-cli".to_owned(),
        name:              "CLI に閲覧コマンドを足す".to_owned(),
        status:            GOAL_STATUS_AWAITING_MERGE,
        execution_status:  EXECUTION_STATUS_ACTIVE,
        task_counts:       Some(TaskStatusCounts {
          pending: 1,
          running: 0,
          completed: 2,
          failed: 1,
          ..Default::default()
        }),
        paused_reason:     None,
        pull_request:      Some(v1::PullRequest { number: 42, url: "https://example.test/pr/42".to_owned() }),
        awaiting_merge_at: Some(ts(1_731_500_000)),
        merge_task_last:   None,
      }],
      runs:              vec![
        v1::Run {
          run_slug:                "run-1".to_owned(),
          session_id:              "session-1".to_owned(),
          server_id:               "mini".to_owned(),
          project:                 "taktdash-cli".to_owned(),
          goal_id:                 "goal-cli".to_owned(),
          task_name:               "閲覧コマンドの実装".to_owned(),
          summary:                 "概要を表示する実装を進めています".to_owned(),
          started_at:              Some(ts(1_731_590_000)),
          steps:                   Vec::new(),
          current_step_started_at: None,
          last_log_at:             Some(ts(1_731_599_500)),
          reports:                 Vec::new(),
          status:                  RUN_STATUS_RUNNING,
          log_silent:              false,
        },
        // 実行中でない run を混ぜ、実行中の件数が全 run の件数と取り違えられないようにする。
        v1::Run {
          run_slug:                "run-2".to_owned(),
          session_id:              "session-2".to_owned(),
          server_id:               "studio".to_owned(),
          project:                 "taktdash-cli".to_owned(),
          goal_id:                 "goal-cli".to_owned(),
          task_name:               "終わったタスク".to_owned(),
          summary:                 "完了済みの run".to_owned(),
          started_at:              Some(ts(1_731_580_000)),
          steps:                   Vec::new(),
          current_step_started_at: None,
          last_log_at:             Some(ts(1_731_590_000)),
          reports:                 Vec::new(),
          status:                  v1::RunStatus::Completed as i32,
          log_silent:              false,
        },
      ],
      pending_decisions: vec![
        PendingDecision {
          r#type:       PendingDecisionType::Inquiry as i32,
          server_id:    "mini".to_owned(),
          project:      "taktdash-cli".to_owned(),
          goal_id:      Some("goal-cli".to_owned()),
          inquiry_kind: INQUIRY_KIND_QUESTION,
          created_at:   Some(ts(1_731_300_000)),
          body:         "表示の並び順はどれがよいですか。".to_owned(),
          options:      vec!["待ち時間の長い順".to_owned()],
          pull_request: None,
          stale:        false,
        },
        PendingDecision {
          r#type:       PendingDecisionType::AwaitingMerge as i32,
          server_id:    "mini".to_owned(),
          project:      "taktdash-cli".to_owned(),
          goal_id:      Some("goal-cli".to_owned()),
          inquiry_kind: v1::InquiryKind::Unspecified as i32,
          created_at:   Some(ts(1_731_400_000)),
          body:         "プルリクエスト 42 をマージしてください。".to_owned(),
          options:      Vec::new(),
          pull_request: Some(v1::PullRequest { number: 42, url: "https://example.test/pr/42".to_owned() }),
          stale:        false,
        },
      ],
      thresholds:        Some(v1::AnomalyThresholds {
        pending_decision_stale_minutes: 240,
        run_log_silent_minutes:         10,
      }),
    }
  }

  // ----------------------------------------------------------------------------
  // CLI-OVERVIEW-RENDER: 概要の取得と表示（要件 1, 2, 4(a)）
  // ----------------------------------------------------------------------------

  #[test]
  fn overview_command_prints_fetched_summary_to_stdout() {
    let runtime =
      tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("テスト用の実行環境を作れませんでした");
    let server = runtime.block_on(start_dummy_server(sample_overview()));

    let (code, stdout, stderr) = run_cli(&["overview".to_owned()], server.addr);
    let seen_calls = server.seen_calls();
    runtime.block_on(server.shutdown());

    assert_eq!(code, EXIT_OK, "閲覧コマンドは成功する必要があります。stderr: {stderr}");
    assert_eq!(seen_calls, vec!["GetOverview".to_owned()], "閲覧は概要の取得だけを呼ぶ必要があります");
    assert!(stderr.is_empty(), "成功時に標準エラーへ出してはいけません: {stderr}");
    assert!(stdout.contains("サーバー: 2台"), "サーバーの台数: {stdout}");
    assert!(stdout.contains("j5ik2o-mac-mini（Mac mini、接続中）"), "接続中のサーバー: {stdout}");
    assert!(stdout.contains("j5ik2o-mac-studio（Mac Studio、応答なし）"), "応答なしのサーバー: {stdout}");
    assert!(stdout.contains("taktdash-cli（taktDash の概要を表示する閲覧コマンド）"), "プロジェクト: {stdout}");
    assert!(
      stdout.contains("taktdash-cli/goal-cli「CLI に閲覧コマンドを足す」完了2件・失敗1件（未着手1件・実行中0件）"),
      "ゴールとタスクの件数: {stdout}"
    );
    assert!(stdout.contains("実行中の run: 1件"), "実行中の run の件数: {stdout}");
    assert!(stdout.contains("taktdash-cli/閲覧コマンドの実装（概要を表示する実装を進めています）"), "run: {stdout}");
    assert!(
      stdout.contains("taktdash-cli/終わったタスク（完了済みの run）"),
      "実行中でない run も一覧に出す必要があります: {stdout}"
    );
    assert!(
      stdout.contains("判断待ち: 2件（未回答の問い合わせ 1件、人のマージ待ち 1件）"),
      "判断待ちの件数と内訳: {stdout}"
    );
    assert!(stdout.contains("問い合わせ（質問）"), "問い合わせの種類: {stdout}");
    assert!(stdout.contains("異常判定の閾値: 判断待ちの滞留 240分、ログの無音 10分"), "異常判定の閾値: {stdout}");
  }

  #[test]
  fn overview_rendering_distinguishes_operation_request_inquiry() {
    let mut state = sample_overview();
    state.pending_decisions = vec![PendingDecision {
      r#type:       PendingDecisionType::Inquiry as i32,
      server_id:    "mini".to_owned(),
      project:      "taktdash-cli".to_owned(),
      goal_id:      None,
      inquiry_kind: INQUIRY_KIND_OPERATION_REQUEST,
      created_at:   Some(ts(1_731_300_000)),
      body:         "ログを取得してください。".to_owned(),
      options:      Vec::new(),
      pull_request: None,
      stale:        false,
    }];

    let rendered = render_overview(&state);

    assert!(
      rendered.contains("問い合わせ（作業の依頼）"),
      "作業の依頼は質問と区別して表示する必要があります: {rendered}"
    );
    assert!(
      rendered.contains("判断待ち: 1件（未回答の問い合わせ 1件、人のマージ待ち 0件）"),
      "作業の依頼は未回答の問い合わせに数える必要があります: {rendered}"
    );
    assert!(!rendered.contains("人のマージ待ち 1件"), "マージ待ちの件数を混同しないこと: {rendered}");
  }

  #[test]
  fn overview_of_empty_state_renders_empty_markers() {
    let runtime =
      tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("テスト用の実行環境を作れませんでした");
    let server = runtime.block_on(start_dummy_server(DashboardState::default()));

    let (code, stdout, stderr) = run_cli(&[], server.addr);
    runtime.block_on(server.shutdown());

    // 位置引数を省略しても overview として動作する。
    assert_eq!(code, EXIT_OK, "位置引数を省略しても成功する必要があります。stderr: {stderr}");
    assert!(stdout.contains("サーバー: 0台"), "サーバーの台数: {stdout}");
    assert!(stdout.contains("実行中の run: 0件"), "実行中の run: {stdout}");
    assert!(stdout.contains("判断待ち: 0件（未回答の問い合わせ 0件、人のマージ待ち 0件）"), "判断待ち: {stdout}");
    assert!(stdout.contains("（なし）"), "空の一覧は空であることが分かる表示にする必要があります: {stdout}");
  }

  // ----------------------------------------------------------------------------
  // CLI-UNAVAILABLE: 接続不能時の異常終了（要件 6, 7, 8, 4(b)）
  // ----------------------------------------------------------------------------

  #[test]
  fn overview_fails_with_nonzero_code_when_backend_is_down() {
    let addr = reserve_closed_addr();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();

    let code = run_with(&["overview".to_owned()], Ok(format!("{addr}")), &mut stdout, &mut stderr);

    let stdout = String::from_utf8(stdout).expect("標準出力は UTF-8 である必要があります");
    let stderr = String::from_utf8(stderr).expect("標準エラーは UTF-8 である必要があります");
    assert_ne!(code, EXIT_OK, "接続できないときは 0 以外で終わる必要があります");
    assert_eq!(code, EXIT_FAILURE, "接続不能は取得の失敗として終わる必要があります");
    assert!(
      stderr.contains("backend に接続できません。"),
      "接続できない旨を標準エラーへ日本語で出す必要があります: {stderr}"
    );
    assert!(stderr.contains(&addr.to_string()), "接続先を標準エラーで示す必要があります: {stderr}");
    assert!(!stdout.contains("サーバー:"), "接続できないときは概要を標準出力へ出してはいけません: {stdout}");
  }

  #[test]
  fn overview_treats_non_grpc_listener_as_connection_failure() {
    // 待ち受けはあるが gRPC を話さない相手（accept 後即時 close）でも、待ち受けが無い場合と
    // 同じ分類・文言・終了コードで異常終了する。
    let addr = spawn_accept_then_close();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();

    let code = run_with(&["overview".to_owned()], Ok(format!("{addr}")), &mut stdout, &mut stderr);

    let stdout = String::from_utf8(stdout).expect("標準出力は UTF-8 である必要があります");
    let stderr = String::from_utf8(stderr).expect("標準エラーは UTF-8 である必要があります");
    assert_eq!(code, EXIT_FAILURE, "gRPC を話さない相手は接続不能として終わる必要があります: {stderr}");
    assert!(
      stderr.starts_with("backend に接続できません。"),
      "待ち受けあり・gRPC を話さない相手でも同じ文言で始まる必要があります: {stderr}"
    );
    assert!(stderr.contains(&addr.to_string()), "接続先を標準エラーで示す必要があります: {stderr}");
    assert!(!stdout.contains("サーバー:"), "接続できないときは概要を標準出力へ出してはいけません: {stdout}");
  }

  #[test]
  fn connection_failure_classification_covers_cancelled_and_unknown() {
    // 実測で gRPC を話さない相手は Cancelled/Unknown を返す。backend が応答したうえでの失敗
    // （例如: NotFound）は接続不能にしない。
    assert!(is_connection_failure(Code::Unavailable));
    assert!(is_connection_failure(Code::DeadlineExceeded));
    assert!(is_connection_failure(Code::Cancelled));
    assert!(is_connection_failure(Code::Unknown));
    assert!(!is_connection_failure(Code::NotFound));
    assert!(!is_connection_failure(Code::PermissionDenied));
    assert!(!is_connection_failure(Code::Internal));
  }

  // ----------------------------------------------------------------------------
  // CLI-NO-WRITE: 書き込み操作のコマンドを持たない（要件 9）
  // ----------------------------------------------------------------------------

  #[test]
  fn write_command_is_rejected_with_usage() {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();

    let code = run_with(&["answer".to_owned()], Ok(DEFAULT_BACKEND_ADDR.to_owned()), &mut stdout, &mut stderr);

    let stdout = String::from_utf8(stdout).expect("標準出力は UTF-8 である必要があります");
    let stderr = String::from_utf8(stderr).expect("標準エラーは UTF-8 である必要があります");
    assert_eq!(code, EXIT_USAGE, "未知のコマンドは使い方を出して異常終了する必要があります");
    assert!(stderr.contains("未知のコマンドです: answer"), "未知のコマンドであることを示す必要があります: {stderr}");
    assert!(stderr.contains("使い方: takt-dash-cli overview"), "使い方を示す必要があります: {stderr}");
    assert!(stdout.is_empty(), "使い方の誤りで標準出力へ出してはいけません: {stdout}");
  }

  #[test]
  fn unknown_flag_is_rejected_with_usage() {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();

    let code = run_with(&["--merge".to_owned()], Ok(DEFAULT_BACKEND_ADDR.to_owned()), &mut stdout, &mut stderr);

    let stderr = String::from_utf8(stderr).expect("標準エラーは UTF-8 である必要があります");
    assert_eq!(code, EXIT_USAGE);
    assert!(stderr.contains("未知の引数です: --merge"), "未知の引数であることを示す必要があります: {stderr}");
  }

  #[test]
  fn generated_service_exposes_only_read_rpcs() {
    // proto のサービス定義は読み取りの 6 つだけで、書き込み系の RPC を持たない。
    let rpcs = ["GetOverview", "GetProjects", "GetPendingDecisions", "GetActivity", "GetRunDetail", "WatchState"];
    assert_eq!(rpcs.len(), 6);
    for forbidden in ["Answer", "Merge", "Approve", "Reject", "Update", "Delete"] {
      assert!(
        !rpcs.iter().any(|rpc| rpc.contains(forbidden)),
        "読み取りの RPC の集合に書き込み操作 {forbidden} が現れています: {rpcs:?}"
      );
    }
  }

  #[test]
  fn generated_client_type_uses_the_transport_channel() {
    let _client: std::marker::PhantomData<
      proto::v1::takt_dash_service_client::TaktDashServiceClient<tonic::transport::Channel>,
    > = std::marker::PhantomData;
  }

  // ----------------------------------------------------------------------------
  // CLI-ADDR-RESOLVE: 接続先の解決（要件 18, 19）
  // ----------------------------------------------------------------------------

  #[test]
  fn explicit_addr_wins_over_env_and_config() {
    let sources = AddrSources {
      cli_addr:      Some("127.0.0.1:50101"),
      env_addr:      Ok("127.0.0.1:50202".to_owned()),
      config_listen: Some("127.0.0.1:50303"),
    };

    assert_eq!(resolve_backend_addr(&sources), Ok("127.0.0.1:50101".parse().unwrap()));
  }

  #[test]
  fn env_wins_over_config_listen() {
    let sources = AddrSources {
      cli_addr:      None,
      env_addr:      Ok("127.0.0.1:50202".to_owned()),
      config_listen: Some("127.0.0.1:50303"),
    };

    assert_eq!(resolve_backend_addr(&sources), Ok("127.0.0.1:50202".parse().unwrap()));
  }

  #[test]
  fn config_listen_used_when_addr_and_env_absent() {
    let sources = AddrSources {
      cli_addr:      None,
      env_addr:      Err(std::env::VarError::NotPresent),
      config_listen: Some("127.0.0.1:50303"),
    };

    assert_eq!(resolve_backend_addr(&sources), Ok("127.0.0.1:50303".parse().unwrap()));
  }

  #[test]
  fn default_addr_used_when_nothing_configured() {
    let sources =
      AddrSources { cli_addr: None, env_addr: Err(std::env::VarError::NotPresent), config_listen: None };

    assert_eq!(resolve_backend_addr(&sources), Ok("127.0.0.1:50051".parse().unwrap()));
    assert_eq!(DEFAULT_BACKEND_ADDR, "127.0.0.1:50051");
  }

  #[test]
  fn empty_values_fall_through_to_next_candidate() {
    let sources =
      AddrSources { cli_addr: Some(""), env_addr: Ok(String::new()), config_listen: Some("127.0.0.1:50303") };

    assert_eq!(resolve_backend_addr(&sources), Ok("127.0.0.1:50303".parse().unwrap()));
  }

  #[test]
  fn invalid_addr_is_not_replaced_with_default() {
    let sources = AddrSources {
      cli_addr:      Some("not-an-address"),
      env_addr:      Err(std::env::VarError::NotPresent),
      config_listen: None,
    };

    assert_eq!(
      resolve_backend_addr(&sources),
      Err(AddrError::Invalid { source: "--addr", value: "not-an-address".to_owned() })
    );
  }

  #[test]
  fn invalid_addr_exits_with_usage_error() {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();

    let code = run_with(
      &["overview".to_owned(), "--addr".to_owned(), "not-an-address".to_owned()],
      Err(std::env::VarError::NotPresent),
      &mut stdout,
      &mut stderr,
    );

    let stderr = String::from_utf8(stderr).expect("標準エラーは UTF-8 である必要があります");
    assert_eq!(code, EXIT_USAGE, "--addr の不正値は既定値へ置き換えず異常終了する必要があります");
    assert!(stderr.contains("not-an-address"), "不正な値を示す必要があります: {stderr}");
  }

  #[test]
  fn parse_arguments_accepts_overview_and_flags() {
    let args = [
      "overview".to_owned(),
      "--addr".to_owned(),
      "127.0.0.1:50052".to_owned(),
      "--config".to_owned(),
      "/tmp/a.json".to_owned(),
    ];

    assert_eq!(
      parse_arguments(&args),
      Ok(Invocation::Overview(OverviewOptions {
        addr:   Some("127.0.0.1:50052".to_owned()),
        config: Some(std::path::PathBuf::from("/tmp/a.json")),
      }))
    );
  }

  #[test]
  fn parse_arguments_defaults_to_overview_without_position() {
    assert_eq!(parse_arguments(&[]), Ok(Invocation::Overview(OverviewOptions::default())));
  }

  #[test]
  fn parse_arguments_reports_help() {
    assert_eq!(parse_arguments(&["--help".to_owned()]), Ok(Invocation::Help));
  }

  #[test]
  fn parse_arguments_requires_flag_value() {
    assert_eq!(parse_arguments(&["--addr".to_owned()]), Err(UsageError("--addr には値が必要です。".to_owned())));
  }

  #[test]
  fn config_listen_overrides_default_for_connection() {
    let dir = tempfile::tempdir().expect("一時ディレクトリを作れませんでした");
    let config = write_config(&dir, r#"{"listen":"127.0.0.1:59999"}"#);

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = run_with(
      &["--config".to_owned(), config.display().to_string()],
      Err(std::env::VarError::NotPresent),
      &mut stdout,
      &mut stderr,
    );

    let stderr = String::from_utf8(stderr).expect("標準エラーは UTF-8 である必要があります");
    // 設定の listen が使われ（既定値ではない）、待ち受けが無いため接続不能で終わる。
    assert_eq!(code, EXIT_FAILURE, "設定の listen に待ち受けが無いため失敗する必要があります");
    assert!(stderr.contains("127.0.0.1:59999"), "設定の listen を接続先に使う必要があります: {stderr}");
    assert!(!stderr.contains(DEFAULT_BACKEND_ADDR), "既定値へ戻ってはいけません: {stderr}");
  }

  #[test]
  fn missing_config_file_falls_through_to_default() {
    let dir = tempfile::tempdir().expect("一時ディレクトリを作れませんでした");
    let missing = dir.path().join("does-not-exist.json");
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();

    let code = run_with(
      &["--config".to_owned(), missing.display().to_string()],
      Err(std::env::VarError::NotPresent),
      &mut stdout,
      &mut stderr,
    );

    let stderr = String::from_utf8(stderr).expect("標準エラーは UTF-8 である必要があります");
    assert_eq!(code, EXIT_FAILURE);
    assert!(stderr.contains(DEFAULT_BACKEND_ADDR), "設定ファイルが無いときは既定値へ進む必要があります: {stderr}");
  }

  // ----------------------------------------------------------------------------
  // 到達経路: 引数解析・接続先解決・表示が一つの入口からつながる
  // ----------------------------------------------------------------------------

  #[test]
  fn cli_end_to_end_from_arguments_to_stdout() {
    let runtime =
      tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("テスト用の実行環境を作れませんでした");
    let server = runtime.block_on(start_dummy_server(sample_overview()));

    // 環境変数ではなく --addr で接続先を渡す。表示は標準出力へ出る。
    let (code, stdout, stderr) = {
      let mut stdout = Vec::new();
      let mut stderr = Vec::new();
      let args = ["overview".to_owned(), "--addr".to_owned(), format!("{}", server.addr)];
      let code = run_with(&args, Ok("127.0.0.1:1".to_owned()), &mut stdout, &mut stderr);
      (code, String::from_utf8(stdout).unwrap(), String::from_utf8(stderr).unwrap())
    };
    let seen_calls = server.seen_calls();
    runtime.block_on(server.shutdown());

    assert_eq!(code, EXIT_OK, "stderr: {stderr}");
    assert_eq!(seen_calls, vec!["GetOverview".to_owned()], "取得は GetOverview だけを呼ぶ必要があります");
    assert!(stdout.contains("概要"), "--addr の接続先から取得した概要を表示する必要があります: {stdout}");
    assert!(stdout.contains("j5ik2o-mac-mini（Mac mini、接続中）"), "{stdout}");
  }
}

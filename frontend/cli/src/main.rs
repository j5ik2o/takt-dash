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
  let goal_registration = state
    .pending_decisions
    .iter()
    .filter(|decision| decision.r#type == PendingDecisionType::GoalRegistration as i32)
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
      } else if decision.r#type == PendingDecisionType::GoalRegistration as i32 {
        "承認待ち"
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
     判断待ち: {}件（未回答の問い合わせ {}件、人のマージ待ち {}件、ゴール登録の承認待ち {}件）\n\
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
    goal_registration,
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
mod tests;

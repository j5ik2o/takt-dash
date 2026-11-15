//! taktDash のコマンドラインの入口。
//!
//! backend の gRPC へ接続し、概要の閲覧と、問い合わせへの回答・マージの依頼・ゴール登録の
//! 承認待ちの一覧・承認・却下を行います。表示は日本語に揃えます。

mod proto;

#[cfg(test)]
mod write_command_tests;

use std::{env::VarError, io::Write, net::SocketAddr, path::PathBuf, time::Duration};

use proto::v1::{
  takt_dash_service_client::TaktDashServiceClient, AnswerInquiryRequest, AnswerInquiryResponse, AnswerReport,
  ApproveGoalRegistrationRequest, ApproveGoalRegistrationResponse, ApproveMergeRequest, ApproveMergeResponse,
  DashboardState, GetOverviewRequest, GoalRegistrationProposal, ListGoalRegistrationProposalsRequest,
  PendingDecisionType, RejectGoalRegistrationRequest, RejectGoalRegistrationResponse,
};
use tonic::{
  transport::{Channel, Endpoint},
  Code, Status,
};

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

/// 既定のコマンドの名前。位置引数の既定値でもあります。
const OVERVIEW_COMMAND: &str = "overview";
/// ゴール登録の承認待ちの種別表示。問い合わせ（質問）とは別の語彙を使います。
const GOAL_REGISTRATION_LABEL: &str = "ゴール登録の承認待ち";

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
  /// 書き込み操作を実行する。
  Write(WriteInvocation),
  /// 使い方を表示して正常終了する。
  Help,
}

/// 概要の表示に使う解決前の入力。
#[derive(Debug, Default, PartialEq, Eq)]
struct OverviewOptions {
  addr:   Option<String>,
  config: Option<PathBuf>,
}

/// 書き込みコマンドと、その接続先の解決前の入力。
#[derive(Debug, PartialEq, Eq)]
struct WriteInvocation {
  command: WriteCommand,
  addr:    Option<String>,
  config:  Option<PathBuf>,
}

/// 書き込みコマンド。引数は解析時に検証済みです。
#[derive(Debug, PartialEq, Eq)]
enum WriteCommand {
  /// 問い合わせへ回答する。
  Answer { goal: String, question: String, text: String, report: AnswerReport },
  /// プルリクエストのマージを依頼する。
  Merge { goal: String },
  /// ゴール登録の承認待ちを取得する。
  Registrations { project: Option<String> },
  /// ゴール登録を承認する。
  ApproveRegistration { project: String, revision: i64 },
  /// ゴール登録を却下する。
  RejectRegistration { project: String, revision: i64 },
}

impl WriteCommand {
  /// 失敗の表示に使う操作名。
  fn operation(&self) -> &'static str {
    match self {
      | WriteCommand::Answer { .. } => "回答の記録",
      | WriteCommand::Merge { .. } => "マージの依頼",
      | WriteCommand::Registrations { .. } => "ゴール登録の承認待ちの取得",
      | WriteCommand::ApproveRegistration { .. } => "ゴール登録の承認",
      | WriteCommand::RejectRegistration { .. } => "ゴール登録の却下",
    }
  }
}

/// 引数の解釈に失敗したときに返す失敗。使い方と一緒に表示します。
#[derive(Debug, PartialEq, Eq)]
struct UsageError(String);

/// 既知のコマンドかどうか。
fn is_known_command(name: &str) -> bool {
  matches!(
    name,
    OVERVIEW_COMMAND | "answer" | "merge" | "registrations" | "approve-registration" | "reject-registration"
  )
}

/// そのコマンドで使えるフラグかどうか。`command` が未確定のときは共通のフラグだけを許します。
fn flag_allowed(command: Option<&str>, flag: &str) -> bool {
  match flag {
    | "--addr" | "--config" => true,
    | "--goal" => matches!(command, Some("answer") | Some("merge")),
    | "--question" | "--text" | "--report" => command == Some("answer"),
    | "--project" => {
      matches!(command, Some("registrations") | Some("approve-registration") | Some("reject-registration"))
    },
    | "--revision" => matches!(command, Some("approve-registration") | Some("reject-registration")),
    | "--yes" => matches!(command, Some("merge") | Some("approve-registration") | Some("reject-registration")),
    | _ => false,
  }
}

/// コマンドラインの引数を解釈する。
fn parse_arguments(args: &[String]) -> Result<Invocation, UsageError> {
  let mut addr = None;
  let mut config = None;
  let mut goal = None;
  let mut question = None;
  let mut text = None;
  let mut report = None;
  let mut project = None;
  let mut revision = None;
  let mut yes = false;
  let mut command: Option<&str> = None;
  let mut index = 0;
  while index < args.len() {
    let argument = args[index].as_str();
    if argument == "--help" || argument == "-h" {
      return Ok(Invocation::Help);
    }
    if argument.starts_with('-') {
      if !flag_allowed(command, argument) {
        return Err(UsageError(format!("未知の引数です: {argument}")));
      }
      match argument {
        | "--addr" => {
          addr = Some(take_value(args, index, "--addr")?);
          index += 2;
        },
        | "--config" => {
          config = Some(PathBuf::from(take_value(args, index, "--config")?));
          index += 2;
        },
        | "--goal" => {
          goal = Some(take_value(args, index, "--goal")?);
          index += 2;
        },
        | "--question" => {
          question = Some(take_value(args, index, "--question")?);
          index += 2;
        },
        | "--text" => {
          text = Some(take_optional_empty_value(args, index, "--text")?);
          index += 2;
        },
        | "--report" => {
          report = Some(take_value(args, index, "--report")?);
          index += 2;
        },
        | "--project" => {
          project = Some(take_value(args, index, "--project")?);
          index += 2;
        },
        | "--revision" => {
          revision = Some(take_value(args, index, "--revision")?);
          index += 2;
        },
        | "--yes" => {
          yes = true;
          index += 1;
        },
        | _ => return Err(UsageError(format!("未知の引数です: {argument}"))),
      }
      continue;
    }
    if command.is_none() {
      if !is_known_command(argument) {
        return Err(UsageError(format!("未知のコマンドです: {argument}")));
      }
      command = Some(argument);
      index += 1;
      continue;
    }
    return Err(UsageError(format!("{} に位置引数は取れません: {argument}", command.unwrap_or(OVERVIEW_COMMAND))));
  }
  let command = command.unwrap_or(OVERVIEW_COMMAND);
  if command == OVERVIEW_COMMAND {
    return Ok(Invocation::Overview(OverviewOptions { addr, config }));
  }
  let write = |command: WriteCommand| Invocation::Write(WriteInvocation { command, addr, config });
  match command {
    | "answer" => {
      let goal = goal.ok_or_else(|| UsageError("answer には --goal が必要です。".to_owned()))?;
      let question = question.ok_or_else(|| UsageError("answer には --question が必要です。".to_owned()))?;
      let text = text.unwrap_or_default();
      let report = match report.as_deref() {
        | None => AnswerReport::Unspecified,
        | Some("completed") => AnswerReport::Completed,
        | Some("unable") => AnswerReport::Unable,
        | Some(other) => {
          return Err(UsageError(format!("--report には completed か unable を指定してください: {other}")));
        },
      };
      if text.trim().is_empty() && report == AnswerReport::Unspecified {
        return Err(UsageError("answer には --text か --report が必要です。".to_owned()));
      }
      Ok(write(WriteCommand::Answer { goal, question, text, report }))
    },
    | "merge" => {
      let goal = goal.ok_or_else(|| UsageError("merge には --goal が必要です。".to_owned()))?;
      require_yes("merge", yes)?;
      Ok(write(WriteCommand::Merge { goal }))
    },
    | "registrations" => Ok(write(WriteCommand::Registrations { project })),
    | "approve-registration" => {
      let project = project.ok_or_else(|| UsageError("approve-registration には --project が必要です。".to_owned()))?;
      let revision = parse_revision(revision)?;
      require_yes("approve-registration", yes)?;
      Ok(write(WriteCommand::ApproveRegistration { project, revision }))
    },
    | "reject-registration" => {
      let project = project.ok_or_else(|| UsageError("reject-registration には --project が必要です。".to_owned()))?;
      let revision = parse_revision(revision)?;
      require_yes("reject-registration", yes)?;
      Ok(write(WriteCommand::RejectRegistration { project, revision }))
    },
    | other => Err(UsageError(format!("未知のコマンドです: {other}"))),
  }
}

/// 破壊的な操作の確認フラグを要求する。
fn require_yes(command: &str, yes: bool) -> Result<(), UsageError> {
  if yes {
    Ok(())
  } else {
    Err(UsageError(format!("{command} は破壊的な操作です。実行するには --yes を付けてください。")))
  }
}

/// `--revision` の値を正の整数として取り出す。
fn parse_revision(value: Option<String>) -> Result<i64, UsageError> {
  let value = value.ok_or_else(|| UsageError("--revision には正の整数が必要です。".to_owned()))?;
  let revision =
    value.parse::<i64>().map_err(|_| UsageError(format!("--revision には正の整数を指定してください: {value}")))?;
  if revision <= 0 {
    return Err(UsageError(format!("--revision には正の整数を指定してください: {value}")));
  }
  Ok(revision)
}

/// `--addr` や `--config` の値を取り出す。
fn take_value(args: &[String], index: usize, flag: &str) -> Result<String, UsageError> {
  args
    .get(index + 1)
    .filter(|value| !value.is_empty())
    .cloned()
    .ok_or_else(|| UsageError(format!("{flag} には値が必要です。")))
}

/// 空文字を未指定として扱うフラグの値を取り出す。`--text` は本文だけで答える場合に省けるため、
/// 空文字を許して呼び出し側で未指定として扱います。
fn take_optional_empty_value(args: &[String], index: usize, flag: &str) -> Result<String, UsageError> {
  args.get(index + 1).cloned().ok_or_else(|| UsageError(format!("{flag} には値が必要です。")))
}

/// 使い方の説明。未知の引数・未知のコマンドのときに stderr へ出します。
fn usage() -> String {
  [
    "使い方: takt-dash-cli [overview] [--addr <待ち受けアドレス>] [--config <設定ファイル>]",
    "  overview                            backend の gRPC から概要を取得して表示します（既定）",
    "  answer --goal <ID> --question <ID> (--text <本文> | --report completed|unable)",
    "                                      問い合わせへ回答します",
    "  merge --goal <ID> --yes            プルリクエストのマージを依頼します",
    "  registrations [--project <名前>]    ゴール登録の承認待ちを一覧表示します",
    "  approve-registration --project <名前> --revision <n> --yes",
    "                                      ゴール登録を承認します",
    "  reject-registration --project <名前> --revision <n> --yes",
    "                                      ゴール登録を却下します",
    "--addr      接続先の待ち受けアドレス（例: 127.0.0.1:50051）",
    "--config    backend と同じ形式の設定ファイルのパス。listen を接続先に使います",
    "--yes       破壊的な操作（merge・approve-registration・reject-registration）に必要です",
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

/// backend へ gRPC で接続する。接続できないときは表示用のメッセージを返す。
async fn connect(addr: SocketAddr) -> Result<Channel, String> {
  let endpoint = Endpoint::from_shared(format!("http://{addr}"))
    .map_err(|error| format!("接続先 {addr} の指定が不正です: {error}"))?
    .connect_timeout(CONNECT_TIMEOUT)
    .timeout(CONNECT_TIMEOUT);
  endpoint.connect().await.map_err(|error| format!("接続先 {addr} に接続できません: {error}"))
}

/// backend の `GetOverview` で概要を取得する。
///
/// 接続成否の判定はこの関数だけが行います。`Endpoint::connect()` の失敗と、`GetOverview` が返す
/// `Status` を同じ分類（接続不能か、backend が応答したうえでの失敗か）へ写します。
async fn fetch_overview(addr: SocketAddr) -> Result<DashboardState, OverviewError> {
  let channel = connect(addr).await.map_err(OverviewError::Unavailable)?;
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

/// 書き込みコマンドの実行に失敗した理由。
#[derive(Debug)]
enum WriteError {
  /// backend に接続できない、または backend が gRPC として応答しない（`is_connection_failure`）。
  Unavailable(String),
  /// backend は応答したが、接続不能以外の理由で失敗した。
  Call(Status),
}

/// 書き込み RPC の失敗を、接続不能と backend の失敗に分類する。
fn write_call_error(addr: SocketAddr, operation: &str, status: Status) -> WriteError {
  if is_connection_failure(status.code()) {
    WriteError::Unavailable(format!("接続先 {addr} から{operation}に失敗しました: {}", status.message()))
  } else {
    WriteError::Call(status)
  }
}

/// 書き込みコマンドを 1 つ実行し、成功時の表示を返す。
async fn execute_write(addr: SocketAddr, command: &WriteCommand) -> Result<String, WriteError> {
  let channel = connect(addr).await.map_err(WriteError::Unavailable)?;
  let mut client = TaktDashServiceClient::new(channel);
  let operation = command.operation();
  match command {
    | WriteCommand::Answer { goal, question, text, report } => {
      let response = client
        .answer_inquiry(AnswerInquiryRequest {
          goal_id:    goal.clone(),
          inquiry_id: question.clone(),
          text:       text.clone(),
          report:     *report as i32,
        })
        .await
        .map_err(|status| write_call_error(addr, operation, status))?
        .into_inner();
      Ok(render_answer(&response))
    },
    | WriteCommand::Merge { goal } => {
      let response = client
        .approve_merge(ApproveMergeRequest { goal_id: goal.clone() })
        .await
        .map_err(|status| write_call_error(addr, operation, status))?
        .into_inner();
      Ok(render_merge(&response))
    },
    | WriteCommand::Registrations { project } => {
      let response = client
        .list_goal_registration_proposals(ListGoalRegistrationProposalsRequest {
          project: project.clone().unwrap_or_default(),
        })
        .await
        .map_err(|status| write_call_error(addr, operation, status))?
        .into_inner();
      Ok(render_registrations(&response.proposals))
    },
    | WriteCommand::ApproveRegistration { project, revision } => {
      let response = client
        .approve_goal_registration(ApproveGoalRegistrationRequest { project: project.clone(), revision: *revision })
        .await
        .map_err(|status| write_call_error(addr, operation, status))?
        .into_inner();
      Ok(render_approve_registration(&response))
    },
    | WriteCommand::RejectRegistration { project, revision } => {
      let response = client
        .reject_goal_registration(RejectGoalRegistrationRequest { project: project.clone(), revision: *revision })
        .await
        .map_err(|status| write_call_error(addr, operation, status))?
        .into_inner();
      Ok(render_reject_registration(&response))
    },
  }
}

/// `run_with` が実行する処理。
enum Action {
  /// 概要を表示する。
  Overview,
  /// 書き込みコマンドを実行する。
  Write(WriteCommand),
}

/// 概要を標準出力へ表示する。
///
/// 引数を解釈し、接続先を解決し、概要の取得または書き込みコマンドを実行して表示します。出力と
/// エラーは指定された writer へ書き、終了コードを返します。`run` とテストの両方がこの関数を
/// 通して観測します。
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
  let (addr_option, config_option, action) = match invocation {
    | Invocation::Help => {
      let _ = writeln!(stdout, "{}", usage());
      return EXIT_OK;
    },
    | Invocation::Overview(options) => (options.addr, options.config, Action::Overview),
    | Invocation::Write(write) => (write.addr, write.config, Action::Write(write.command)),
  };
  let config_path = config_option.unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG_PATH));
  let config_listen = match read_config_listen(&config_path) {
    | Ok(listen) => listen,
    | Err(message) => {
      let _ = writeln!(stderr, "{message}");
      return EXIT_FAILURE;
    },
  };
  let sources = AddrSources { cli_addr: addr_option.as_deref(), env_addr, config_listen: config_listen.as_deref() };
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
  match action {
    | Action::Overview => match runtime.block_on(fetch_overview(addr)) {
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
    },
    | Action::Write(command) => match runtime.block_on(execute_write(addr, &command)) {
      | Ok(text) => {
        let _ = writeln!(stdout, "{text}");
        EXIT_OK
      },
      | Err(WriteError::Unavailable(message)) => {
        let _ = writeln!(stderr, "backend に接続できません。{message}");
        EXIT_FAILURE
      },
      | Err(WriteError::Call(status)) => {
        let _ = writeln!(
          stderr,
          "backend から{}に失敗しました（{}）。{}",
          command.operation(),
          status.code(),
          status.message()
        );
        EXIT_FAILURE
      },
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
      // 回答・マージ依頼の対象を決められるよう、ゴールに属する判断待ちにはゴール ID を出す。
      let goal_id = decision
        .goal_id
        .as_deref()
        .filter(|goal_id| !goal_id.is_empty())
        .map(|goal_id| format!(" ゴールID: {goal_id}"))
        .unwrap_or_default();
      // 回答コマンドは問い合わせ ID を必要とするため、問い合わせの行だけに ID を出す。
      let inquiry_id = if decision.r#type == PendingDecisionType::Inquiry as i32 {
        decision
          .inquiry_id
          .as_deref()
          .filter(|inquiry_id| !inquiry_id.is_empty())
          .map(|inquiry_id| format!(" 問い合わせID: {inquiry_id}"))
          .unwrap_or_default()
      } else {
        String::new()
      };
      format!("- {}/{}: {} {}{}{}", decision.project, decision.server_id, kind, decision.body, goal_id, inquiry_id)
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

/// 問い合わせへの回答の結果を表示する。
fn render_answer(response: &AnswerInquiryResponse) -> String {
  format!("回答を記録しました。\nゴール: {}\n問い合わせ: {}", response.goal_id, response.inquiry_id)
}

/// マージの依頼の結果を表示する。
fn render_merge(response: &ApproveMergeResponse) -> String {
  format!(
    "マージを依頼しました。\nゴール: {}\nプルリクエスト: #{}\nタスク: {}",
    response.goal_id, response.pr_number, response.task_name
  )
}

/// ゴール登録の承認待ちの一覧を表示する。
fn render_registrations(proposals: &[GoalRegistrationProposal]) -> String {
  let lines: Vec<String> = proposals
    .iter()
    .map(|proposal| {
      format!(
        "- [{GOAL_REGISTRATION_LABEL}] {}（{}）リビジョン {}: {}",
        proposal.project, proposal.server_id, proposal.revision, proposal.objective
      )
    })
    .collect();
  format!("ゴール登録の承認待ち: {}件\n{}", proposals.len(), join_lines(&lines))
}

/// ゴール登録の承認の結果を表示する。
fn render_approve_registration(response: &ApproveGoalRegistrationResponse) -> String {
  format!(
    "ゴール登録を承認しました。\nリビジョン: {}\nゴール: {}「{}」",
    response.revision, response.goal_id, response.objective
  )
}

/// ゴール登録の却下の結果を表示する。
fn render_reject_registration(response: &RejectGoalRegistrationResponse) -> String {
  let mut text = format!("ゴール登録を却下しました。\nリビジョン: {}", response.revision);
  if !response.notice.trim().is_empty() {
    text.push('\n');
    text.push_str(&response.notice);
  }
  text
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
    is_connection_failure, parse_arguments, proto, read_config_listen, render_overview, resolve_backend_addr, run_with,
    AddrError, AddrSources, Invocation, OverviewOptions, UsageError, DEFAULT_BACKEND_ADDR, EXIT_FAILURE, EXIT_OK,
    EXIT_USAGE,
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

    async fn answer_inquiry(
      &self,
      _request: Request<v1::AnswerInquiryRequest>,
    ) -> Result<Response<v1::AnswerInquiryResponse>, Status> {
      self.record("AnswerInquiry");
      Err(Status::unimplemented("このダミーサーバーは AnswerInquiry を提供しません。"))
    }

    async fn approve_merge(
      &self,
      _request: Request<v1::ApproveMergeRequest>,
    ) -> Result<Response<v1::ApproveMergeResponse>, Status> {
      self.record("ApproveMerge");
      Err(Status::unimplemented("このダミーサーバーは ApproveMerge を提供しません。"))
    }

    async fn list_goal_registration_proposals(
      &self,
      _request: Request<v1::ListGoalRegistrationProposalsRequest>,
    ) -> Result<Response<v1::ListGoalRegistrationProposalsResponse>, Status> {
      self.record("ListGoalRegistrationProposals");
      Err(Status::unimplemented("このダミーサーバーは ListGoalRegistrationProposals を提供しません。"))
    }

    async fn approve_goal_registration(
      &self,
      _request: Request<v1::ApproveGoalRegistrationRequest>,
    ) -> Result<Response<v1::ApproveGoalRegistrationResponse>, Status> {
      self.record("ApproveGoalRegistration");
      Err(Status::unimplemented("このダミーサーバーは ApproveGoalRegistration を提供しません。"))
    }

    async fn reject_goal_registration(
      &self,
      _request: Request<v1::RejectGoalRegistrationRequest>,
    ) -> Result<Response<v1::RejectGoalRegistrationResponse>, Status> {
      self.record("RejectGoalRegistration");
      Err(Status::unimplemented("このダミーサーバーは RejectGoalRegistration を提供しません。"))
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
          inquiry_id:   Some("q-cli".to_owned()),
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
          inquiry_id:   None,
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
      inquiry_id:   None,
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
  // CLI-USAGE: 未知の引数の扱い（要件 9）
  // ----------------------------------------------------------------------------

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
    // 空きポートを確保してすぐ手放し、待ち受けの無い接続先を実行時に作る。固定ポートの空きに
    // 依存すると、ホストで稼働しているサービスとの衝突でテスト結果が変わってしまう。
    let closed = reserve_closed_addr();
    let config = write_config(&dir, &format!(r#"{{"listen":"{closed}"}}"#));

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
    assert!(stderr.contains(&closed.to_string()), "設定の listen を接続先に使う必要があります: {stderr}");
    assert!(!stderr.contains(DEFAULT_BACKEND_ADDR), "既定値へ戻ってはいけません: {stderr}");
  }

  #[test]
  fn missing_config_file_falls_through_to_default() {
    let dir = tempfile::tempdir().expect("一時ディレクトリを作れませんでした");
    let missing = dir.path().join("does-not-exist.json");

    // 設定ファイルが無いときは listen を提供せず、接続先の解決が既定値へ進むことを、実際の
    // 待ち受けに依存せず観測する。既定ポートで待ち受けが動いている環境でも結果が変わらない。
    let config_listen = read_config_listen(&missing).expect("存在しない設定ファイルは未設定として扱う必要があります");
    assert_eq!(config_listen, None, "存在しない設定ファイルは listen を提供してはいけません");

    let sources = AddrSources {
      cli_addr:      None,
      env_addr:      Err(std::env::VarError::NotPresent),
      config_listen: config_listen.as_deref(),
    };
    assert_eq!(
      resolve_backend_addr(&sources),
      Ok(DEFAULT_BACKEND_ADDR.parse().expect("既定の待ち受けアドレスは解釈できる必要があります")),
      "設定ファイルが無いときは既定値へ進む必要があります"
    );
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

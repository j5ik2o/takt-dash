//! taktDash のコマンドラインの入口。
//!
//! backend の gRPC へ接続して概要に相当する状態を日本語で表示する閲覧コマンドと、問い合わせへの
//! 回答・マージの承認・ゴール登録の承認と却下・承認待ちの一覧を行う書き込みコマンドを持ちます。

mod args;
mod command;
mod domain;
mod proto;
mod render;

use std::{env::VarError, io::Write, net::SocketAddr, path::PathBuf, time::Duration};

use args::{parse_arguments, usage, UsageError};
use command::{Answer, ConversationOptions, GoalOptions, GoalsOptions, Invocation, OverviewOptions, WriteCommand};
use domain::{ProjectName, ServerId};
use proto::v1::{
  takt_dash_service_client::TaktDashServiceClient, AnswerInquiryRequest, ApproveGoalMergeRequest,
  ApproveGoalRegistrationRequest, ConversationSkipped, DashboardState, GetConversationRequest, GetConversationResponse,
  GetGoalDetailRequest, GetGoalDetailResponse, GetGoalsRequest, GetGoalsResponse, GetOverviewRequest,
  GetPendingDecisionsRequest, RejectGoalRegistrationRequest,
};
use render::{
  render_answer, render_approve_registration, render_conversation, render_decisions, render_goal_detail, render_goals,
  render_merge, render_overview, render_registrations, render_reject_registration,
};
use tonic::{transport::Endpoint, Code, Request, Status};

/// backend の gRPC の待ち受けアドレスの既定値。backend の設定の既定値と揃えます。
pub const DEFAULT_BACKEND_ADDR: &str = "127.0.0.1:50051";
/// 接続先を上書きする環境変数。frontend/web と同じ名前を使い、新しい設定を増やしません。
pub const BACKEND_ADDR_ENV: &str = "TAKTDASH_BACKEND_ADDR";
/// backend の設定ファイルの既定のファイル名。backend の bin と同じ名前を使います。
pub const DEFAULT_CONFIG_PATH: &str = "taktdash.config.json";
/// 接続の試行に許す時間。応答のないアドレスでいつまでも待たないためです。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// 読み取り RPC に許す時間。書き込みの期限とは分け、既存の待ち時間を変えません。
const READ_TIMEOUT: Duration = Duration::from_secs(5);
/// ゴール詳細 RPC に許す時間。GetGoalDetail は backend が takt-mcp を直列に呼ぶため、
/// 他の読み取りより長い期限を持たせます。
const GOAL_DETAIL_TIMEOUT: Duration = Duration::from_secs(60);
/// 書き込み RPC に許す時間。backend の初期化・道具実行・停止を見込みます。
const WRITE_TIMEOUT: Duration = Duration::from_secs(60);

/// 正常終了の終了コード。
const EXIT_OK: i32 = 0;
/// 接続・取得・引数のエラーで異常終了するときの終了コード。
const EXIT_FAILURE: i32 = 1;
/// 使い方の誤りで異常終了するときの終了コード。
const EXIT_USAGE: i32 = 2;

/// 会話の取得で一度に調べる行数の上限。
const CONVERSATION_LIMIT: i32 = 200;

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

/// 接続先の解決に失敗した理由。設定の読み込み失敗とアドレスの不正を分け、終了コードを写し分けます。
#[derive(Debug, PartialEq, Eq)]
enum AddrResolutionError {
  /// 設定ファイルを読み込めない、または JSON を解析できない。
  Config(String),
  /// 指定された値が待ち受けアドレスとして解釈できない。
  Invalid { source: &'static str, value: String },
}

/// 接続先を解決する。上位指定が無いときだけ設定ファイルを読みます。
///
/// `--addr` と環境変数の非空判定を先に行い、どちらかが非空なら設定ファイルを読まずに
/// `resolve_backend_addr` へ渡します。両方とも未設定（または空）のときだけ `--config` または
/// 既定パスの設定を読みます。不正な非空値は設定へフォールバックさせず失敗させます。
fn resolve_overview_addr(
  options: &OverviewOptions,
  env_addr: Result<String, VarError>,
) -> Result<SocketAddr, AddrResolutionError> {
  let cli_addr = options.addr.as_deref();
  let env_value = env_addr.as_ref().ok();
  let higher_priority_present =
    cli_addr.is_some_and(|value| !value.is_empty()) || env_value.is_some_and(|value| !value.is_empty());
  let config_listen = if higher_priority_present {
    None
  } else {
    let config_path = options.config.clone().unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG_PATH));
    read_config_listen(&config_path).map_err(AddrResolutionError::Config)?
  };
  let sources = AddrSources { cli_addr, env_addr, config_listen: config_listen.as_deref() };
  resolve_backend_addr(&sources)
    .map_err(|AddrError::Invalid { source, value }| AddrResolutionError::Invalid { source, value })
}

/// `--addr` ＞ 環境変数 ＞ 設定の `listen` ＞ 既定値の順で接続先を解決する。
///
/// `--addr` と環境変数のどちらかが非空なら設定ファイルは読まない。両方とも未設定（または空）の
/// ときだけ `--config` または既定パスの設定を読む。不正な非空値は設定へフォールバックさせない。
fn resolve_addr(
  addr: Option<&str>,
  config: Option<&PathBuf>,
  env_addr: Result<String, VarError>,
  stderr: &mut impl Write,
) -> Result<SocketAddr, i32> {
  let options = OverviewOptions { addr: addr.map(str::to_owned), config: config.cloned() };
  match resolve_overview_addr(&options, env_addr) {
    | Ok(addr) => Ok(addr),
    | Err(AddrResolutionError::Config(message)) => {
      let _ = writeln!(stderr, "{message}");
      Err(EXIT_FAILURE)
    },
    | Err(AddrResolutionError::Invalid { source, value }) => {
      let _ = writeln!(stderr, "{source} の値 {value} を待ち受けアドレスとして解釈できませんでした。");
      Err(EXIT_USAGE)
    },
  }
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

/// 読み取り系 API の取得に失敗した理由。
#[derive(Debug)]
enum ReadError {
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
async fn connect(addr: SocketAddr) -> Result<tonic::transport::Channel, String> {
  // Endpoint の timeout は RPC ごとの期限の上限になるため設定しません。RPC の期限は
  // Request::set_timeout で読み取りと書き込みに分けて指定します。
  let endpoint = Endpoint::from_shared(format!("http://{addr}"))
    .map_err(|error| format!("接続先 {addr} の指定が不正です: {error}"))?
    .connect_timeout(CONNECT_TIMEOUT);
  endpoint.connect().await.map_err(|error| format!("接続先 {addr} に接続できません: {error}"))
}

/// 読み取り RPC の失敗を、接続不能と backend の失敗に分類する。
fn read_call_error(addr: SocketAddr, operation: &str, status: Status) -> ReadError {
  if is_connection_failure(status.code()) {
    ReadError::Unavailable(format!("接続先 {addr} から{operation}に失敗しました: {}", status.message()))
  } else {
    ReadError::Call(status)
  }
}

/// backend の `GetOverview` で概要を取得する。
///
/// 接続成否の判定はこの関数だけが行います。`Endpoint::connect()` の失敗と、`GetOverview` が返す
/// `Status` を同じ分類（接続不能か、backend が応答したうえでの失敗か）へ写します。
async fn fetch_overview(addr: SocketAddr) -> Result<DashboardState, ReadError> {
  let channel = connect(addr).await.map_err(ReadError::Unavailable)?;
  let mut client = TaktDashServiceClient::new(channel);
  let mut request = Request::new(GetOverviewRequest {});
  request.set_timeout(READ_TIMEOUT);
  let response = client.get_overview(request).await.map_err(|status| read_call_error(addr, "概要の取得", status))?;
  Ok(response.into_inner())
}

/// backend の `GetGoals` でゴールの一覧を取得する。
async fn fetch_goals(addr: SocketAddr) -> Result<GetGoalsResponse, ReadError> {
  let channel = connect(addr).await.map_err(ReadError::Unavailable)?;
  let mut client = TaktDashServiceClient::new(channel);
  let mut request = Request::new(GetGoalsRequest {});
  request.set_timeout(READ_TIMEOUT);
  let response = client.get_goals(request).await.map_err(|status| read_call_error(addr, "ゴール一覧の取得", status))?;
  Ok(response.into_inner())
}

/// backend の `GetGoalDetail` でゴールの詳細を取得する。
async fn fetch_goal_detail(addr: SocketAddr, goal_id: &str) -> Result<GetGoalDetailResponse, ReadError> {
  let channel = connect(addr).await.map_err(ReadError::Unavailable)?;
  let mut client = TaktDashServiceClient::new(channel);
  let mut request = Request::new(GetGoalDetailRequest { goal_id: goal_id.to_owned() });
  request.set_timeout(GOAL_DETAIL_TIMEOUT);
  let response =
    client.get_goal_detail(request).await.map_err(|status| read_call_error(addr, "ゴールの取得", status))?;
  Ok(response.into_inner())
}

/// backend の `GetPendingDecisions` で判断待ちの状態を取得する。
async fn fetch_decisions(addr: SocketAddr) -> Result<DashboardState, ReadError> {
  let channel = connect(addr).await.map_err(ReadError::Unavailable)?;
  let mut client = TaktDashServiceClient::new(channel);
  let mut request = Request::new(GetPendingDecisionsRequest {});
  request.set_timeout(READ_TIMEOUT);
  let response =
    client.get_pending_decisions(request).await.map_err(|status| read_call_error(addr, "判断待ちの取得", status))?;
  response
    .into_inner()
    .state
    .ok_or_else(|| ReadError::Call(Status::internal("backend が判断待ちの状態を返しませんでした。")))
}

/// backend の `GetConversation` で会話を取得する。
///
/// `has_more` の間は応答の `next_after_seq` を次の `after_seq` にして続きを取り、最初の応答の
/// `session_id` を固定します。カーソルが進まない応答では、同じページを取り続けないよう止めます。
async fn fetch_conversation(
  addr: SocketAddr,
  project: &ProjectName,
  session: Option<&str>,
) -> Result<GetConversationResponse, ReadError> {
  let channel = connect(addr).await.map_err(ReadError::Unavailable)?;
  let mut client = TaktDashServiceClient::new(channel);

  let mut after_seq = 0_i64;
  let mut session_id = session.map(str::to_owned);
  let mut merged: Option<GetConversationResponse> = None;
  loop {
    let request = GetConversationRequest {
      project: project.as_str().to_owned(),
      session_id: session_id.clone(),
      after_seq,
      limit: CONVERSATION_LIMIT,
    };
    let mut request = Request::new(request);
    request.set_timeout(READ_TIMEOUT);
    let page = client.get_conversation(request).await.map_err(|status| read_call_error(addr, "会話の取得", status))?;
    let page = page.into_inner();
    if session_id.is_none() {
      session_id = Some(page.session_id.clone());
    }
    let has_more = page.has_more;
    let next_after_seq = page.next_after_seq;
    merged = Some(match merged {
      | None => page,
      | Some(accumulated) => merge_conversation_pages(accumulated, page),
    });
    if !has_more || next_after_seq <= after_seq {
      break;
    }
    after_seq = next_after_seq;
  }
  Ok(merged.expect("会話の取得は少なくとも1回成功します"))
}

/// 差分のページを、既に取得した会話へ足す。
///
/// 同じ seq は重複させず、skipped は理由ごとに合算します。会話の識別は最初の応答の値を保ち、
/// 続きの位置と未読の有無は新しいページの値を使います。
fn merge_conversation_pages(
  mut base: GetConversationResponse,
  next: GetConversationResponse,
) -> GetConversationResponse {
  base.messages.extend(next.messages);
  base.messages.sort_by_key(|message| message.seq);
  base.messages.dedup_by_key(|message| message.seq);

  let mut skipped: Vec<ConversationSkipped> = Vec::new();
  for item in base.skipped.into_iter().chain(next.skipped) {
    if let Some(existing) = skipped.iter_mut().find(|existing| existing.reason == item.reason) {
      existing.count += item.count;
    } else {
      skipped.push(item);
    }
  }
  base.skipped = skipped;
  base.next_after_seq = next.next_after_seq;
  base.has_more = next.has_more;
  if next.unavailable_reason.is_some() {
    base.unavailable_reason = next.unavailable_reason;
  }
  base
}

/// 書き込みコマンドの実行に失敗した理由。
#[derive(Debug)]
enum WriteError {
  /// backend に接続できない、または backend が gRPC として応答しない（`is_connection_failure`）。
  Unavailable(String),
  /// backend は応答したが、接続不能以外の理由で失敗した。
  Call(Status),
  /// backend の応答に、検証済みの型として扱えない値が含まれていた。
  Invalid(domain::DomainError),
}

/// 書き込み RPC の失敗を、接続不能と backend の失敗に分類する。
fn write_call_error(addr: SocketAddr, operation: &str, status: Status) -> WriteError {
  if is_connection_failure(status.code()) {
    WriteError::Unavailable(format!("接続先 {addr} から{operation}に失敗しました: {}", status.message()))
  } else {
    WriteError::Call(status)
  }
}

/// 書き込みコマンドを1つ実行し、成功時の表示を返す。
async fn execute_write(addr: SocketAddr, command: &WriteCommand) -> Result<String, WriteError> {
  let channel = connect(addr).await.map_err(WriteError::Unavailable)?;
  let mut client = TaktDashServiceClient::new(channel);
  let operation = command.operation();
  match command {
    | WriteCommand::Answer { server_id, project, goal_id, inquiry_id, answer } => {
      let (text, report) = match answer {
        | Answer::Text(text) => (Some(text.clone()), None),
        | Answer::Report(report) => (None, Some(*report as i32)),
      };
      let mut request = Request::new(AnswerInquiryRequest {
        idempotency_key: None,
        server_id: server_id.as_str().to_owned(),
        project: project.as_str().to_owned(),
        goal_id: goal_id.as_ref().map(|goal_id| goal_id.as_str().to_owned()),
        inquiry_id: inquiry_id.as_str().to_owned(),
        text,
        report,
      });
      request.set_timeout(WRITE_TIMEOUT);
      let response =
        client.answer_inquiry(request).await.map_err(|status| write_call_error(addr, operation, status))?.into_inner();
      render_answer(&response).map_err(WriteError::Invalid)
    },
    | WriteCommand::Merge { server_id, project, goal_id, pr_number, until } => {
      let mut request = Request::new(ApproveGoalMergeRequest {
        idempotency_key: None,
        server_id:       server_id.as_str().to_owned(),
        project:         project.as_str().to_owned(),
        goal_id:         goal_id.as_str().to_owned(),
        pr_number:       pr_number.get(),
        until:           until.as_str().to_owned(),
      });
      request.set_timeout(WRITE_TIMEOUT);
      let response = client
        .approve_goal_merge(request)
        .await
        .map_err(|status| write_call_error(addr, operation, status))?
        .into_inner();
      Ok(render_merge(&response))
    },
    | WriteCommand::Registrations { server_id, project } => {
      let mut request = Request::new(proto::v1::ListGoalRegistrationProposalsRequest {
        server_id: server_id.as_str().to_owned(),
        project:   project.as_str().to_owned(),
      });
      request.set_timeout(READ_TIMEOUT);
      let response = client
        .list_goal_registration_proposals(request)
        .await
        .map_err(|status| write_call_error(addr, operation, status))?
        .into_inner();
      render_registrations(&response.proposals).map_err(WriteError::Invalid)
    },
    | WriteCommand::ApproveRegistration { server_id, project, revision } => {
      let mut request = Request::new(ApproveGoalRegistrationRequest {
        idempotency_key: None,
        server_id:       server_id.as_str().to_owned(),
        project:         project.as_str().to_owned(),
        revision:        revision.get(),
      });
      request.set_timeout(WRITE_TIMEOUT);
      let response = client
        .approve_goal_registration(request)
        .await
        .map_err(|status| write_call_error(addr, operation, status))?
        .into_inner();
      render_approve_registration(&response).map_err(WriteError::Invalid)
    },
    | WriteCommand::RejectRegistration { server_id, project, revision } => {
      let mut request = Request::new(RejectGoalRegistrationRequest {
        idempotency_key: None,
        server_id:       server_id.as_str().to_owned(),
        project:         project.as_str().to_owned(),
        revision:        revision.get(),
      });
      request.set_timeout(WRITE_TIMEOUT);
      let response = client
        .reject_goal_registration(request)
        .await
        .map_err(|status| write_call_error(addr, operation, status))?
        .into_inner();
      render_reject_registration(&response).map_err(WriteError::Invalid)
    },
  }
}

/// 引数を解釈し、接続先を解決し、概要・会話の表示または書き込みコマンドを実行する。
///
/// 出力とエラーは指定された writer へ書き、終了コードを返します。`run` とテストの両方がこの
/// 関数を通して観測します。
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
    | Invocation::Overview(options) => return run_overview(&options, env_addr, stdout, stderr),
    | Invocation::Decisions(options) => {
      (options.addr, options.config, Action::Decisions { server_id: options.server_id, project: options.project })
    },
    | Invocation::Write(write) => (write.addr, write.config, Action::Write(write.command)),
    | Invocation::Conversation(options) => return run_conversation(&options, env_addr, stdout, stderr),
    | Invocation::Goals(options) => return run_goals(&options, env_addr, stdout, stderr),
    | Invocation::Goal(options) => return run_goal(&options, env_addr, stdout, stderr),
  };
  let addr = match resolve_addr(addr_option.as_deref(), config_option.as_ref(), env_addr, stderr) {
    | Ok(addr) => addr,
    | Err(code) => return code,
  };
  let runtime = match build_runtime(stderr) {
    | Ok(runtime) => runtime,
    | Err(code) => return code,
  };
  match action {
    | Action::Decisions { server_id, project } => match runtime.block_on(fetch_decisions(addr)) {
      | Ok(state) => match render_decisions(&state, &server_id, &project) {
        | Ok(text) => {
          let _ = writeln!(stdout, "{text}");
          EXIT_OK
        },
        | Err(error) => report_invalid_response(stderr, error),
      },
      | Err(ReadError::Unavailable(message)) => {
        let _ = writeln!(stderr, "backend に接続できません。{message}");
        EXIT_FAILURE
      },
      | Err(ReadError::Call(status)) => {
        let _ =
          writeln!(stderr, "backend から判断待ちを取得できませんでした（{}）。{}", status.code(), status.message());
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
      | Err(WriteError::Invalid(error)) => report_invalid_response(stderr, error),
    },
  }
}

/// backend の応答に検証済みの型として扱えない値があったことを表示し、失敗の終了コードを返す。
fn report_invalid_response(stderr: &mut impl Write, error: domain::DomainError) -> i32 {
  let _ = writeln!(stderr, "backend の応答に不正な値があります: {error}");
  EXIT_FAILURE
}

/// 概要の表示を実行する。
fn run_overview(
  options: &OverviewOptions,
  env_addr: Result<String, VarError>,
  stdout: &mut impl Write,
  stderr: &mut impl Write,
) -> i32 {
  let addr = match resolve_addr(options.addr.as_deref(), options.config.as_ref(), env_addr, stderr) {
    | Ok(addr) => addr,
    | Err(code) => return code,
  };
  let runtime = match build_runtime(stderr) {
    | Ok(runtime) => runtime,
    | Err(code) => return code,
  };
  match runtime.block_on(fetch_overview(addr)) {
    | Ok(state) => match render_overview(&state) {
      | Ok(text) => {
        let _ = writeln!(stdout, "{text}");
        EXIT_OK
      },
      | Err(error) => report_invalid_response(stderr, error),
    },
    | Err(ReadError::Unavailable(message)) => {
      let _ = writeln!(stderr, "backend に接続できません。{message}");
      EXIT_FAILURE
    },
    | Err(ReadError::Call(status)) => {
      let _ = writeln!(stderr, "backend から概要を取得できませんでした（{}）。{}", status.code(), status.message());
      EXIT_FAILURE
    },
  }
}

/// 会話の表示を実行する。
fn run_conversation(
  options: &ConversationOptions,
  env_addr: Result<String, VarError>,
  stdout: &mut impl Write,
  stderr: &mut impl Write,
) -> i32 {
  let addr = match resolve_addr(options.addr.as_deref(), options.config.as_ref(), env_addr, stderr) {
    | Ok(addr) => addr,
    | Err(code) => return code,
  };
  let runtime = match build_runtime(stderr) {
    | Ok(runtime) => runtime,
    | Err(code) => return code,
  };
  match runtime.block_on(fetch_conversation(addr, &options.project, options.session.as_deref())) {
    | Ok(response) => match render_conversation(&response) {
      | Ok(text) => {
        let _ = writeln!(stdout, "{text}");
        EXIT_OK
      },
      | Err(error) => report_invalid_response(stderr, error),
    },
    | Err(ReadError::Unavailable(message)) => {
      let _ = writeln!(stderr, "backend に接続できません。{message}");
      EXIT_FAILURE
    },
    | Err(ReadError::Call(status)) => {
      let _ = writeln!(stderr, "backend から会話を取得できませんでした（{}）。{}", status.code(), status.message());
      EXIT_FAILURE
    },
  }
}

/// ゴール一覧の表示を実行する。
fn run_goals(
  options: &GoalsOptions,
  env_addr: Result<String, VarError>,
  stdout: &mut impl Write,
  stderr: &mut impl Write,
) -> i32 {
  let addr = match resolve_addr(options.addr.as_deref(), options.config.as_ref(), env_addr, stderr) {
    | Ok(addr) => addr,
    | Err(code) => return code,
  };
  let runtime = match build_runtime(stderr) {
    | Ok(runtime) => runtime,
    | Err(code) => return code,
  };
  match runtime.block_on(fetch_goals(addr)) {
    | Ok(response) => {
      let _ = writeln!(stdout, "{}", render_goals(&response));
      EXIT_OK
    },
    | Err(ReadError::Unavailable(message)) => {
      let _ = writeln!(stderr, "backend に接続できません。{message}");
      EXIT_FAILURE
    },
    | Err(ReadError::Call(status)) => {
      let _ =
        writeln!(stderr, "backend からゴール一覧を取得できませんでした（{}）。{}", status.code(), status.message());
      EXIT_FAILURE
    },
  }
}

/// ゴール詳細の表示を実行する。
fn run_goal(
  options: &GoalOptions,
  env_addr: Result<String, VarError>,
  stdout: &mut impl Write,
  stderr: &mut impl Write,
) -> i32 {
  let addr = match resolve_addr(options.addr.as_deref(), options.config.as_ref(), env_addr, stderr) {
    | Ok(addr) => addr,
    | Err(code) => return code,
  };
  let runtime = match build_runtime(stderr) {
    | Ok(runtime) => runtime,
    | Err(code) => return code,
  };
  match runtime.block_on(fetch_goal_detail(addr, &options.goal_id)) {
    | Ok(detail) => {
      let _ = writeln!(stdout, "{}", render_goal_detail(&detail));
      EXIT_OK
    },
    | Err(ReadError::Unavailable(message)) => {
      let _ = writeln!(stderr, "backend に接続できません。{message}");
      EXIT_FAILURE
    },
    | Err(ReadError::Call(status)) if status.code() == Code::NotFound => {
      let _ = writeln!(stderr, "ゴールが見つかりません。{}", status.message());
      EXIT_FAILURE
    },
    | Err(ReadError::Call(status)) => {
      let _ = writeln!(stderr, "backend からゴールを取得できませんでした（{}）。{}", status.code(), status.message());
      EXIT_FAILURE
    },
  }
}

/// 非同期の処理を実行するための実行環境を準備する。
fn build_runtime(stderr: &mut impl Write) -> Result<tokio::runtime::Runtime, i32> {
  tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|error| {
    let _ = writeln!(stderr, "実行環境の準備に失敗しました: {error}");
    EXIT_FAILURE
  })
}

/// `run_with` が実行する処理。
enum Action {
  /// 判断待ちの操作対象を表示する。
  Decisions { server_id: ServerId, project: ProjectName },
  /// 書き込みコマンドを実行する。
  Write(WriteCommand),
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

fn main() {
  let args: Vec<String> = std::env::args().skip(1).collect();
  std::process::exit(run(&args));
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod write_command_tests;

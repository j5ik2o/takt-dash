//! taktDash のコマンドラインの入口。
//!
//! backend の gRPC へ接続して概要に相当する状態を日本語で表示する閲覧コマンドと、問い合わせへの
//! 回答・マージの承認・ゴール登録の承認と却下・承認待ちの一覧を行う書き込みコマンドを持ちます。

mod proto;

use std::{env::VarError, io::Write, net::SocketAddr, path::PathBuf, time::Duration};

use proto::v1::{
  takt_dash_service_client::TaktDashServiceClient, AnswerInquiryRequest, AnswerInquiryResponse,
  ApproveGoalMergeRequest, ApproveGoalMergeResponse, ApproveGoalRegistrationRequest, ApproveGoalRegistrationResponse,
  ConversationSkipped, DashboardState, ExecutionStatus, GetConversationRequest, GetConversationResponse,
  GetGoalDetailRequest, GetGoalDetailResponse, GetGoalsRequest, GetGoalsResponse, GetOverviewRequest,
  GetPendingDecisionsRequest, Goal, GoalQuestion, GoalRegistrationProposal, GoalStatus, OperationAnswerReport,
  PendingDecisionType, RejectGoalRegistrationRequest, RejectGoalRegistrationResponse, TaskStatus,
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
/// 書き込み RPC に許す時間。backend の初期化・道具実行・停止を見込みます。
const WRITE_TIMEOUT: Duration = Duration::from_secs(60);

/// 正常終了の終了コード。
const EXIT_OK: i32 = 0;
/// 接続・取得・引数のエラーで異常終了するときの終了コード。
const EXIT_FAILURE: i32 = 1;
/// 使い方の誤りで異常終了するときの終了コード。
const EXIT_USAGE: i32 = 2;

/// 閲覧のみを持つコマンドの名前。位置引数の既定値でもあります。
const OVERVIEW_COMMAND: &str = "overview";

/// manager の会話を閲覧するコマンドの名前。
const CONVERSATION_COMMAND: &str = "conversation";
/// 会話の取得で一度に調べる行数の上限。
const CONVERSATION_LIMIT: i32 = 200;

/// ゴールの一覧を表示するコマンドの名前。
const GOALS_COMMAND: &str = "goals";
/// ゴールの詳細を表示するコマンドの名前。
const GOAL_COMMAND: &str = "goal";

/// 問い合わせへ回答するコマンド。
const ANSWER_COMMAND: &str = "answer";
/// プルリクエストのマージを承認するコマンド。
const MERGE_COMMAND: &str = "merge";
/// ゴール登録の承認待ちを一覧表示するコマンド。
const REGISTRATIONS_COMMAND: &str = "registrations";
/// 判断待ちの操作対象を一覧表示するコマンド。
const DECISIONS_COMMAND: &str = "decisions";
/// ゴール登録を承認するコマンド。
const APPROVE_REGISTRATION_COMMAND: &str = "approve-registration";
/// ゴール登録を却下するコマンド。
const REJECT_REGISTRATION_COMMAND: &str = "reject-registration";
/// マージ承認で使う `until` の値（準備まで）。
const UNTIL_PREPARE: &str = "prepare";
/// マージ承認で使う `until` の値（マージまで）。
const UNTIL_MERGE: &str = "merge";

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

/// 引数を解釈した結果。
#[derive(Debug, PartialEq, Eq)]
enum Invocation {
  /// 概要を表示する。
  Overview(OverviewOptions),
  /// 判断待ちの操作対象を表示する。
  Decisions(DecisionsOptions),
  /// 書き込み操作を実行する。
  Write(WriteInvocation),
  /// manager の会話を表示する。
  Conversation(ConversationOptions),
  /// ゴールの一覧を表示する。
  Goals(GoalsOptions),
  /// ゴールの詳細を表示する。
  Goal(GoalOptions),
  /// 使い方を表示して正常終了する。
  Help,
}

/// 判断待ちの一覧の表示に使う解決前の入力。
#[derive(Debug, Default, PartialEq, Eq)]
struct DecisionsOptions {
  server_id: String,
  project:   String,
  addr:      Option<String>,
  config:    Option<PathBuf>,
}

/// 概要の表示に使う解決前の入力。
#[derive(Debug, Default, PartialEq, Eq)]
struct OverviewOptions {
  addr:   Option<String>,
  config: Option<PathBuf>,
}

/// 会話の表示に使う解決前の入力。
#[derive(Debug, Default, PartialEq, Eq)]
struct ConversationOptions {
  project: String,
  session: Option<String>,
  addr:    Option<String>,
  config:  Option<PathBuf>,
}

/// ゴール一覧の表示に使う解決前の入力。
#[derive(Debug, Default, PartialEq, Eq)]
struct GoalsOptions {
  addr:   Option<String>,
  config: Option<PathBuf>,
}

/// ゴール詳細の表示に使う解決前の入力。
#[derive(Debug, Default, PartialEq, Eq)]
struct GoalOptions {
  goal_id: String,
  addr:    Option<String>,
  config:  Option<PathBuf>,
}

/// 書き込みコマンドと、その接続先の解決前の入力。
#[derive(Debug, PartialEq, Eq)]
struct WriteInvocation {
  command: WriteCommand,
  addr:    Option<String>,
  config:  Option<PathBuf>,
}

/// 書き込みコマンド。引数は解釈時に検証済みです。
#[derive(Debug, PartialEq, Eq)]
enum WriteCommand {
  /// 問い合わせへ回答する。`text` か `report` のどちらか一方を指定します。
  Answer { server_id: String, project: String, goal_id: Option<String>, inquiry_id: String, answer: Answer },
  /// プルリクエストのマージを承認する。
  Merge { server_id: String, project: String, goal_id: String, pr_number: i64, until: String },
  /// ゴール登録の承認待ちを一覧表示する。
  Registrations { server_id: String, project: String },
  /// ゴール登録を承認する。
  ApproveRegistration { server_id: String, project: String, revision: i32 },
  /// ゴール登録を却下する。
  RejectRegistration { server_id: String, project: String, revision: i32 },
}

/// 問い合わせへの回答の種類。質問は自由記述（`text`）、作業の依頼は実施結果（`report`）を使います。
#[derive(Debug, PartialEq, Eq)]
enum Answer {
  Text(String),
  Report(OperationAnswerReport),
}

impl WriteCommand {
  /// 失敗の表示に使う操作名。
  fn operation(&self) -> &'static str {
    match self {
      | WriteCommand::Answer { .. } => "回答の記録",
      | WriteCommand::Merge { .. } => "マージの承認",
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
    OVERVIEW_COMMAND
      | ANSWER_COMMAND
      | MERGE_COMMAND
      | REGISTRATIONS_COMMAND
      | DECISIONS_COMMAND
      | APPROVE_REGISTRATION_COMMAND
      | REJECT_REGISTRATION_COMMAND
      | CONVERSATION_COMMAND
      | GOALS_COMMAND
      | GOAL_COMMAND
  )
}

/// そのコマンドで使えるフラグかどうか。`command` が未確定のときは共通のフラグだけを許します。
fn flag_allowed(command: Option<&str>, flag: &str) -> bool {
  match flag {
    | "--addr" | "--config" => true,
    | "--server" | "--project" => {
      matches!(
        command,
        Some(ANSWER_COMMAND)
          | Some(MERGE_COMMAND)
          | Some(REGISTRATIONS_COMMAND)
          | Some(DECISIONS_COMMAND)
          | Some(APPROVE_REGISTRATION_COMMAND)
          | Some(REJECT_REGISTRATION_COMMAND)
      )
    },
    | "--session" => command.is_none() || matches!(command, Some(OVERVIEW_COMMAND) | Some(CONVERSATION_COMMAND)),
    | "--goal" => matches!(command, Some(ANSWER_COMMAND) | Some(MERGE_COMMAND) | Some(GOAL_COMMAND)),
    | "--question" | "--text" | "--report" => command == Some(ANSWER_COMMAND),
    | "--pr" | "--until" => command == Some(MERGE_COMMAND),
    | "--revision" => matches!(command, Some(APPROVE_REGISTRATION_COMMAND) | Some(REJECT_REGISTRATION_COMMAND)),
    | _ => false,
  }
}

/// コマンドラインの引数を解釈する。
fn parse_arguments(args: &[String]) -> Result<Invocation, UsageError> {
  let mut addr = None;
  let mut config = None;
  let mut server = None;
  let mut project = None;
  let mut goal = None;
  let mut question = None;
  let mut text = None;
  let mut report = None;
  let mut pr = None;
  let mut until = None;
  let mut revision = None;
  let mut session = None;
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
        | "--server" => {
          server = Some(take_value(args, index, "--server")?);
          index += 2;
        },
        | "--project" => {
          project = Some(take_value(args, index, "--project")?);
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
          text = Some(take_value(args, index, "--text")?);
          index += 2;
        },
        | "--report" => {
          report = Some(take_value(args, index, "--report")?);
          index += 2;
        },
        | "--pr" => {
          pr = Some(take_value(args, index, "--pr")?);
          index += 2;
        },
        | "--until" => {
          until = Some(take_value(args, index, "--until")?);
          index += 2;
        },
        | "--revision" => {
          revision = Some(take_value(args, index, "--revision")?);
          index += 2;
        },
        | "--session" => {
          if command == Some(OVERVIEW_COMMAND) {
            return Err(UsageError("概要の表示に --session は使えません。".to_owned()));
          }
          session = Some(take_value(args, index, "--session")?);
          index += 2;
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
    if command == Some(CONVERSATION_COMMAND) {
      if project.is_some() {
        return Err(UsageError(format!("会話の表示に位置引数は1つだけ取れます: {argument}")));
      }
      project = Some(argument.to_owned());
      index += 1;
      continue;
    }
    return Err(UsageError(format!("{} に位置引数は取れません: {argument}", command.unwrap_or(OVERVIEW_COMMAND))));
  }
  let command = command.unwrap_or(OVERVIEW_COMMAND);
  if command == OVERVIEW_COMMAND {
    if session.is_some() {
      return Err(UsageError("概要の表示に --session は使えません。".to_owned()));
    }
    return Ok(Invocation::Overview(OverviewOptions { addr, config }));
  }
  if command == CONVERSATION_COMMAND {
    let project = project.ok_or_else(|| UsageError("会話の表示にはプロジェクト名が必要です。".to_owned()))?;
    return Ok(Invocation::Conversation(ConversationOptions { project, session, addr, config }));
  }
  if command == GOALS_COMMAND {
    return Ok(Invocation::Goals(GoalsOptions { addr, config }));
  }
  if command == GOAL_COMMAND {
    let goal_id = require_flag(goal, GOAL_COMMAND, "--goal")?;
    return Ok(Invocation::Goal(GoalOptions { goal_id, addr, config }));
  }
  if command == DECISIONS_COMMAND {
    let server_id = require_flag(server, DECISIONS_COMMAND, "--server")?;
    let project = require_flag(project, DECISIONS_COMMAND, "--project")?;
    return Ok(Invocation::Decisions(DecisionsOptions { server_id, project, addr, config }));
  }
  let write = |command: WriteCommand| Invocation::Write(WriteInvocation { command, addr, config });
  match command {
    | ANSWER_COMMAND => {
      let server_id = require_flag(server, ANSWER_COMMAND, "--server")?;
      let project = require_flag(project, ANSWER_COMMAND, "--project")?;
      let inquiry_id = require_flag(question, ANSWER_COMMAND, "--question")?;
      let answer = parse_answer(text, report)?;
      Ok(write(WriteCommand::Answer { server_id, project, goal_id: goal, inquiry_id, answer }))
    },
    | MERGE_COMMAND => {
      let server_id = require_flag(server, MERGE_COMMAND, "--server")?;
      let project = require_flag(project, MERGE_COMMAND, "--project")?;
      let goal_id = require_flag(goal, MERGE_COMMAND, "--goal")?;
      let pr_number = parse_pr_number(pr)?;
      let until = parse_until(until)?;
      Ok(write(WriteCommand::Merge { server_id, project, goal_id, pr_number, until }))
    },
    | REGISTRATIONS_COMMAND => {
      let server_id = require_flag(server, REGISTRATIONS_COMMAND, "--server")?;
      let project = require_flag(project, REGISTRATIONS_COMMAND, "--project")?;
      Ok(write(WriteCommand::Registrations { server_id, project }))
    },
    | APPROVE_REGISTRATION_COMMAND => {
      let server_id = require_flag(server, APPROVE_REGISTRATION_COMMAND, "--server")?;
      let project = require_flag(project, APPROVE_REGISTRATION_COMMAND, "--project")?;
      let revision = parse_revision(revision)?;
      Ok(write(WriteCommand::ApproveRegistration { server_id, project, revision }))
    },
    | REJECT_REGISTRATION_COMMAND => {
      let server_id = require_flag(server, REJECT_REGISTRATION_COMMAND, "--server")?;
      let project = require_flag(project, REJECT_REGISTRATION_COMMAND, "--project")?;
      let revision = parse_revision(revision)?;
      Ok(write(WriteCommand::RejectRegistration { server_id, project, revision }))
    },
    | other => Err(UsageError(format!("未知のコマンドです: {other}"))),
  }
}

/// 必須のフラグの値を取り出す。
fn require_flag(value: Option<String>, command: &str, flag: &str) -> Result<String, UsageError> {
  value.ok_or_else(|| UsageError(format!("{command} には {flag} が必要です。")))
}

/// 回答の種類を、`--text` と `--report` から決める。両方の同時指定と両方の欠落は誤りです。
fn parse_answer(text: Option<String>, report: Option<String>) -> Result<Answer, UsageError> {
  match (text, report) {
    | (Some(_), Some(_)) => {
      Err(UsageError("--text と --report は同時に指定できません。どちらか一方だけを指定してください。".to_owned()))
    },
    | (Some(text), None) => Ok(Answer::Text(text)),
    | (None, Some(report)) => {
      let report = match report.as_str() {
        | "completed" => OperationAnswerReport::Completed,
        | "unable" => OperationAnswerReport::Unable,
        | other => {
          return Err(UsageError(format!("--report には completed か unable を指定してください: {other}")));
        },
      };
      Ok(Answer::Report(report))
    },
    | (None, None) => Err(UsageError("answer には --text か --report が必要です。".to_owned())),
  }
}

/// `--pr` の値を正の整数として取り出す。
fn parse_pr_number(value: Option<String>) -> Result<i64, UsageError> {
  let value = value.ok_or_else(|| UsageError("merge には --pr が必要です。".to_owned()))?;
  let pr_number =
    value.parse::<i64>().map_err(|_| UsageError(format!("--pr には正の整数を指定してください: {value}")))?;
  if pr_number <= 0 {
    return Err(UsageError(format!("--pr には正の整数を指定してください: {value}")));
  }
  Ok(pr_number)
}

/// `--until` の値を、`prepare` か `merge` に限定して取り出す。
fn parse_until(value: Option<String>) -> Result<String, UsageError> {
  match value.as_deref() {
    | Some(UNTIL_PREPARE) => Ok(UNTIL_PREPARE.to_owned()),
    | Some(UNTIL_MERGE) => Ok(UNTIL_MERGE.to_owned()),
    | Some(other) => Err(UsageError(format!("--until には prepare か merge を指定してください: {other}"))),
    | None => Err(UsageError("merge には --until が必要です。".to_owned())),
  }
}

/// `--revision` の値を正の整数として取り出す。
fn parse_revision(value: Option<String>) -> Result<i32, UsageError> {
  let value = value.ok_or_else(|| UsageError("--revision には正の整数が必要です。".to_owned()))?;
  let revision =
    value.parse::<i32>().map_err(|_| UsageError(format!("--revision には正の整数を指定してください: {value}")))?;
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

/// 使い方の説明。未知の引数・未知のコマンドのときに stderr へ出します。
fn usage() -> String {
  [
    "使い方: takt-dash-cli [overview] [--addr <待ち受けアドレス>] [--config <設定ファイル>]",
    "  overview                            backend の gRPC から概要を取得して表示します（既定）",
    "  conversation <プロジェクト> [--session <セッション>]",
    "                                      manager の会話を発言の時系列で表示します",
    "  goals                               backend の gRPC からゴールの一覧を取得して表示します",
    "  goal --goal <ID>                    backend の gRPC からゴールの詳細を取得して表示します",
    "  answer --server <名前> --project <名前> --question <ID> (--text <本文> | --report completed|unable)",
    "                                      問い合わせへ回答します（--goal でゴールを明示できます）",
    "  merge --server <名前> --project <名前> --goal <ID> --pr <番号> --until prepare|merge",
    "                                      プルリクエストのマージを承認します",
    "  registrations --server <名前> --project <名前>",
    "                                      ゴール登録の承認待ちを一覧表示します",
    "  decisions --server <名前> --project <名前>",
    "                                      判断待ちの操作対象と識別子を一覧表示します",
    "  approve-registration --server <名前> --project <名前> --revision <n>",
    "                                      ゴール登録を承認します",
    "  reject-registration --server <名前> --project <名前> --revision <n>",
    "                                      ゴール登録を却下します",
    "--addr      接続先の待ち受けアドレス（例: 127.0.0.1:50051）",
    "--config    backend と同じ形式の設定ファイルのパス。listen を接続先に使います",
    "--session   会話のセッション。省略した場合は最新の manager のセッションを使います",
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
  request.set_timeout(READ_TIMEOUT);
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
  project: &str,
  session: Option<&str>,
) -> Result<GetConversationResponse, ReadError> {
  let channel = connect(addr).await.map_err(ReadError::Unavailable)?;
  let mut client = TaktDashServiceClient::new(channel);

  let mut after_seq = 0_i64;
  let mut session_id = session.map(str::to_owned);
  let mut merged: Option<GetConversationResponse> = None;
  loop {
    let request = GetConversationRequest {
      project: project.to_owned(),
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
        server_id: server_id.clone(),
        project: project.clone(),
        goal_id: goal_id.clone(),
        inquiry_id: inquiry_id.clone(),
        text,
        report,
      });
      request.set_timeout(WRITE_TIMEOUT);
      let response =
        client.answer_inquiry(request).await.map_err(|status| write_call_error(addr, operation, status))?.into_inner();
      Ok(render_answer(&response))
    },
    | WriteCommand::Merge { server_id, project, goal_id, pr_number, until } => {
      let mut request = Request::new(ApproveGoalMergeRequest {
        server_id: server_id.clone(),
        project:   project.clone(),
        goal_id:   goal_id.clone(),
        pr_number: *pr_number,
        until:     until.clone(),
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
        server_id: server_id.clone(),
        project:   project.clone(),
      });
      request.set_timeout(READ_TIMEOUT);
      let response = client
        .list_goal_registration_proposals(request)
        .await
        .map_err(|status| write_call_error(addr, operation, status))?
        .into_inner();
      Ok(render_registrations(&response.proposals))
    },
    | WriteCommand::ApproveRegistration { server_id, project, revision } => {
      let mut request = Request::new(ApproveGoalRegistrationRequest {
        server_id: server_id.clone(),
        project:   project.clone(),
        revision:  *revision,
      });
      request.set_timeout(WRITE_TIMEOUT);
      let response = client
        .approve_goal_registration(request)
        .await
        .map_err(|status| write_call_error(addr, operation, status))?
        .into_inner();
      Ok(render_approve_registration(&response))
    },
    | WriteCommand::RejectRegistration { server_id, project, revision } => {
      let mut request = Request::new(RejectGoalRegistrationRequest {
        server_id: server_id.clone(),
        project:   project.clone(),
        revision:  *revision,
      });
      request.set_timeout(WRITE_TIMEOUT);
      let response = client
        .reject_goal_registration(request)
        .await
        .map_err(|status| write_call_error(addr, operation, status))?
        .into_inner();
      Ok(render_reject_registration(&response))
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
      | Ok(state) => {
        let _ = writeln!(stdout, "{}", render_decisions(&state, &server_id, &project));
        EXIT_OK
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
    },
  }
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
    | Ok(state) => {
      let _ = writeln!(stdout, "{}", render_overview(&state));
      EXIT_OK
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
    | Ok(response) => {
      let _ = writeln!(stdout, "{}", render_conversation(&response));
      EXIT_OK
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
  Decisions { server_id: String, project: String },
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

/// 会話の表示内容を日本語の文字列へ組み立てる。
///
/// 発言は seq の昇順に並べ、発話者・時刻・本文を出す。unavailable_reason は会話が無い理由、
/// skipped は理由と件数として表示する。
fn render_conversation(response: &GetConversationResponse) -> String {
  let mut lines = vec![
    format!("会話: {}", response.project),
    format!("セッション: {}", response.session_id),
    format!("プロバイダ: {}", response.provider),
  ];
  if let Some(reason) = response.unavailable_reason.as_deref() {
    lines.push(format!("会話を表示できません: {reason}"));
  }
  for skipped in &response.skipped {
    lines.push(format!("スキップした行: {} {}件", skipped.reason, skipped.count));
  }

  let mut messages = response.messages.clone();
  messages.sort_by_key(|message| message.seq);
  messages.dedup_by_key(|message| message.seq);
  if messages.is_empty() {
    if response.unavailable_reason.is_none() {
      lines.push("（会話はありません）".to_owned());
    }
  } else {
    lines.extend(messages.iter().map(|message| {
      format!(
        "- [{}] {}: {}",
        format_timestamp(message.occurred_at.as_ref()),
        conversation_speaker_label(message.speaker),
        message.text
      )
    }));
  }
  lines.join("\n")
}

/// 発話者の数値を表示名へ変換する。未知の値は「不明」にする。
fn conversation_speaker_label(speaker: i32) -> &'static str {
  match proto::v1::ConversationSpeaker::try_from(speaker) {
    | Ok(proto::v1::ConversationSpeaker::User) => "ユーザー",
    | Ok(proto::v1::ConversationSpeaker::Manager) => "manager",
    | Ok(proto::v1::ConversationSpeaker::System) => "システム",
    | _ => "不明",
  }
}

/// ゴールの状態を表示名へ変換する。未知の値は「不明」にする。
fn goal_status_label(status: i32) -> &'static str {
  match GoalStatus::try_from(status) {
    | Ok(GoalStatus::Created) => "作成済み",
    | Ok(GoalStatus::AwaitingMerge) => "人のマージ待ち",
    | Ok(GoalStatus::Completed) => "完了",
    | _ => "不明",
  }
}

/// ゴールの実行状態を表示名へ変換する。未知の値は「不明」にする。
fn execution_status_label(status: i32) -> &'static str {
  match ExecutionStatus::try_from(status) {
    | Ok(ExecutionStatus::Active) => "稼働中",
    | Ok(ExecutionStatus::Paused) => "一時停止中",
    | Ok(ExecutionStatus::Aborted) => "中止",
    | _ => "不明",
  }
}

/// タスクの状態を表示名へ変換する。未知の値は「不明」にする。
fn task_status_label(status: i32) -> &'static str {
  match TaskStatus::try_from(status) {
    | Ok(TaskStatus::Pending) => "未着手",
    | Ok(TaskStatus::Running) => "実行中",
    | Ok(TaskStatus::Completed) => "完了",
    | Ok(TaskStatus::Failed) => "失敗",
    | Ok(TaskStatus::Exceeded) => "上限超過",
    | Ok(TaskStatus::PrFailed) => "PR 失敗",
    | _ => "不明",
  }
}

/// 質問の種類を表示名へ変換する。未知の値は原値のまま返す。
fn question_kind_label(kind: &str) -> String {
  match kind {
    | "question" => "質問".to_owned(),
    | "operation_request" => "作業の依頼".to_owned(),
    | "approval" => "承認".to_owned(),
    | other => other.to_owned(),
  }
}

/// 質問の状態を表示名へ変換する。未知の値は原値のまま返す。
fn question_status_label(status: &str) -> String {
  match status {
    | "pending" => "未回答".to_owned(),
    | "answered" => "回答済み".to_owned(),
    | "withdrawn" => "取り下げ".to_owned(),
    | other => other.to_owned(),
  }
}

/// 質問への回答（可否）を表示名へ変換する。未知の値は原値のまま返す。
fn answer_decision_label(decision: &str) -> String {
  match decision {
    | "allow" => "許可".to_owned(),
    | "deny" => "否認".to_owned(),
    | other => other.to_owned(),
  }
}

/// 質問への回答（作業の報告）を表示名へ変換する。未知の値は原値のまま返す。
fn answer_report_label(report: &str) -> String {
  match report {
    | "completed" => "実施した".to_owned(),
    | "unable" => "対応できない".to_owned(),
    | other => other.to_owned(),
  }
}

/// ゴール一覧の表示内容を日本語の文字列へ組み立てる。
///
/// 応答順に1行ずつ、サーバー・プロジェクト・ゴールID・名前・状態・実行状態・タスクの4種の
/// 件数・マージ待ちの有無・プルリクエストを出す。空の一覧は `join_lines` が「（なし）」にする。
fn render_goals(response: &GetGoalsResponse) -> String {
  let lines: Vec<String> = response.goals.iter().map(render_goal_row).collect();
  join_lines(&lines)
}

/// ゴール一覧の1行を作る。マージ待ちは `awaiting_merge_at` の有無だけで判定する。
fn render_goal_row(goal: &Goal) -> String {
  let counts = goal.task_counts.unwrap_or_default();
  let merge_wait = if goal.awaiting_merge_at.is_some() { "あり" } else { "なし" };
  let pull_request = match &goal.pull_request {
    | Some(pull_request) => format!("#{} {}", pull_request.number, pull_request.url),
    | None => "なし".to_owned(),
  };
  format!(
    "- {}/{}/{}「{}」状態: {}、実行状態: {}、タスク: 完了{}件・失敗{}件（未着手{}件・実行中{}件）、\
     マージ待ち: {}、プルリクエスト: {}",
    goal.server_id,
    goal.project,
    goal.goal_id,
    goal.name,
    goal_status_label(goal.status),
    execution_status_label(goal.execution_status),
    counts.completed,
    counts.failed,
    counts.pending,
    counts.running,
    merge_wait,
    pull_request
  )
}

/// ゴール詳細の表示内容を日本語の文字列へ組み立てる。
///
/// 目的・対象外・受け入れ条件・制約・ブランチ・統合ブランチ・状態・実行状態・作業記録・
/// 質問と回答・紐づくタスクを出す。空の一覧は「（なし）」にする。
fn render_goal_detail(detail: &GetGoalDetailResponse) -> String {
  let work_log: Vec<String> = detail
    .work_log
    .iter()
    .map(|log| format!("- {} {}", format_timestamp(log.recorded_at.as_ref()), log.summary))
    .collect();
  let questions: Vec<String> = detail.questions.iter().map(render_goal_question).collect();
  let tasks: Vec<String> = detail
    .tasks
    .iter()
    .map(|task| {
      let run_slug = task.run_slug.as_deref().filter(|run_slug| !run_slug.is_empty()).unwrap_or("なし");
      format!("- {}（{}、run: {}）", task.task_name, task_status_label(task.status), run_slug)
    })
    .collect();
  [
    format!("ゴール: {}", detail.goal_id),
    format!("サーバー: {}", detail.server_id),
    format!("プロジェクト: {}", detail.project),
    format!("状態: {}", goal_status_label(detail.status)),
    format!("実行状態: {}", execution_status_label(detail.execution_status)),
    format!("目的: {}", detail.objective),
    format!("対象外:\n{}", render_string_items(&detail.out_of_scope)),
    format!("受け入れ条件:\n{}", render_string_items(&detail.acceptance_criteria)),
    format!("制約:\n{}", render_string_items(&detail.constraints)),
    format!("ブランチ: {}", detail.branch),
    format!("統合ブランチ: {}", detail.integration_branch),
    format!("作業記録:\n{}", join_lines(&work_log)),
    format!("質問:\n{}", join_lines(&questions)),
    format!("タスク:\n{}", join_lines(&tasks)),
  ]
  .join("\n")
}

/// ゴールの質問1件を表示する。回答があれば回答本文を併記する。
fn render_goal_question(question: &GoalQuestion) -> String {
  let mut line = format!(
    "- {}（{}）: {}",
    question_kind_label(&question.kind),
    question_status_label(&question.status),
    question.body
  );
  if let Some(answer) = question.answer_text.as_deref() {
    line.push_str(&format!("/回答: {answer}"));
  }
  if let Some(decision) = question.answer_decision.as_deref() {
    line.push_str(&format!("/回答: {}", answer_decision_label(decision)));
  }
  if let Some(report) = question.answer_report.as_deref() {
    line.push_str(&format!("/回答: {}", answer_report_label(report)));
  }
  line
}

/// 文字列の一覧を箇条書きにし、空の一覧は「（なし）」にする。
fn render_string_items(items: &[String]) -> String {
  let lines: Vec<String> = items.iter().map(|item| format!("- {item}")).collect();
  join_lines(&lines)
}

/// 判断待ちの一覧を、指定したサーバーとプロジェクトに絞って表示する。
///
/// 回答・マージの承認へそのまま渡せる識別子（問い合わせ ID・ゴール ID・プルリクエスト番号・
/// リビジョン）を固定ラベルで出す。滞留期限は proto に専用フィールドが無いため、作成時刻と
/// 設定の滞留分数から算出して併記します。承認待ちの登録提案は backend が作成時刻を射影時刻で
/// 返すため、滞留期限の算出対象にしません。
fn render_decisions(state: &DashboardState, server_id: &str, project: &str) -> String {
  let stale_minutes = state.thresholds.unwrap_or_default().pending_decision_stale_minutes;
  let decisions: Vec<&proto::v1::PendingDecision> = state
    .pending_decisions
    .iter()
    .filter(|decision| decision.server_id == server_id && decision.project == project)
    .collect();
  let lines: Vec<String> = decisions.iter().map(|decision| render_decision(decision, stale_minutes)).collect();
  format!("判断待ち: {}件（{}/{}）\n{}", decisions.len(), server_id, project, join_lines(&lines))
}

/// 判断待ち1件の表示を作る。該当しない項目の行は出さない。
fn render_decision(decision: &proto::v1::PendingDecision, stale_minutes: i32) -> String {
  let kind = if decision.r#type == PendingDecisionType::AwaitingMerge as i32 {
    "人のマージ待ち"
  } else if decision.r#type == PendingDecisionType::GoalRegistration as i32 {
    "承認待ち"
  } else if decision.inquiry_kind == proto::v1::InquiryKind::OperationRequest as i32 {
    "問い合わせ（作業の依頼）"
  } else {
    "問い合わせ（質問）"
  };
  let mut lines = vec![format!("- {kind}: {}", decision.body)];
  if !decision.inquiry_id.is_empty() {
    lines.push(format!("問い合わせID: {}", decision.inquiry_id));
  }
  if let Some(goal_id) = decision.goal_id.as_deref().filter(|goal_id| !goal_id.is_empty()) {
    lines.push(format!("ゴールID: {goal_id}"));
  }
  if let Some(pull_request) = &decision.pull_request {
    lines.push(format!("プルリクエスト番号: {}", pull_request.number));
  }
  if let Some(revision) = decision.revision {
    lines.push(format!("リビジョン: {revision}"));
  }
  if decision.r#type != PendingDecisionType::GoalRegistration as i32 {
    lines.push(format!("作成時刻: {}", format_timestamp(decision.created_at.as_ref())));
    lines.push(format!("滞留期限: {}", stale_deadline(decision.created_at.as_ref(), stale_minutes)));
  }
  if !decision.options.is_empty() {
    lines.push(format!("選択肢: {}", decision.options.join("、")));
  }
  lines.join("\n")
}

/// 判断待ちの滞留期限（作成時刻 + 滞留分数）を表示用の文字列にする。
fn stale_deadline(created_at: Option<&prost_types::Timestamp>, stale_minutes: i32) -> String {
  let Some(created_at) = created_at else {
    return "不明".to_owned();
  };
  let Some(at) = chrono::DateTime::from_timestamp(created_at.seconds, 0) else {
    return "不明".to_owned();
  };
  (at + chrono::Duration::minutes(i64::from(stale_minutes))).format("%Y-%m-%d %H:%M:%S UTC").to_string()
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
  format!("回答を記録しました。\n問い合わせ: {}", response.inquiry_id)
}

/// マージの承認の結果を表示する。
fn render_merge(response: &ApproveGoalMergeResponse) -> String {
  format!("マージを承認しました。\nタスク: {}\nワークフロー: {}", response.task_name, response.workflow)
}

/// ゴール登録の承認待ちの一覧を表示する。
fn render_registrations(proposals: &[GoalRegistrationProposal]) -> String {
  let lines: Vec<String> = proposals
    .iter()
    .map(|proposal| {
      format!(
        "- {}/{}（リビジョン {}）: {}（{}）",
        proposal.server_id, proposal.project, proposal.revision, proposal.body, proposal.repository_directory
      )
    })
    .collect();
  format!("ゴール登録の承認待ち: {}件\n{}", proposals.len(), join_lines(&lines))
}

/// ゴール登録の承認の結果を表示する。
fn render_approve_registration(response: &ApproveGoalRegistrationResponse) -> String {
  format!("ゴール登録を承認しました。\nリビジョン: {}\nゴール: {}", response.revision, response.goal_id)
}

/// ゴール登録の却下の結果を表示する。
fn render_reject_registration(response: &RejectGoalRegistrationResponse) -> String {
  format!("ゴール登録を却下しました。\nリビジョン: {}", response.revision)
}

fn main() {
  let args: Vec<String> = std::env::args().skip(1).collect();
  std::process::exit(run(&args));
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod write_command_tests;

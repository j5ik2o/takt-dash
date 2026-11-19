//! コマンドラインの引数を解釈し、サブコマンドと検証済みの入力へ変換します。

use std::path::PathBuf;

use crate::{
  command::{
    Answer, ConversationOptions, DecisionsOptions, GoalOptions, GoalsOptions, Invocation, OverviewOptions,
    WriteCommand, WriteInvocation, ANSWER_COMMAND, APPROVE_REGISTRATION_COMMAND, CONVERSATION_COMMAND,
    DECISIONS_COMMAND, GOALS_COMMAND, GOAL_COMMAND, MERGE_COMMAND, OVERVIEW_COMMAND, REGISTRATIONS_COMMAND,
    REJECT_REGISTRATION_COMMAND,
  },
  domain::{self, GoalId, MergeUntil, ProjectName, PullRequestNumber, RegistrationRevision},
  proto::v1::OperationAnswerReport,
};

/// 引数の解釈に失敗したときに返す失敗。使い方と一緒に表示します。
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct UsageError(pub(crate) String);

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

/// 引数の走査で集めた、検証前の値。
#[derive(Default)]
struct RawArguments<'a> {
  command:  Option<&'a str>,
  addr:     Option<String>,
  config:   Option<PathBuf>,
  server:   Option<String>,
  project:  Option<String>,
  goal:     Option<String>,
  question: Option<String>,
  text:     Option<String>,
  report:   Option<String>,
  pr:       Option<String>,
  until:    Option<String>,
  revision: Option<String>,
  session:  Option<String>,
}

/// コマンドラインの引数を解釈する。
pub(crate) fn parse_arguments(args: &[String]) -> Result<Invocation, UsageError> {
  match scan_arguments(args)? {
    | None => Ok(Invocation::Help),
    | Some(raw) => build_invocation(raw),
  }
}

/// 引数を先頭から走査し、コマンドとフラグの値を集める。
///
/// `--help` または `-h` を見つけた時点で走査を止め、`None` を返します。それより前の引数の誤りは、
/// 使い方の表示より先に返します。
fn scan_arguments(args: &[String]) -> Result<Option<RawArguments<'_>>, UsageError> {
  let mut raw = RawArguments::default();
  let mut index = 0;
  while index < args.len() {
    let argument = args[index].as_str();
    if argument == "--help" || argument == "-h" {
      return Ok(None);
    }
    if argument.starts_with('-') {
      if !flag_allowed(raw.command, argument) {
        return Err(UsageError(format!("未知の引数です: {argument}")));
      }
      store_flag(&mut raw, args, index, argument)?;
      index += 2;
      continue;
    }
    if raw.command.is_none() {
      if !is_known_command(argument) {
        return Err(UsageError(format!("未知のコマンドです: {argument}")));
      }
      raw.command = Some(argument);
      index += 1;
      continue;
    }
    if raw.command == Some(CONVERSATION_COMMAND) {
      if raw.project.is_some() {
        return Err(UsageError(format!("会話の表示に位置引数は1つだけ取れます: {argument}")));
      }
      raw.project = Some(argument.to_owned());
      index += 1;
      continue;
    }
    return Err(UsageError(format!("{} に位置引数は取れません: {argument}", raw.command.unwrap_or(OVERVIEW_COMMAND))));
  }
  Ok(Some(raw))
}

/// `args[index]` のフラグの値を `args[index + 1]` から取り出し、対応する欄へ入れる。
fn store_flag(raw: &mut RawArguments<'_>, args: &[String], index: usize, flag: &str) -> Result<(), UsageError> {
  let slot = match flag {
    | "--config" => {
      raw.config = Some(PathBuf::from(take_value(args, index, "--config")?));
      return Ok(());
    },
    | "--session" if raw.command == Some(OVERVIEW_COMMAND) => {
      return Err(UsageError("概要の表示に --session は使えません。".to_owned()));
    },
    | "--addr" => &mut raw.addr,
    | "--server" => &mut raw.server,
    | "--project" => &mut raw.project,
    | "--goal" => &mut raw.goal,
    | "--question" => &mut raw.question,
    | "--text" => &mut raw.text,
    | "--report" => &mut raw.report,
    | "--pr" => &mut raw.pr,
    | "--until" => &mut raw.until,
    | "--revision" => &mut raw.revision,
    | "--session" => &mut raw.session,
    | _ => return Err(UsageError(format!("未知の引数です: {flag}"))),
  };
  *slot = Some(take_value(args, index, flag)?);
  Ok(())
}

/// 集めた値をコマンドごとに検証し、解釈の結果を組み立てる。
fn build_invocation(mut raw: RawArguments<'_>) -> Result<Invocation, UsageError> {
  let command = raw.command.unwrap_or(OVERVIEW_COMMAND);
  if command == OVERVIEW_COMMAND {
    if raw.session.is_some() {
      return Err(UsageError("概要の表示に --session は使えません。".to_owned()));
    }
    return Ok(Invocation::Overview(OverviewOptions { addr: raw.addr, config: raw.config }));
  }
  if command == CONVERSATION_COMMAND {
    let project = raw.project.ok_or_else(|| UsageError("会話の表示にはプロジェクト名が必要です。".to_owned()))?;
    let project = parse_project_name(&project)?;
    return Ok(Invocation::Conversation(ConversationOptions {
      project,
      session: raw.session,
      addr: raw.addr,
      config: raw.config,
    }));
  }
  if command == GOALS_COMMAND {
    return Ok(Invocation::Goals(GoalsOptions { addr: raw.addr, config: raw.config }));
  }
  if command == GOAL_COMMAND {
    let goal_id = require_flag(raw.goal, GOAL_COMMAND, "--goal")?;
    return Ok(Invocation::Goal(GoalOptions { goal_id, addr: raw.addr, config: raw.config }));
  }
  if command == DECISIONS_COMMAND {
    let server_id = require_parsed(raw.server, DECISIONS_COMMAND, "--server")?;
    let project = require_parsed(raw.project, DECISIONS_COMMAND, "--project")?;
    return Ok(Invocation::Decisions(DecisionsOptions { server_id, project, addr: raw.addr, config: raw.config }));
  }
  let addr = raw.addr.take();
  let config = raw.config.take();
  let command = build_write_command(command, raw)?;
  Ok(Invocation::Write(WriteInvocation { command, addr, config }))
}

/// 書き込みコマンドの値を、コマンドごとの順で検証して組み立てる。
fn build_write_command(command: &str, raw: RawArguments<'_>) -> Result<WriteCommand, UsageError> {
  match command {
    | ANSWER_COMMAND => {
      let server_id = require_parsed(raw.server, ANSWER_COMMAND, "--server")?;
      let project = require_parsed(raw.project, ANSWER_COMMAND, "--project")?;
      let inquiry_id = require_parsed(raw.question, ANSWER_COMMAND, "--question")?;
      let goal_id = parse_optional_goal(raw.goal)?;
      let answer = parse_answer(raw.text, raw.report)?;
      Ok(WriteCommand::Answer { server_id, project, goal_id, inquiry_id, answer })
    },
    | MERGE_COMMAND => {
      let server_id = require_parsed(raw.server, MERGE_COMMAND, "--server")?;
      let project = require_parsed(raw.project, MERGE_COMMAND, "--project")?;
      let goal_id = require_parsed(raw.goal, MERGE_COMMAND, "--goal")?;
      let pr_number = parse_pr_number(raw.pr)?;
      let until = parse_until(raw.until)?;
      Ok(WriteCommand::Merge { server_id, project, goal_id, pr_number, until })
    },
    | REGISTRATIONS_COMMAND => {
      let server_id = require_parsed(raw.server, REGISTRATIONS_COMMAND, "--server")?;
      let project = require_parsed(raw.project, REGISTRATIONS_COMMAND, "--project")?;
      Ok(WriteCommand::Registrations { server_id, project })
    },
    | APPROVE_REGISTRATION_COMMAND => {
      let server_id = require_parsed(raw.server, APPROVE_REGISTRATION_COMMAND, "--server")?;
      let project = require_parsed(raw.project, APPROVE_REGISTRATION_COMMAND, "--project")?;
      let revision = parse_revision(raw.revision)?;
      Ok(WriteCommand::ApproveRegistration { server_id, project, revision })
    },
    | REJECT_REGISTRATION_COMMAND => {
      let server_id = require_parsed(raw.server, REJECT_REGISTRATION_COMMAND, "--server")?;
      let project = require_parsed(raw.project, REJECT_REGISTRATION_COMMAND, "--project")?;
      let revision = parse_revision(raw.revision)?;
      Ok(WriteCommand::RejectRegistration { server_id, project, revision })
    },
    | other => Err(UsageError(format!("未知のコマンドです: {other}"))),
  }
}

/// 必須のフラグの値を取り出す。
fn require_flag(value: Option<String>, command: &str, flag: &str) -> Result<String, UsageError> {
  value.ok_or_else(|| UsageError(format!("{command} には {flag} が必要です。")))
}

/// 必須のフラグの値を、検証済みの型へ変換して取り出す。
fn require_parsed<T>(value: Option<String>, command: &str, flag: &str) -> Result<T, UsageError>
where
  T: for<'a> TryFrom<&'a str, Error = domain::DomainError>, {
  let value = require_flag(value, command, flag)?;
  T::try_from(value.as_str()).map_err(|error| UsageError(format!("{flag} の値が不正です: {value}（{error}）")))
}

/// 位置引数のプロジェクト名を検証する。
fn parse_project_name(value: &str) -> Result<ProjectName, UsageError> {
  ProjectName::try_from(value).map_err(|error| UsageError(format!("プロジェクト名の値が不正です: {value}（{error}）")))
}

/// `--goal` の任意指定を検証する。
fn parse_optional_goal(value: Option<String>) -> Result<Option<GoalId>, UsageError> {
  match value {
    | Some(value) => GoalId::try_from(value.as_str())
      .map(Some)
      .map_err(|error| UsageError(format!("--goal の値が不正です: {value}（{error}）"))),
    | None => Ok(None),
  }
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
fn parse_pr_number(value: Option<String>) -> Result<PullRequestNumber, UsageError> {
  let value = value.ok_or_else(|| UsageError("merge には --pr が必要です。".to_owned()))?;
  let pr_number =
    value.parse::<i64>().map_err(|_| UsageError(format!("--pr には正の整数を指定してください: {value}")))?;
  PullRequestNumber::try_from(pr_number)
    .map_err(|_| UsageError(format!("--pr には正の整数を指定してください: {value}")))
}

/// `--until` の値を、`MergeUntil` が受け付ける値に限定して取り出す。
fn parse_until(value: Option<String>) -> Result<MergeUntil, UsageError> {
  match value.as_deref() {
    | Some(value) => MergeUntil::try_from(value)
      .map_err(|_| UsageError(format!("--until には prepare か merge を指定してください: {value}"))),
    | None => Err(UsageError("merge には --until が必要です。".to_owned())),
  }
}

/// `--revision` の値を正の整数として取り出す。
fn parse_revision(value: Option<String>) -> Result<RegistrationRevision, UsageError> {
  let value = value.ok_or_else(|| UsageError("--revision には正の整数が必要です。".to_owned()))?;
  let revision =
    value.parse::<i32>().map_err(|_| UsageError(format!("--revision には正の整数を指定してください: {value}")))?;
  RegistrationRevision::try_from(revision)
    .map_err(|_| UsageError(format!("--revision には正の整数を指定してください: {value}")))
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
pub(crate) fn usage() -> String {
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

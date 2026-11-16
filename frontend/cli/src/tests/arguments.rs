//! 使い方エラーと引数の解析。

use super::support::write_args;
use crate::{
  command::{WriteInvocation, ANSWER_COMMAND},
  domain::{GoalId, InquiryId, MergeUntil, ProjectName, PullRequestNumber, ServerId},
  parse_arguments, proto, run_with, Answer, ConversationOptions, Invocation, OverviewOptions, UsageError, WriteCommand,
  DEFAULT_BACKEND_ADDR, EXIT_USAGE,
};

// ----------------------------------------------------------------------------
// CLI-USAGE: 書き込みコマンドの必須引数が欠けたときの使い方エラー
// ----------------------------------------------------------------------------

#[test]
fn write_command_missing_required_flag_is_rejected_with_usage() {
  let mut stdout = Vec::new();
  let mut stderr = Vec::new();

  // answer は --server/--project/--question と --text か --report が必須。欠けたら backend
  // を呼ばず使い方を出す。
  let code = run_with(&[ANSWER_COMMAND.to_owned()], Ok(DEFAULT_BACKEND_ADDR.to_owned()), &mut stdout, &mut stderr);

  let stdout = String::from_utf8(stdout).expect("標準出力は UTF-8 である必要があります");
  let stderr = String::from_utf8(stderr).expect("標準エラーは UTF-8 である必要があります");
  assert_eq!(code, EXIT_USAGE, "必須引数が欠けた書き込みコマンドは使い方を出して異常終了する必要があります");
  assert!(stderr.contains("answer には"), "欠けた引数を示す必要があります: {stderr}");
  assert!(stderr.contains("使い方: takt-dash-cli"), "使い方を示す必要があります: {stderr}");
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
fn generated_client_exposes_the_read_and_write_calls() {
  // proto のサービス定義は読み取りの 6 つと書き込みの 5 つを持つ。CLI はこの版から
  // 書き込みを画面と共有する API として使うため、生成クライアントの型に両方が現れる。
  let _request_types = (
    std::marker::PhantomData::<proto::v1::GetOverviewRequest>,
    std::marker::PhantomData::<proto::v1::GetProjectsRequest>,
    std::marker::PhantomData::<proto::v1::GetPendingDecisionsRequest>,
    std::marker::PhantomData::<proto::v1::GetActivityRequest>,
    std::marker::PhantomData::<proto::v1::GetRunDetailRequest>,
    std::marker::PhantomData::<proto::v1::WatchStateRequest>,
    std::marker::PhantomData::<proto::v1::AnswerInquiryRequest>,
    std::marker::PhantomData::<proto::v1::ApproveGoalMergeRequest>,
    std::marker::PhantomData::<proto::v1::ApproveGoalRegistrationRequest>,
    std::marker::PhantomData::<proto::v1::RejectGoalRegistrationRequest>,
    std::marker::PhantomData::<proto::v1::ListGoalRegistrationProposalsRequest>,
  );
  let _response_types = (
    std::marker::PhantomData::<proto::v1::AnswerInquiryResponse>,
    std::marker::PhantomData::<proto::v1::ApproveGoalMergeResponse>,
    std::marker::PhantomData::<proto::v1::ApproveGoalRegistrationResponse>,
    std::marker::PhantomData::<proto::v1::RejectGoalRegistrationResponse>,
    std::marker::PhantomData::<proto::v1::ListGoalRegistrationProposalsResponse>,
  );
}

#[test]
fn generated_client_type_uses_the_transport_channel() {
  let _client: std::marker::PhantomData<
    proto::v1::takt_dash_service_client::TaktDashServiceClient<tonic::transport::Channel>,
  > = std::marker::PhantomData;
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
fn parse_arguments_accepts_conversation_with_project_and_session() {
  let args = [
    "conversation",
    "corporate-site",
    "--session",
    "session-1",
    "--addr",
    "127.0.0.1:50052",
    "--config",
    "/tmp/a.json",
  ]
  .map(str::to_owned);

  assert_eq!(
    parse_arguments(&args),
    Ok(Invocation::Conversation(ConversationOptions {
      project: ProjectName::try_from("corporate-site").expect("有効です"),
      session: Some("session-1".to_owned()),
      addr:    Some("127.0.0.1:50052".to_owned()),
      config:  Some(std::path::PathBuf::from("/tmp/a.json")),
    }))
  );
}

#[test]
fn parse_arguments_requires_project_for_conversation() {
  let result = parse_arguments(&["conversation".to_owned()]);

  match result {
    | Err(UsageError(message)) => assert!(message.contains("プロジェクト"), "-- {message}"),
    | other => panic!("会話のプロジェクト未指定は使い方の誤りにする必要があります: {other:?}"),
  }
}

#[test]
fn parse_arguments_rejects_extra_project_position() {
  let args = ["conversation".to_owned(), "corporate-site".to_owned(), "esa".to_owned()];

  match parse_arguments(&args) {
    | Err(UsageError(message)) => assert!(message.contains("esa"), "-- {message}"),
    | other => panic!("会話の2個目の位置引数は使い方の誤りにする必要があります: {other:?}"),
  }
}

#[test]
fn parse_arguments_rejects_session_for_overview() {
  let args = ["overview".to_owned(), "--session".to_owned(), "session-1".to_owned()];

  match parse_arguments(&args) {
    | Err(UsageError(message)) => assert!(message.contains("--session"), "-- {message}"),
    | other => panic!("概要に --session は使い方の誤りにする必要があります: {other:?}"),
  }
}

// ----------------------------------------------------------------------------
// 書き込みコマンドの引数解釈
// ----------------------------------------------------------------------------

#[test]
fn parse_arguments_builds_answer_with_text() {
  let args = write_args(&[
    "answer",
    "--server",
    "mac-mini",
    "--project",
    "takt-dash",
    "--goal",
    "g-1",
    "--question",
    "q-1",
    "--text",
    "はい",
  ]);
  assert_eq!(
    parse_arguments(&args),
    Ok(Invocation::Write(WriteInvocation {
      command: WriteCommand::Answer {
        server_id:  ServerId::try_from("mac-mini").expect("有効です"),
        project:    ProjectName::try_from("takt-dash").expect("有効です"),
        goal_id:    Some(GoalId::try_from("g-1").expect("有効です")),
        inquiry_id: InquiryId::try_from("q-1").expect("有効です"),
        answer:     Answer::Text("はい".to_owned()),
      },
      addr:    None,
      config:  None,
    }))
  );
}

#[test]
fn parse_arguments_rejects_answer_with_text_and_report() {
  let args = write_args(&[
    "answer",
    "--server",
    "mac-mini",
    "--project",
    "takt-dash",
    "--question",
    "q-1",
    "--text",
    "はい",
    "--report",
    "completed",
  ]);
  assert!(parse_arguments(&args).is_err(), "--text と --report の同時指定は誤りです");
}

#[test]
fn parse_arguments_rejects_unknown_report_value() {
  let args =
    write_args(&["answer", "--server", "mac-mini", "--project", "takt-dash", "--question", "q-1", "--report", "done"]);
  assert!(parse_arguments(&args).is_err(), "--report は completed か unable だけを許します");
}

#[test]
fn parse_arguments_builds_merge_with_until() {
  let args = write_args(&[
    "merge",
    "--server",
    "mac-mini",
    "--project",
    "takt-dash",
    "--goal",
    "g-1",
    "--pr",
    "42",
    "--until",
    "merge",
  ]);
  assert_eq!(
    parse_arguments(&args),
    Ok(Invocation::Write(WriteInvocation {
      command: WriteCommand::Merge {
        server_id: ServerId::try_from("mac-mini").expect("有効です"),
        project:   ProjectName::try_from("takt-dash").expect("有効です"),
        goal_id:   GoalId::try_from("g-1").expect("有効です"),
        pr_number: PullRequestNumber::try_from(42).expect("有効です"),
        until:     MergeUntil::Merge,
      },
      addr:    None,
      config:  None,
    }))
  );
}

#[test]
fn parse_arguments_rejects_merge_with_unknown_until() {
  let args = write_args(&[
    "merge",
    "--server",
    "mac-mini",
    "--project",
    "takt-dash",
    "--goal",
    "g-1",
    "--pr",
    "42",
    "--until",
    "later",
  ]);
  assert!(parse_arguments(&args).is_err(), "--until は prepare か merge だけを許します");
}

#[test]
fn parse_arguments_rejects_a_flag_for_another_command() {
  // --pr は merge 専用。answer に付けたら未知の引数として拒否する。
  let args = write_args(&["answer", "--server", "mac-mini", "--project", "takt-dash", "--pr", "42"]);
  assert_eq!(parse_arguments(&args), Err(UsageError("未知の引数です: --pr".to_owned())));
}

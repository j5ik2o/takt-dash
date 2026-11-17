//! proto/taktdash.proto の API 契約を検証する。
//!
//! このテストは、proto/ の定義本文と、そこから生成した Rust の型の両方を観測する。
//! proto を変えて生成物が追随しない状態、読み取りと書き込みの RPC 集合が要件から外れた状態、
//! 判断待ちの種類が要件の3種類（問い合わせ・マージ待ち・ゴール登録の承認待ち）から外れた状態を、
//! それぞれ失敗させる。

use takt_dash_backend::takt_dash::v1;

const PROTO_SOURCE: &str = include_str!("../../proto/taktdash.proto");

/// service 本文の `rpc <名前>(...) returns (...)` から、RPC の名前を出現順に取り出す。
fn rpc_declarations(source: &str) -> Vec<String> {
  let mut names = Vec::new();
  for line in source.lines() {
    let trimmed = line.trim();
    let Some(rest) = trimmed.strip_prefix("rpc ") else {
      continue;
    };
    let name: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
    if !name.is_empty() {
      names.push(name);
    }
  }
  names
}

#[test]
fn proto_defines_the_read_and_write_rpcs() {
  let names = rpc_declarations(PROTO_SOURCE);
  let mut sorted = names.clone();
  sorted.sort();
  sorted.dedup();
  assert_eq!(
    sorted,
    vec![
      "AnswerInquiry".to_owned(),
      "ApproveGoalMerge".to_owned(),
      "ApproveGoalRegistration".to_owned(),
      "GetActivity".to_owned(),
      "GetConversation".to_owned(),
      "GetGoalDetail".to_owned(),
      "GetGoals".to_owned(),
      "GetOverview".to_owned(),
      "GetPendingDecisions".to_owned(),
      "GetProjects".to_owned(),
      "GetRunContent".to_owned(),
      "GetRunDetail".to_owned(),
      "GetTaskDetail".to_owned(),
      "ListGoalRegistrationProposals".to_owned(),
      "RejectGoalRegistration".to_owned(),
      "WatchState".to_owned(),
    ],
    "読み取りの10 RPC と WatchState と書き込みの5 RPC が定義されている必要があります: {names:?}"
  );
}

#[test]
fn watch_state_is_a_server_streaming_rpc() {
  let declarations: Vec<&str> =
    PROTO_SOURCE.lines().map(str::trim).filter(|line| line.starts_with("rpc WatchState")).collect();
  assert_eq!(declarations.len(), 1, "WatchState の宣言は1つだけ必要です: {declarations:?}");
  assert!(
    declarations[0].contains("returns (stream "),
    "WatchState は server streaming として宣言する必要があります: {}",
    declarations[0]
  );
  for name in [
    "GetOverview",
    "GetProjects",
    "GetPendingDecisions",
    "GetActivity",
    "GetRunDetail",
    "GetConversation",
    "GetGoals",
    "GetGoalDetail",
    "GetTaskDetail",
    "AnswerInquiry",
    "ApproveGoalMerge",
    "ApproveGoalRegistration",
    "RejectGoalRegistration",
    "ListGoalRegistrationProposals",
  ] {
    let line = PROTO_SOURCE
      .lines()
      .map(str::trim)
      .find(|line| line.starts_with(&format!("rpc {name}")))
      .unwrap_or_else(|| panic!("{name} の宣言が見つかりません"));
    assert!(!line.contains("stream "), "{name} は unary として宣言する必要があります: {line}");
  }
}

#[test]
fn pending_decision_type_covers_inquiry_awaiting_merge_and_goal_registration() {
  let line = PROTO_SOURCE
    .lines()
    .map(str::trim)
    .find(|line| line.starts_with("enum PendingDecisionType"))
    .unwrap_or_else(|| panic!("enum PendingDecisionType が見つかりません"));
  let body = PROTO_SOURCE
    .split("enum PendingDecisionType")
    .nth(1)
    .and_then(|rest| rest.split('}').next())
    .unwrap_or_else(|| panic!("PendingDecisionType の本文が見つかりません: {line}"));
  let members: Vec<&str> = body
    .split(';')
    .filter_map(|part| part.split('=').next())
    .map(|name| name.trim().trim_start_matches('{').trim())
    .filter(|name| !name.is_empty())
    .collect();
  assert_eq!(
    members,
    vec![
      "PENDING_DECISION_TYPE_UNSPECIFIED",
      "PENDING_DECISION_TYPE_INQUIRY",
      "PENDING_DECISION_TYPE_AWAITING_MERGE",
      "PENDING_DECISION_TYPE_GOAL_REGISTRATION",
    ],
    "判断待ちの種類は問い合わせ・マージ待ち・ゴール登録の承認待ちである必要があります: {members:?}"
  );
}

#[test]
fn generated_dashboard_state_carries_the_read_state() {
  let state = v1::DashboardState {
    servers: vec![v1::Server {
      server_id:    "mini".to_owned(),
      name:         "j5ik2o-mac-mini".to_owned(),
      kind:         "Mac mini".to_owned(),
      connected:    false,
      last_read_at: None,
      concurrency:  4,
    }],
    thresholds: Some(v1::AnomalyThresholds { pending_decision_stale_minutes: 240, run_log_silent_minutes: 10 }),
    ..Default::default()
  };

  assert_eq!(state.servers.len(), 1);
  assert!(!state.servers[0].connected);
  assert_eq!(state.thresholds.expect("thresholds が設定されている").pending_decision_stale_minutes, 240);
}

#[test]
fn proto_declares_build_info_on_dashboard_state() {
  assert!(
    PROTO_SOURCE.lines().any(|line| line.trim().starts_with("message BuildInfo")),
    "proto に message BuildInfo を定義する必要があります"
  );
  let field = PROTO_SOURCE.lines().map(str::trim).find(|line| line.starts_with("BuildInfo build_info"));
  assert!(field.is_some(), "DashboardState のビルド情報は BuildInfo 型の build_info として定義する必要があります");
}

#[test]
fn generated_dashboard_state_carries_build_info() {
  let state = v1::DashboardState {
    build_info: Some(v1::BuildInfo { commit: "a1b2c3d".to_owned(), branch: "takt/20261116-build-info".to_owned() }),
    ..Default::default()
  };

  let info = state.build_info.expect("build_info が設定されている");
  assert_eq!(info.commit, "a1b2c3d");
  assert_eq!(info.branch, "takt/20261116-build-info");
}

#[test]
fn generated_client_exposes_the_read_and_write_calls() {
  fn client_type(
    client: v1::takt_dash_service_client::TaktDashServiceClient<tonic::transport::Channel>,
  ) -> v1::takt_dash_service_client::TaktDashServiceClient<tonic::transport::Channel> {
    client
  }
  let _: fn(
    v1::takt_dash_service_client::TaktDashServiceClient<tonic::transport::Channel>,
  ) -> v1::takt_dash_service_client::TaktDashServiceClient<tonic::transport::Channel> = client_type;

  let _request_types = (
    std::marker::PhantomData::<v1::GetOverviewRequest>,
    std::marker::PhantomData::<v1::GetProjectsRequest>,
    std::marker::PhantomData::<v1::GetPendingDecisionsRequest>,
    std::marker::PhantomData::<v1::GetActivityRequest>,
    std::marker::PhantomData::<v1::GetRunDetailRequest>,
    std::marker::PhantomData::<v1::GetConversationRequest>,
    std::marker::PhantomData::<v1::GetGoalsRequest>,
    std::marker::PhantomData::<v1::GetGoalDetailRequest>,
    std::marker::PhantomData::<v1::GetTaskDetailRequest>,
    std::marker::PhantomData::<v1::WatchStateRequest>,
    std::marker::PhantomData::<v1::AnswerInquiryRequest>,
    std::marker::PhantomData::<v1::ApproveGoalMergeRequest>,
    std::marker::PhantomData::<v1::ApproveGoalRegistrationRequest>,
    std::marker::PhantomData::<v1::RejectGoalRegistrationRequest>,
    std::marker::PhantomData::<v1::ListGoalRegistrationProposalsRequest>,
  );
  let _response_types = (
    std::marker::PhantomData::<v1::GetProjectsResponse>,
    std::marker::PhantomData::<v1::GetPendingDecisionsResponse>,
    std::marker::PhantomData::<v1::GetActivityResponse>,
    std::marker::PhantomData::<v1::GetRunDetailResponse>,
    std::marker::PhantomData::<v1::GetConversationResponse>,
    std::marker::PhantomData::<v1::GetGoalsResponse>,
    std::marker::PhantomData::<v1::GetGoalDetailResponse>,
    std::marker::PhantomData::<v1::GetTaskDetailResponse>,
    std::marker::PhantomData::<v1::AnswerInquiryResponse>,
    std::marker::PhantomData::<v1::ApproveGoalMergeResponse>,
    std::marker::PhantomData::<v1::ApproveGoalRegistrationResponse>,
    std::marker::PhantomData::<v1::RejectGoalRegistrationResponse>,
    std::marker::PhantomData::<v1::ListGoalRegistrationProposalsResponse>,
  );
}

#[test]
fn run_detail_lists_reports_and_instructions_without_their_bodies() {
  // run 詳細のレポートと指示書は一覧の項目だけを持ち、本文は GetRunContent で取得する。生成した型を
  // 本文のフィールド無しで組み立てられることで、本文が run 詳細の応答から外れていることを確かめる。
  let report = v1::RunReport { filename: "plan.md".to_owned(), size_bytes: 120 };
  let instruction =
    v1::RunInstruction { instruction_id: 1, issued_at: None, state: "pending".to_owned() };
  let removed = v1::GetRunDetailResponse { logs_removed: true, ..Default::default() };

  assert_eq!(report.filename, "plan.md");
  assert_eq!(instruction.instruction_id, 1);
  assert!(removed.logs_removed, "run 詳細はログが残っていないことを logs_removed で表します");
}

#[test]
fn run_content_request_selects_either_a_report_or_an_instruction() {
  let report = v1::GetRunContentRequest {
    run_slug: "run-1".to_owned(),
    target:   Some(v1::get_run_content_request::Target::ReportFilename("plan.md".to_owned())),
  };
  let instruction = v1::GetRunContentRequest {
    run_slug: "run-1".to_owned(),
    target:   Some(v1::get_run_content_request::Target::InstructionId(2)),
  };
  let response = v1::GetRunContentResponse {
    run_slug:   "run-1".to_owned(),
    content:    "本文".to_owned(),
    size_bytes: 6,
    truncated:  false,
  };

  assert!(matches!(report.target, Some(v1::get_run_content_request::Target::ReportFilename(_))));
  assert!(matches!(instruction.target, Some(v1::get_run_content_request::Target::InstructionId(2))));
  assert_eq!(response.content, "本文");
}

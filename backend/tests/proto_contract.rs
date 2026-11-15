//! proto/taktdash.proto の読み取り API 契約を検証する。
//!
//! このテストは、proto/ の定義本文と、そこから生成した Rust の型の両方を観測する。
//! proto を変えて生成物が追随しない状態、読み取り以外の RPC が増えた状態、
//! 判断待ちの種類が要件の2種類から外れた状態を、それぞれ失敗させる。

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
      "ApproveGoalRegistration".to_owned(),
      "ApproveMerge".to_owned(),
      "GetActivity".to_owned(),
      "GetOverview".to_owned(),
      "GetPendingDecisions".to_owned(),
      "GetProjects".to_owned(),
      "GetRunDetail".to_owned(),
      "ListGoalRegistrationProposals".to_owned(),
      "RejectGoalRegistration".to_owned(),
      "WatchState".to_owned(),
    ],
    "読み取り6つと書き込み5つの RPC が定義されている必要があります: {names:?}"
  );
}

#[test]
fn watch_state_is_the_only_server_streaming_rpc() {
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
    "AnswerInquiry",
    "ApproveMerge",
    "ListGoalRegistrationProposals",
    "ApproveGoalRegistration",
    "RejectGoalRegistration",
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
fn pending_decision_type_is_limited_to_inquiry_and_awaiting_merge() {
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
    vec!["PENDING_DECISION_TYPE_UNSPECIFIED", "PENDING_DECISION_TYPE_INQUIRY", "PENDING_DECISION_TYPE_AWAITING_MERGE",],
    "判断待ちの種類は問い合わせとマージ待ちの2種類だけにする必要があります: {members:?}"
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
fn generated_client_exposes_the_read_calls() {
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
    std::marker::PhantomData::<v1::WatchStateRequest>,
  );
  let _response_types = (
    std::marker::PhantomData::<v1::GetProjectsResponse>,
    std::marker::PhantomData::<v1::GetPendingDecisionsResponse>,
    std::marker::PhantomData::<v1::GetActivityResponse>,
    std::marker::PhantomData::<v1::GetRunDetailResponse>,
  );
}

#[test]
fn generated_client_exposes_the_write_calls() {
  let _request_types = (
    std::marker::PhantomData::<v1::AnswerInquiryRequest>,
    std::marker::PhantomData::<v1::ApproveMergeRequest>,
    std::marker::PhantomData::<v1::ListGoalRegistrationProposalsRequest>,
    std::marker::PhantomData::<v1::ApproveGoalRegistrationRequest>,
    std::marker::PhantomData::<v1::RejectGoalRegistrationRequest>,
  );
  let _response_types = (
    std::marker::PhantomData::<v1::AnswerInquiryResponse>,
    std::marker::PhantomData::<v1::ApproveMergeResponse>,
    std::marker::PhantomData::<v1::ListGoalRegistrationProposalsResponse>,
    std::marker::PhantomData::<v1::ApproveGoalRegistrationResponse>,
    std::marker::PhantomData::<v1::RejectGoalRegistrationResponse>,
  );
}

#[test]
fn generated_answer_report_covers_completed_and_unable() {
  assert_ne!(v1::AnswerReport::Unspecified as i32, v1::AnswerReport::Completed as i32);
  assert_ne!(v1::AnswerReport::Unspecified as i32, v1::AnswerReport::Unable as i32);
  assert_ne!(v1::AnswerReport::Completed as i32, v1::AnswerReport::Unable as i32);
}

#[test]
fn pending_decision_carries_the_inquiry_id() {
  // 問い合わせを一意に指せるよう、判断待ちは inquiry_id を持ちます。マージ待ちは持ちません。
  let inquiry = v1::PendingDecision {
    r#type: v1::PendingDecisionType::Inquiry as i32,
    inquiry_id: Some("q-open".to_owned()),
    ..Default::default()
  };
  let merge = v1::PendingDecision {
    r#type: v1::PendingDecisionType::AwaitingMerge as i32,
    inquiry_id: None,
    ..Default::default()
  };

  assert_eq!(inquiry.inquiry_id.as_deref(), Some("q-open"));
  assert_eq!(merge.inquiry_id, None);
}

#[test]
fn registration_proposal_carries_the_summary_fields() {
  let proposal = v1::GoalRegistrationProposal {
    revision:            3,
    server_id:           "mac-mini".to_owned(),
    project:             "repo-a".to_owned(),
    objective:           "目的A".to_owned(),
    out_of_scope:        vec!["範囲外".to_owned()],
    acceptance_criteria: vec!["受け入れ条件".to_owned()],
    constraints:         vec!["制約".to_owned()],
    integration_branch:  "takt/goal-a".to_owned(),
  };

  assert_eq!(proposal.revision, 3);
  assert_eq!(proposal.project, "repo-a");
  assert_eq!(proposal.objective, "目的A");
  assert_eq!(proposal.integration_branch, "takt/goal-a");
}

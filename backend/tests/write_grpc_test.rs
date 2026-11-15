//! 書き込み系 unary RPC（AnswerInquiry・ApproveMerge・ListGoalRegistrationProposals・
//! ApproveGoalRegistration・RejectGoalRegistration）が、状態ストアから対象プロジェクトを解決し、
//! takt-mcp の操作道具を正しい引数で1回だけ呼び、応答を proto のメッセージ型へ写すことを、
//! 実ソケットのテストサーバ・生成クライアント・mock MCP サーバの経路で検証する。

mod common;
mod grpc_harness;

use std::path::Path;

use common::*;
use grpc_harness::{client, start_server, GrpcHarness};
use serde_json::{json, Value};
use takt_dash_backend::{
  model::{ExecutionStatus, GoalStatus, InquiryEntry, ProjectSnapshot},
  takt_dash::v1::{
    AnswerInquiryRequest, AnswerReport, ApproveGoalRegistrationRequest, ApproveMergeRequest, GetOverviewRequest,
    ListGoalRegistrationProposalsRequest, PendingDecisionType, RejectGoalRegistrationRequest,
  },
};
use tonic::Code;

fn project_dir() -> &'static Path {
  Path::new("/abs/takt-dash")
}

/// 書き込みの対象になるゴールと問い合わせを持つ状態を作る。
fn seeded_with(state: Value) -> GrpcHarness {
  let harness =
    GrpcHarness::builder().single_project(project("takt-dash", project_dir()), server_meta(), state).build();
  seed(harness)
}

/// 状態ストアへ、書き込みの対象になるゴールと問い合わせを記録する。
fn seed(harness: GrpcHarness) -> GrpcHarness {
  let snapshot = ProjectSnapshot {
    goals:         vec![
      merge_goal("g-merge", 42),
      goal("g1", GoalStatus::Created, ExecutionStatus::Active),
      goal("g-open", GoalStatus::Created, ExecutionStatus::Active),
    ],
    tasks:         vec![],
    inquiries:     vec![InquiryEntry {
      goal_id: Some("g1".to_owned()),
      ..inquiry("q1", false, time("2026-11-13T07:00:00Z"))
    }],
    runs:          vec![],
    notifications: vec![],
  };
  harness.store.record_success("takt-dash", snapshot, time("2026-11-13T12:00:00Z"));
  harness
}

fn seeded() -> GrpcHarness {
  seeded_with(json!({}))
}

/// 同じサーバーに2つのプロジェクトを持つ土台を作る。書き込み先の cwd の振り分けを観測します。
fn seeded_two_projects() -> GrpcHarness {
  let harness = GrpcHarness::builder()
    .single_project(project("takt-dash", project_dir()), server_meta(), json!({}))
    .with_project(project("repo-b", Path::new("/abs/repo-b")), server_meta())
    .build();
  seed(harness)
}

/// 2つのプロジェクトに、それぞれ別のゴールと問い合わせを持つ状態を作る。
/// 先頭（`takt-dash`）と2番目（`repo-b`）でゴール ID を分け、要求したゴールのプロジェクトへ
/// 書き込みが届くかを cwd で観測できるようにします。
fn seeded_two_projects_with_repo_b() -> GrpcHarness {
  let harness = seeded_two_projects();
  let snapshot = ProjectSnapshot {
    goals:         vec![merge_goal("g-merge-b", 42), goal("g-b", GoalStatus::Created, ExecutionStatus::Active)],
    tasks:         vec![],
    inquiries:     vec![InquiryEntry {
      goal_id: Some("g-b".to_owned()),
      ..inquiry("q-b", false, time("2026-11-13T07:00:00Z"))
    }],
    runs:          vec![],
    notifications: vec![],
  };
  harness.store.record_success("repo-b", snapshot, time("2026-11-13T12:00:00Z"));
  harness
}

fn cwd_arg(call: &Value) -> &str {
  call["arguments"]["cwd"].as_str().expect("道具の cwd は文字列です")
}

fn call_names(harness: &GrpcHarness) -> Vec<String> {
  harness.calls().iter().map(|call| call["name"].as_str().expect("道具名は文字列です").to_owned()).collect()
}

// ----------------------------------------------------------------------------
// 問い合わせへの回答
// ----------------------------------------------------------------------------

#[tokio::test]
async fn answer_inquiry_calls_the_answer_tool_and_returns_the_ids() {
  let harness = seeded_two_projects_with_repo_b();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let response = client
    .answer_inquiry(AnswerInquiryRequest {
      goal_id:    "g-b".to_owned(),
      inquiry_id: "q-b".to_owned(),
      text:       "実装しました".to_owned(),
      report:     AnswerReport::Unspecified as i32,
    })
    .await
    .expect("回答を記録できます")
    .into_inner();

  assert_eq!(response.goal_id, "g-b");
  assert_eq!(response.inquiry_id, "q-b");

  let calls = harness.calls();
  assert_eq!(calls.len(), 1, "回答は道具を1回だけ呼ぶ必要があります: {calls:?}");
  assert_eq!(calls[0]["name"], "takt_answer_goal_question");
  let args = &calls[0]["arguments"];
  assert_eq!(args["goalId"], "g-b");
  assert_eq!(args["questionId"], "q-b");
  assert_eq!(args["answer"]["text"], "実装しました");
  assert!(args["answer"].get("report").is_none(), "report を指定しない要求では report を送りません: {args}");
  assert_eq!(
    args["cwd"],
    harness.project_cwd("repo-b").display().to_string(),
    "回答はゴールが属するプロジェクトの cwd へ送ります: {args}"
  );
  assert_ne!(
    args["cwd"],
    harness.project_cwd("takt-dash").display().to_string(),
    "先頭プロジェクトの cwd へ送ってはいけません: {args}"
  );

  server.stop().await;
}

#[tokio::test]
async fn answer_inquiry_maps_the_report_enum_to_the_tool_argument() {
  let harness = seeded();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  for (report, expected) in [(AnswerReport::Completed, "completed"), (AnswerReport::Unable, "unable")] {
    client
      .answer_inquiry(AnswerInquiryRequest {
        goal_id:    "g1".to_owned(),
        inquiry_id: "q1".to_owned(),
        text:       String::new(),
        report:     report as i32,
      })
      .await
      .expect("回答を記録できます");
    let calls = harness.calls();
    let last = calls.last().expect("道具の呼び出しが記録されています");
    assert_eq!(last["arguments"]["answer"]["report"], expected, "report を道具の語彙へ写します: {last}");
  }

  server.stop().await;
}

#[tokio::test]
async fn answer_inquiry_rejects_an_unknown_goal() {
  let harness = seeded();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let status = client
    .answer_inquiry(AnswerInquiryRequest {
      goal_id:    "g-missing".to_owned(),
      inquiry_id: "q1".to_owned(),
      text:       "実装しました".to_owned(),
      report:     AnswerReport::Unspecified as i32,
    })
    .await
    .expect_err("状態ストアに無いゴールは回答できません");

  assert_eq!(status.code(), Code::NotFound);
  assert!(harness.calls().is_empty(), "対象を解決できなければ道具を呼びません: {:?}", harness.calls());

  server.stop().await;
}

#[tokio::test]
async fn answer_inquiry_rejects_invalid_entry_values_before_calling_tools() {
  let harness = seeded();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let cases = [
    AnswerInquiryRequest {
      goal_id:    " ".to_owned(),
      inquiry_id: "q1".to_owned(),
      text:       "実装しました".to_owned(),
      report:     AnswerReport::Unspecified as i32,
    },
    AnswerInquiryRequest {
      goal_id:    "g1".to_owned(),
      inquiry_id: String::new(),
      text:       "実装しました".to_owned(),
      report:     AnswerReport::Unspecified as i32,
    },
    AnswerInquiryRequest {
      goal_id:    "g1".to_owned(),
      inquiry_id: "q1".to_owned(),
      text:       "実装しました".to_owned(),
      report:     99,
    },
    AnswerInquiryRequest {
      goal_id:    "g1".to_owned(),
      inquiry_id: "q1".to_owned(),
      text:       "  ".to_owned(),
      report:     AnswerReport::Unspecified as i32,
    },
  ];

  for request in cases {
    let status = client.answer_inquiry(request.clone()).await.expect_err("入口の検証で回答を拒否します");
    assert_eq!(status.code(), Code::InvalidArgument, "不正な入口の値は InvalidArgument で拒否します: {request:?}");
    assert!(harness.calls().is_empty(), "入口の検証で拒否した要求は道具を呼びません: {:?}", harness.calls());
  }

  server.stop().await;
}

// ----------------------------------------------------------------------------
// プルリクエストのマージ承認
// ----------------------------------------------------------------------------

#[tokio::test]
async fn approve_merge_calls_the_merge_tool_until_merge_and_returns_the_task_name() {
  let harness = seeded_two_projects_with_repo_b();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let response = client
    .approve_merge(ApproveMergeRequest { goal_id: "g-merge-b".to_owned() })
    .await
    .expect("マージを依頼できます")
    .into_inner();

  assert_eq!(response.goal_id, "g-merge-b");
  assert_eq!(response.pr_number, 42, "PR 番号は状態ストアのゴールから解決します");
  assert_eq!(response.task_name, "仕上げタスク", "道具が返したタスク名を写します");

  let calls = harness.calls();
  assert_eq!(calls.len(), 1);
  assert_eq!(calls[0]["name"], "takt_merge_goal_pr");
  let args = &calls[0]["arguments"];
  assert_eq!(args["goalId"], "g-merge-b");
  assert_eq!(args["prNumber"], 42);
  assert_eq!(args["until"], "merge", "マージ承認は until=merge で実行します");
  assert_eq!(
    cwd_arg(&calls[0]),
    harness.project_cwd("repo-b").display().to_string(),
    "マージ依頼はゴールが属するプロジェクトの cwd へ送ります"
  );
  assert_ne!(
    cwd_arg(&calls[0]),
    harness.project_cwd("takt-dash").display().to_string(),
    "先頭プロジェクトの cwd へ送ってはいけません"
  );

  server.stop().await;
}

#[tokio::test]
async fn approve_merge_rejects_a_goal_without_a_pull_request() {
  let harness = seeded();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let status = client
    .approve_merge(ApproveMergeRequest { goal_id: "g-open".to_owned() })
    .await
    .expect_err("PR の無いゴールはマージできません");

  assert!(
    matches!(status.code(), Code::InvalidArgument | Code::FailedPrecondition),
    "PR の無いゴールは前提不一致として拒否し、接続不能の Unavailable にしません: {}",
    status.code()
  );
  assert!(harness.calls().is_empty(), "PR を解決できなければ道具を呼びません: {:?}", harness.calls());

  server.stop().await;
}

#[tokio::test]
async fn approve_merge_rejects_an_empty_goal_id_before_calling_tools() {
  let harness = seeded();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let status = client
    .approve_merge(ApproveMergeRequest { goal_id: String::new() })
    .await
    .expect_err("空のゴール ID はマージできません");

  assert_eq!(status.code(), Code::InvalidArgument, "空の goalId は入口で拒否します");
  assert!(harness.calls().is_empty(), "入口の検証で拒否した承認は道具を呼びません: {:?}", harness.calls());

  server.stop().await;
}

// ----------------------------------------------------------------------------
// ゴール登録の承認待ち一覧
// ----------------------------------------------------------------------------

fn registration_state() -> Value {
  json!({
    "registrationProposals": [{
      "revision": 3,
      "repositoryDirectory": "/abs/repo-a",
      "summary": {
        "objective": "目的A",
        "outOfScope": ["範囲外A"],
        "acceptanceCriteria": ["条件A"],
        "constraints": ["制約A"],
        "integrationBranch": "takt/goal-a"
      }
    }]
  })
}

#[tokio::test]
async fn list_goal_registration_proposals_aggregates_the_configured_projects() {
  let harness = GrpcHarness::builder()
    .single_project(project("repo-a", Path::new("/abs/repo-a")), server_meta(), registration_state())
    .with_project(project("repo-b", Path::new("/abs/repo-b")), server_meta())
    .build();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let response = client
    .list_goal_registration_proposals(ListGoalRegistrationProposalsRequest { project: String::new() })
    .await
    .expect("承認待ちを取得できます")
    .into_inner();

  assert_eq!(
    call_names(&harness),
    vec!["takt_list_goal_registration_proposals".to_owned(); 2],
    "設定の全プロジェクトを問い合わせます"
  );
  let calls = harness.calls();
  assert_eq!(
    cwd_arg(&calls[0]),
    harness.project_cwd("repo-a").display().to_string(),
    "設定順の先頭プロジェクトの cwd を使います"
  );
  assert_eq!(
    cwd_arg(&calls[1]),
    harness.project_cwd("repo-b").display().to_string(),
    "設定順の2番目のプロジェクトの cwd を使います"
  );
  assert_eq!(response.proposals.len(), 2);
  let projects: Vec<&str> = response.proposals.iter().map(|proposal| proposal.project.as_str()).collect();
  assert_eq!(projects, vec!["repo-a", "repo-b"]);
  let first = &response.proposals[0];
  assert_eq!(first.revision, 3);
  assert_eq!(first.server_id, "mac-mini");
  assert_eq!(first.objective, "目的A");
  assert_eq!(first.out_of_scope, vec!["範囲外A".to_owned()]);
  assert_eq!(first.acceptance_criteria, vec!["条件A".to_owned()]);
  assert_eq!(first.constraints, vec!["制約A".to_owned()]);
  assert_eq!(first.integration_branch, "takt/goal-a");

  server.stop().await;
}

#[tokio::test]
async fn list_goal_registration_proposals_honours_the_project_filter() {
  let harness = GrpcHarness::builder()
    .single_project(project("repo-a", Path::new("/abs/repo-a")), server_meta(), registration_state())
    .with_project(project("repo-b", Path::new("/abs/repo-b")), server_meta())
    .build();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let response = client
    .list_goal_registration_proposals(ListGoalRegistrationProposalsRequest { project: "repo-b".to_owned() })
    .await
    .expect("承認待ちを取得できます")
    .into_inner();

  assert_eq!(
    call_names(&harness),
    vec!["takt_list_goal_registration_proposals".to_owned()],
    "指定した1プロジェクトだけを問い合わせます"
  );
  assert_eq!(response.proposals.len(), 1);
  assert_eq!(response.proposals[0].project, "repo-b");

  server.stop().await;
}

#[tokio::test]
async fn list_goal_registration_proposals_rejects_an_unknown_project() {
  let harness = seeded();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let status = client
    .list_goal_registration_proposals(ListGoalRegistrationProposalsRequest { project: "unknown".to_owned() })
    .await
    .expect_err("設定に無いプロジェクトは指定できません");

  assert!(
    matches!(status.code(), Code::InvalidArgument | Code::NotFound),
    "設定に無いプロジェクトは要求の誤りとして拒否します: {}",
    status.code()
  );
  assert!(harness.calls().is_empty());

  server.stop().await;
}

// ----------------------------------------------------------------------------
// ゴール登録の承認・却下
// ----------------------------------------------------------------------------

#[tokio::test]
async fn approve_goal_registration_calls_the_tool_and_returns_the_goal() {
  let harness = seeded_two_projects();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let response = client
    .approve_goal_registration(ApproveGoalRegistrationRequest { project: "repo-b".to_owned(), revision: 3 })
    .await
    .expect("承認できます")
    .into_inner();

  assert_eq!(response.revision, 3);
  assert_eq!(response.goal_id, "g-approved");
  assert_eq!(response.objective, "承認した目的");

  let calls = harness.calls();
  assert_eq!(calls.len(), 1);
  assert_eq!(calls[0]["name"], "takt_approve_goal_registration");
  assert_eq!(calls[0]["arguments"]["revision"], 3);
  assert_eq!(
    cwd_arg(&calls[0]),
    harness.project_cwd("repo-b").display().to_string(),
    "要求したプロジェクトの cwd へ送ります"
  );
  assert_ne!(
    cwd_arg(&calls[0]),
    harness.project_cwd("takt-dash").display().to_string(),
    "先頭プロジェクトの cwd へ送ってはいけません"
  );

  server.stop().await;
}

#[tokio::test]
async fn reject_goal_registration_calls_the_tool_and_returns_the_notice() {
  let harness = seeded_two_projects();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let response = client
    .reject_goal_registration(RejectGoalRegistrationRequest { project: "repo-b".to_owned(), revision: 5 })
    .await
    .expect("却下できます")
    .into_inner();

  assert_eq!(response.revision, 5);
  assert_eq!(response.notice, "登録を取り下げました。");

  let calls = harness.calls();
  assert_eq!(calls.len(), 1);
  assert_eq!(calls[0]["name"], "takt_reject_goal_registration");
  assert_eq!(calls[0]["arguments"]["revision"], 5);
  assert_eq!(
    cwd_arg(&calls[0]),
    harness.project_cwd("repo-b").display().to_string(),
    "要求したプロジェクトの cwd へ送ります"
  );
  assert_ne!(
    cwd_arg(&calls[0]),
    harness.project_cwd("takt-dash").display().to_string(),
    "先頭プロジェクトの cwd へ送ってはいけません"
  );

  server.stop().await;
}

#[tokio::test]
async fn approve_goal_registration_rejects_a_non_positive_revision() {
  let harness = seeded();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let status = client
    .approve_goal_registration(ApproveGoalRegistrationRequest { project: "takt-dash".to_owned(), revision: 0 })
    .await
    .expect_err("リビジョンは正の整数である必要があります");

  assert_eq!(status.code(), Code::InvalidArgument);
  assert!(harness.calls().is_empty());

  server.stop().await;
}

#[tokio::test]
async fn registration_commands_reject_non_positive_revisions_before_calling_tools() {
  let harness = seeded();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  for revision in [0, -1] {
    let status = client
      .reject_goal_registration(RejectGoalRegistrationRequest { project: "takt-dash".to_owned(), revision })
      .await
      .expect_err("リビジョンは正の整数である必要があります");
    assert_eq!(status.code(), Code::InvalidArgument, "revision={revision} は入口で拒否します");
    assert!(harness.calls().is_empty(), "入口の検証で拒否した却下は道具を呼びません: {revision}");
  }

  let status = client
    .approve_goal_registration(ApproveGoalRegistrationRequest { project: "takt-dash".to_owned(), revision: -1 })
    .await
    .expect_err("リビジョンは正の整数である必要があります");
  assert_eq!(status.code(), Code::InvalidArgument, "revision=-1 は入口で拒否します");
  assert!(harness.calls().is_empty(), "入口の検証で拒否した承認は道具を呼びません");

  server.stop().await;
}

#[tokio::test]
async fn registration_commands_reject_an_unknown_project_without_calling_tools() {
  let harness = seeded_two_projects();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let approve = client
    .approve_goal_registration(ApproveGoalRegistrationRequest { project: "unknown".to_owned(), revision: 3 })
    .await
    .expect_err("設定に無いプロジェクトは承認できません");
  assert_eq!(approve.code(), Code::InvalidArgument);
  assert!(harness.calls().is_empty(), "対象を解決できなければ道具を呼びません: {:?}", harness.calls());

  let reject = client
    .reject_goal_registration(RejectGoalRegistrationRequest { project: "unknown".to_owned(), revision: 3 })
    .await
    .expect_err("設定に無いプロジェクトは却下できません");
  assert_eq!(reject.code(), Code::InvalidArgument);
  assert!(harness.calls().is_empty(), "対象を解決できなければ道具を呼びません: {:?}", harness.calls());

  server.stop().await;
}

// ----------------------------------------------------------------------------
// 失敗の写像
// ----------------------------------------------------------------------------

#[tokio::test]
async fn tool_failure_maps_to_failed_precondition() {
  let harness = seeded_with(json!({ "isError": ["takt_answer_goal_question"] }));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let status = client
    .answer_inquiry(AnswerInquiryRequest {
      goal_id:    "g1".to_owned(),
      inquiry_id: "q1".to_owned(),
      text:       "実装しました".to_owned(),
      report:     AnswerReport::Unspecified as i32,
    })
    .await
    .expect_err("道具が失敗したら回答は失敗します");

  assert_eq!(
    status.code(),
    Code::FailedPrecondition,
    "道具の失敗は FailedPrecondition へ写し、接続不能の Unavailable と混ぜません"
  );

  server.stop().await;
}

// ----------------------------------------------------------------------------
// 回答に必要な問い合わせ ID の投影
// ----------------------------------------------------------------------------

#[tokio::test]
async fn pending_decisions_expose_the_inquiry_id_for_answering() {
  let harness = seeded();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let state = client.get_overview(GetOverviewRequest {}).await.expect("概要を取得できます").into_inner();

  let inquiry = state
    .pending_decisions
    .iter()
    .find(|decision| decision.r#type == PendingDecisionType::Inquiry as i32)
    .expect("未回答の問い合わせ");
  assert_eq!(inquiry.inquiry_id.as_deref(), Some("q1"), "問い合わせを一意に指せる ID を公開します");
  let merge = state
    .pending_decisions
    .iter()
    .find(|decision| decision.r#type == PendingDecisionType::AwaitingMerge as i32)
    .expect("人のマージ待ち");
  assert_eq!(merge.inquiry_id, None, "マージ待ちは問い合わせ ID を持ちません");

  server.stop().await;
}

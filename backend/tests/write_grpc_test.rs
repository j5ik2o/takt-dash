//! gRPC の書き込み系 unary RPC（AnswerInquiry・ApproveGoalMerge・ApproveGoalRegistration・
//! RejectGoalRegistration・ListGoalRegistrationProposals）が、設定から対象サーバーの起動方法と cwd
//! を 解決して takt-mcp の道具を呼び、その引数と応答を proto のメッセージ型どおりに扱うことを、
//! 実ソケットのテストサーバと生成クライアントの経路で検証する。
//!
//! takt-mcp は mock MCP サーバで代替する。mock は実 takt-mcp の入力契約と同じく cwd 必須・
//! 未知キー拒否で検証し、受け取った引数を記録する。テストは記録された引数を直接観測して、
//! backend が組み立てた引数（フィールド名・型・nested 構造）を検証する。

mod common;
mod grpc_harness;

use std::path::Path;

use common::*;
use grpc_harness::{client, start_server, GrpcHarness};
use serde_json::{json, Value};
use takt_dash_backend::{
  domain::{GoalId, InquiryId},
  model::{EventKind, ExecutionStatus, InquiryEntry, InquiryKind, ProjectSnapshot},
  poller::Poller,
  takt_dash::v1::{
    AnswerInquiryRequest, ApproveGoalMergeRequest, ApproveGoalRegistrationRequest, GetPendingDecisionsRequest,
    ListGoalRegistrationProposalsRequest, OperationAnswerReport, PendingDecisionType, RejectGoalRegistrationRequest,
    WatchStateRequest,
  },
};
use tokio::time::{timeout, Duration};
use tonic::Code;

/// 1プロジェクト分の土台を作る。`state` は mock が返す状態です。
fn harness_with(state: serde_json::Value) -> GrpcHarness {
  GrpcHarness::builder()
    .single_project(project("takt-dash", std::path::Path::new("/abs/takt-dash")), server_meta(), state)
    .build()
}

/// 記録された道具呼び出しのうち、指定した名前のものだけを返す。
fn calls_named(harness: &GrpcHarness, name: &str) -> Vec<serde_json::Value> {
  harness.recorded_calls().into_iter().filter(|call| call.name == name).map(|call| call.arguments).collect()
}

#[tokio::test]
async fn pending_decision_inquiry_id_is_the_identifier_used_for_answering() {
  // 判断待ち一覧が示す識別子が、そのまま回答で takt の questionId として渡ることを、
  // 一覧の取得から回答まで一続きに観測する。
  let harness = harness_with(json!({}));
  harness.store.record_success(
    "takt-dash",
    ProjectSnapshot {
      goals: vec![],
      tasks: vec![],
      inquiries: vec![InquiryEntry {
        inquiry_id: InquiryId::try_from("8f14e45f-ceea-4f6a-9a3c-0f1a2b3c4d5e")
          .expect("テストの inquiry ID は有効です"),
        goal_id:    Some(
          GoalId::try_from("3f9c1a54-2b41-4d1e-9d2e-2c1a5b8f7e11").expect("テストのゴール ID は有効です"),
        ),
        kind:       InquiryKind::Question,
        body:       "続けてよいですか".to_owned(),
        options:    vec!["はい".to_owned(), "いいえ".to_owned()],
        answered:   false,
        created_at: time("2026-11-13T07:00:00Z"),
      }],
      runs: vec![],
      notifications: vec![],
      proposals: vec![],
      ..ProjectSnapshot::empty()
    },
    time("2026-11-13T12:00:00Z"),
  );
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let state = client
    .get_pending_decisions(GetPendingDecisionsRequest {})
    .await
    .expect("判断待ちを取得できます")
    .into_inner()
    .state
    .expect("状態があります");
  let inquiry = state
    .pending_decisions
    .iter()
    .find(|decision| decision.r#type == PendingDecisionType::Inquiry as i32)
    .expect("未回答の問い合わせ");
  assert_eq!(
    inquiry.inquiry_id, "8f14e45f-ceea-4f6a-9a3c-0f1a2b3c4d5e",
    "判断待ちの識別子は takt の questionId と同じ値です"
  );

  client
    .answer_inquiry(AnswerInquiryRequest {
      idempotency_key: None,
      server_id:       "mac-mini".to_owned(),
      project:         "takt-dash".to_owned(),
      goal_id:         Some("3f9c1a54-2b41-4d1e-9d2e-2c1a5b8f7e11".to_owned()),
      inquiry_id:      inquiry.inquiry_id.clone(),
      text:            Some("はい".to_owned()),
      report:          None,
    })
    .await
    .expect("一覧の識別子で回答できます");

  let calls = calls_named(&harness, "takt_answer_goal_question");
  assert_eq!(calls.len(), 1);
  assert_eq!(
    calls[0]["questionId"],
    json!("8f14e45f-ceea-4f6a-9a3c-0f1a2b3c4d5e"),
    "一覧の識別子がそのまま questionId として渡ります"
  );

  server.stop().await;
}

#[tokio::test]
async fn answered_inquiry_is_absent_from_pending_decisions() {
  let harness = harness_with(json!({}));
  harness.store.record_success(
    "takt-dash",
    ProjectSnapshot {
      goals: vec![],
      tasks: vec![],
      inquiries: vec![InquiryEntry {
        inquiry_id: InquiryId::try_from("answered-id").expect("テストの inquiry ID は有効です"),
        goal_id:    Some(
          GoalId::try_from("3f9c1a54-2b41-4d1e-9d2e-2c1a5b8f7e11").expect("テストのゴール ID は有効です"),
        ),
        kind:       InquiryKind::Question,
        body:       "回答済み".to_owned(),
        options:    vec![],
        answered:   true,
        created_at: time("2026-11-13T07:00:00Z"),
      }],
      runs: vec![],
      notifications: vec![],
      proposals: vec![],
      ..ProjectSnapshot::empty()
    },
    time("2026-11-13T12:00:00Z"),
  );
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let state = client
    .get_pending_decisions(GetPendingDecisionsRequest {})
    .await
    .expect("判断待ちを取得できます")
    .into_inner()
    .state
    .expect("状態があります");
  assert!(
    !state.pending_decisions.iter().any(|decision| decision.inquiry_id == "answered-id"),
    "回答済みの問い合わせは判断待ちに出ません: {:?}",
    state.pending_decisions.iter().map(|decision| decision.inquiry_id.as_str()).collect::<Vec<_>>()
  );

  server.stop().await;
}

#[tokio::test]
async fn answer_inquiry_sends_free_text_in_the_answer_object() {
  let harness = harness_with(json!({}));
  harness.store.record_success(
    "takt-dash",
    snapshot_with(vec![inquiry_entry(
      "8f14e45f-ceea-4f6a-9a3c-0f1a2b3c4d5e",
      "11111111-1111-4111-8111-111111111111",
      InquiryKind::Question,
    )]),
    time("2026-11-13T12:00:00Z"),
  );
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let response = client
    .answer_inquiry(AnswerInquiryRequest {
      idempotency_key: None,
      server_id:       "mac-mini".to_owned(),
      project:         "takt-dash".to_owned(),
      goal_id:         Some("3f9c1a54-2b41-4d1e-9d2e-2c1a5b8f7e11".to_owned()),
      inquiry_id:      "8f14e45f-ceea-4f6a-9a3c-0f1a2b3c4d5e".to_owned(),
      text:            Some("はい".to_owned()),
      report:          None,
    })
    .await
    .expect("回答を受け付けられます")
    .into_inner();

  assert_eq!(response.inquiry_id, "8f14e45f-ceea-4f6a-9a3c-0f1a2b3c4d5e", "応答の識別子をそのまま返します");

  let calls = calls_named(&harness, "takt_answer_goal_question");
  assert_eq!(calls.len(), 1, "回答の道具を1回だけ呼びます: {calls:?}");
  let arguments = &calls[0];
  assert_eq!(
    arguments["goalId"],
    json!("3f9c1a54-2b41-4d1e-9d2e-2c1a5b8f7e11"),
    "一致1件では要求が明示したゴールを優先します"
  );
  assert_eq!(arguments["questionId"], json!("8f14e45f-ceea-4f6a-9a3c-0f1a2b3c4d5e"));
  assert_eq!(arguments["answer"], json!({ "text": "はい" }), "answer はオブジェクトで text を持つ必要があります");

  server.stop().await;
}

#[tokio::test]
async fn answer_inquiry_sends_unable_for_an_operation_request() {
  let harness = harness_with(json!({}));
  harness.store.record_success(
    "takt-dash",
    snapshot_with(vec![inquiry_entry(
      "8f14e45f-ceea-4f6a-9a3c-0f1a2b3c4d5e",
      "3f9c1a54-2b41-4d1e-9d2e-2c1a5b8f7e11",
      InquiryKind::OperationRequest,
    )]),
    time("2026-11-13T12:00:00Z"),
  );
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  client
    .answer_inquiry(AnswerInquiryRequest {
      idempotency_key: None,
      server_id:       "mac-mini".to_owned(),
      project:         "takt-dash".to_owned(),
      goal_id:         Some("3f9c1a54-2b41-4d1e-9d2e-2c1a5b8f7e11".to_owned()),
      inquiry_id:      "8f14e45f-ceea-4f6a-9a3c-0f1a2b3c4d5e".to_owned(),
      text:            None,
      report:          Some(OperationAnswerReport::Unable as i32),
    })
    .await
    .expect("対応できないの回答を受け付けられます");

  let calls = calls_named(&harness, "takt_answer_goal_question");
  assert_eq!(calls.len(), 1);
  assert_eq!(
    calls[0]["answer"],
    json!({ "report": "unable" }),
    "operation_request は report に unable を入れて送ります"
  );

  server.stop().await;
}

#[tokio::test]
async fn answer_inquiry_rejects_empty_inquiry_id_without_calling_takt() {
  let harness = harness_with(json!({}));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let status = client
    .answer_inquiry(AnswerInquiryRequest {
      idempotency_key: None,
      server_id:       "mac-mini".to_owned(),
      project:         "takt-dash".to_owned(),
      goal_id:         Some("3f9c1a54-2b41-4d1e-9d2e-2c1a5b8f7e11".to_owned()),
      inquiry_id:      String::new(),
      text:            Some("はい".to_owned()),
      report:          None,
    })
    .await
    .expect_err("空の識別子は拒否されます")
    .code();

  assert_eq!(status, Code::InvalidArgument, "空の識別子は入力の誤りとして拒否します");
  assert!(calls_named(&harness, "takt_answer_goal_question").is_empty(), "入力の誤りでは takt を呼びません");

  server.stop().await;
}

#[tokio::test]
async fn answer_inquiry_rejects_an_answer_without_text_or_report() {
  let harness = harness_with(json!({}));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let status = client
    .answer_inquiry(AnswerInquiryRequest {
      idempotency_key: None,
      server_id:       "mac-mini".to_owned(),
      project:         "takt-dash".to_owned(),
      goal_id:         Some("3f9c1a54-2b41-4d1e-9d2e-2c1a5b8f7e11".to_owned()),
      inquiry_id:      "8f14e45f-ceea-4f6a-9a3c-0f1a2b3c4d5e".to_owned(),
      text:            None,
      report:          None,
    })
    .await
    .expect_err("text も report も無い回答は拒否されます")
    .code();

  assert_eq!(status, Code::InvalidArgument);
  assert!(calls_named(&harness, "takt_answer_goal_question").is_empty());

  server.stop().await;
}

#[tokio::test]
async fn approve_goal_merge_sends_goal_and_pr_with_until() {
  let harness = harness_with(json!({}));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let response = client
    .approve_goal_merge(ApproveGoalMergeRequest {
      idempotency_key: None,
      server_id:       "mac-mini".to_owned(),
      project:         "takt-dash".to_owned(),
      goal_id:         "g-merge".to_owned(),
      pr_number:       42,
      until:           "merge".to_owned(),
    })
    .await
    .expect("マージ承認を受け付けられます")
    .into_inner();

  assert_eq!(response.task_name, "merge", "takt が返したタスク名を返します");

  let calls = calls_named(&harness, "takt_merge_goal_pr");
  assert_eq!(calls.len(), 1);
  let arguments = &calls[0];
  assert_eq!(arguments["goalId"], json!("g-merge"));
  assert_eq!(arguments["prNumber"], json!(42));
  assert_eq!(arguments["until"], json!("merge"), "until は必須で、そのまま渡します");

  server.stop().await;
}

#[tokio::test]
async fn approve_goal_merge_rejects_a_non_positive_pr_number() {
  let harness = harness_with(json!({}));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let status = client
    .approve_goal_merge(ApproveGoalMergeRequest {
      idempotency_key: None,
      server_id:       "mac-mini".to_owned(),
      project:         "takt-dash".to_owned(),
      goal_id:         "g-merge".to_owned(),
      pr_number:       0,
      until:           "merge".to_owned(),
    })
    .await
    .expect_err("0 の PR 番号は拒否されます")
    .code();

  assert_eq!(status, Code::InvalidArgument);
  assert!(calls_named(&harness, "takt_merge_goal_pr").is_empty());

  server.stop().await;
}

#[tokio::test]
async fn approve_goal_registration_sends_the_revision() {
  let harness = harness_with(json!({}));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let response = client
    .approve_goal_registration(ApproveGoalRegistrationRequest {
      idempotency_key: None,
      server_id:       "mac-mini".to_owned(),
      project:         "takt-dash".to_owned(),
      revision:        3,
    })
    .await
    .expect("承認を受け付けられます")
    .into_inner();

  assert_eq!(response.revision, 3);

  let calls = calls_named(&harness, "takt_approve_goal_registration");
  assert_eq!(calls.len(), 1);
  assert_eq!(calls[0]["revision"], json!(3), "revision は正整数で渡します");

  server.stop().await;
}

#[tokio::test]
async fn approve_goal_registration_rejects_control_characters_in_the_target_without_calling_takt() {
  for (server_id, project) in [("mac-mini\n", "takt-dash"), ("mac-mini", "takt-dash\u{0}")] {
    let harness = harness_with(json!({}));
    let server = start_server(&harness).await;
    let mut client = client(server.address).await;

    let status = client
      .approve_goal_registration(ApproveGoalRegistrationRequest {
        idempotency_key: None,
        server_id:       server_id.to_owned(),
        project:         project.to_owned(),
        revision:        3,
      })
      .await
      .expect_err("制御文字を含む server_id / project は拒否されます")
      .code();

    assert_eq!(
      status,
      Code::InvalidArgument,
      "server_id={server_id:?} project={project:?} は入力の誤りとして拒否します"
    );
    assert!(calls_named(&harness, "takt_approve_goal_registration").is_empty(), "入力の誤りでは takt を呼びません");

    server.stop().await;
  }
}

#[tokio::test]
async fn reject_goal_registration_calls_reject_and_not_approve() {
  let harness = harness_with(json!({}));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let response = client
    .reject_goal_registration(RejectGoalRegistrationRequest {
      idempotency_key: None,
      server_id:       "mac-mini".to_owned(),
      project:         "takt-dash".to_owned(),
      revision:        3,
    })
    .await
    .expect("却下を受け付けられます")
    .into_inner();

  assert_eq!(response.revision, 3);

  let reject_calls = calls_named(&harness, "takt_reject_goal_registration");
  assert_eq!(reject_calls.len(), 1);
  assert_eq!(reject_calls[0]["revision"], json!(3));
  assert!(calls_named(&harness, "takt_approve_goal_registration").is_empty(), "却下で承認の道具を呼びません");

  server.stop().await;
}

#[tokio::test]
async fn reject_goal_registration_rejects_a_non_positive_revision() {
  let harness = harness_with(json!({}));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let status = client
    .reject_goal_registration(RejectGoalRegistrationRequest {
      idempotency_key: None,
      server_id:       "mac-mini".to_owned(),
      project:         "takt-dash".to_owned(),
      revision:        0,
    })
    .await
    .expect_err("0 の revision は拒否されます")
    .code();

  assert_eq!(status, Code::InvalidArgument);
  assert!(calls_named(&harness, "takt_reject_goal_registration").is_empty());

  server.stop().await;
}

#[tokio::test]
async fn list_goal_registration_proposals_projects_the_pending_list_shape() {
  let harness = harness_with(json!({}));
  // 承認待ちは専用 RPC がストアのみを読む。poller の1サイクルで mock の proposal を
  // ストアの snapshot へ取り込んでから、一覧の射影を観測する。
  let mut poller = Poller::new(
    harness.config_path().to_path_buf(),
    harness.activity.clone(),
    harness.store.clone(),
    harness.changes.clone(),
    harness.registry.clone(),
  );
  harness.rewrite_state(json!({
    "proposals": [
      { "revision": 3, "repositoryDirectory": "/abs/takt-dash", "summary": "新しいゴール" }
    ]
  }));
  poller.read_cycle_once().await.expect("承認待ちを取り込むサイクルを実行できます");

  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let response = client
    .list_goal_registration_proposals(ListGoalRegistrationProposalsRequest {
      server_id: "mac-mini".to_owned(),
      project:   "takt-dash".to_owned(),
    })
    .await
    .expect("承認待ちを取得できます")
    .into_inner();

  assert_eq!(response.proposals.len(), 1, "承認待ちの1件を返します");
  let proposal = &response.proposals[0];
  assert_eq!(proposal.revision, 3);
  assert_eq!(proposal.server_id, "mac-mini", "引数のサーバーを判断待ち一覧と同じ形で写します");
  assert_eq!(proposal.project, "takt-dash", "引数のプロジェクトを同じ形で写します");
  assert!(proposal.body.contains("新しいゴール"), "本文に提案の要約を含みます: {}", proposal.body);

  poller.shutdown().await;
  server.stop().await;
}

/// mock の状態に承認待ち（proposals）を置いた土台で poller を1サイクル回し、
/// ストアへ取り込んだ poller を返す。
async fn polled_harness(state: serde_json::Value) -> (GrpcHarness, Poller) {
  let harness = harness_with(json!({}));
  let mut poller = Poller::new(
    harness.config_path().to_path_buf(),
    harness.activity.clone(),
    harness.store.clone(),
    harness.changes.clone(),
    harness.registry.clone(),
  );
  harness.rewrite_state(state);
  poller.read_cycle_once().await.expect("状態を取り込むサイクルを実行できます");
  (harness, poller)
}

/// approval-1 を提案プールに持つ mock の状態。
fn state_with_proposal() -> serde_json::Value {
  json!({
    "proposals": [
      { "revision": 3, "repositoryDirectory": "/abs/takt-dash", "summary": "新しいゴール" }
    ]
  })
}

/// 実 takt-mcp の応答形（summary がオブジェクト）を持つ承認待ち1件の mock の状態。
fn state_with_object_summary_proposal() -> serde_json::Value {
  json!({
    "proposals": [
      {
        "revision": 6,
        "repositoryDirectory": "/abs/takt-dash",
        "summary": {
          "objective": "稼働状況を一覧できるようにする",
          "outOfScope": ["認証"],
          "acceptanceCriteria": ["判断待ちに表示される"],
          "constraints": ["proto を変えない"],
          "startBranch": "main",
          "integrationBranch": "takt/goal-6"
        }
      }
    ]
  })
}

/// goal_id を持たない未回答の問い合わせ1件だけを持つ mock の状態。
fn state_with_standalone_inquiry() -> serde_json::Value {
  json!({
    "standaloneInquiries": [
      {
        "id": "8f14e45f-ceea-4f6a-9a3c-0f1a2b3c4d5e",
        "kind": "question",
        "status": "pending",
        "body": "続けてよいですか",
        "options": ["はい", "いいえ"],
        "createdAt": "2026-11-13T07:00:00Z"
      }
    ]
  })
}

/// ゴールに属する未回答の問い合わせ（種類は `kind`）1件を持つ mock の状態。
/// takt_answer_goal_question は goalId
/// 必須のため、種別検証の観測はゴールを持つ問い合わせで行います。
fn state_with_goal_inquiry(kind: &str) -> serde_json::Value {
  json!({
    "goals": [
      {
        "id": "3f9c1a54-2b41-4d1e-9d2e-2c1a5b8f7e11",
        "status": "created",
        "executionStatus": "active",
        "objective": "確認の対象",
        "integrationBranch": "takt/confirm"
      }
    ],
    "goalQuestions": {
      "3f9c1a54-2b41-4d1e-9d2e-2c1a5b8f7e11": [
        {
          "id": "8f14e45f-ceea-4f6a-9a3c-0f1a2b3c4d5e",
          "kind": kind,
          "status": "pending",
          "body": "確認をお願いします",
          "createdAt": "2026-11-13T07:00:00Z"
        }
      ]
    }
  })
}

#[tokio::test]
async fn goal_registration_proposal_appears_in_pending_decisions() {
  // SCN-U1-P1: 承認待ちが判断待ち一覧に現れ、専用 RPC と同じ server/project/本文を持つ。
  let (harness, mut poller) = polled_harness(state_with_proposal()).await;
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let state = client
    .get_pending_decisions(GetPendingDecisionsRequest {})
    .await
    .expect("判断待ちを取得できます")
    .into_inner()
    .state
    .expect("状態があります");
  let proposals: Vec<_> =
    state.pending_decisions.iter().filter(|d| d.r#type == PendingDecisionType::GoalRegistration as i32).collect();
  assert_eq!(proposals.len(), 1, "承認待ちが1件だけ一覧に現れます");
  let proposal = proposals[0];
  assert_eq!(proposal.server_id, "mac-mini");
  assert_eq!(proposal.project, "takt-dash");
  assert_eq!(proposal.revision, Some(3), "承認待ちの revision を写します");
  assert!(proposal.body.contains("新しいゴール"), "本文に提案の要約を含みます: {}", proposal.body);

  // 専用 RPC と同じ server/project/本文であることを、同じストアの射影同士で確認する。
  let listed = client
    .list_goal_registration_proposals(ListGoalRegistrationProposalsRequest {
      server_id: "mac-mini".to_owned(),
      project:   "takt-dash".to_owned(),
    })
    .await
    .expect("承認待ちを取得できます")
    .into_inner()
    .proposals;
  assert_eq!(listed.len(), 1);
  assert_eq!(listed[0].server_id, proposal.server_id);
  assert_eq!(listed[0].project, proposal.project);
  assert_eq!(listed[0].body, proposal.body);

  poller.shutdown().await;
  server.stop().await;
}

#[tokio::test]
async fn goal_registration_proposal_with_object_summary_appears_in_pending_decisions() {
  // 実 takt-mcp の応答（summary がオブジェクト）でも、判断待ちの射影に登録承認項目が現れ、
  // revision と本文の objective が写ることを観測する。
  let (harness, mut poller) = polled_harness(state_with_object_summary_proposal()).await;
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let state = client
    .get_pending_decisions(GetPendingDecisionsRequest {})
    .await
    .expect("判断待ちを取得できます")
    .into_inner()
    .state
    .expect("状態があります");
  let proposals: Vec<_> =
    state.pending_decisions.iter().filter(|d| d.r#type == PendingDecisionType::GoalRegistration as i32).collect();
  assert_eq!(proposals.len(), 1, "承認待ちが1件だけ一覧に現れます");
  let proposal = proposals[0];
  assert_eq!(proposal.server_id, "mac-mini");
  assert_eq!(proposal.project, "takt-dash");
  assert_eq!(proposal.revision, Some(6), "承認待ちの revision を写します");
  assert!(
    proposal.body.contains("稼働状況を一覧できるようにする"),
    "本文に summary オブジェクトの objective を含みます: {}",
    proposal.body
  );

  poller.shutdown().await;
  server.stop().await;
}

#[tokio::test]
async fn malformed_proposal_response_does_not_fail_the_next_poll_cycle() {
  // 同じ poller で「正常な提案を読むサイクル」→「提案の解析に失敗するサイクル」を続け、
  // 2回目も周期が失敗せず、前回の提案が判断待ちの射影に残ることを一続きに観測する。
  let harness = harness_with(json!({}));
  let mut poller = Poller::new(
    harness.config_path().to_path_buf(),
    harness.activity.clone(),
    harness.store.clone(),
    harness.changes.clone(),
    harness.registry.clone(),
  );
  harness.rewrite_state(state_with_proposal());
  let first = poller.read_cycle_once().await.expect("正常な提案を読むサイクルを実行できます");
  assert_eq!(first.projects_read, 1, "1回目は提案を読めます");
  assert_eq!(first.projects_failed, 0, "1回目は失敗しません");

  harness.rewrite_state(json!({
    "responses": {
      "takt_list_goal_registration_proposals": {
        "proposals": [
          { "revision": 6, "repositoryDirectory": "/abs/takt-dash", "summary": 123 }
        ]
      }
    }
  }));
  let second = poller.read_cycle_once().await.expect("提案の解析失敗で周期を失敗させません");
  assert_eq!(second.projects_read, 1, "解析失敗は前回値へ縮退した読み取りとして数えます");
  assert_eq!(second.projects_failed, 0, "解析失敗でプロジェクトの読み取りを失敗にしません");

  let server = start_server(&harness).await;
  let mut client = client(server.address).await;
  let state = client
    .get_pending_decisions(GetPendingDecisionsRequest {})
    .await
    .expect("判断待ちを取得できます")
    .into_inner()
    .state
    .expect("状態があります");
  let proposals: Vec<_> =
    state.pending_decisions.iter().filter(|d| d.r#type == PendingDecisionType::GoalRegistration as i32).collect();
  assert_eq!(proposals.len(), 1, "解析失敗後も前回の承認待ちが残ります");
  assert_eq!(proposals[0].revision, Some(3), "前回の提案の revision を保持します");
  assert!(proposals[0].body.contains("新しいゴール"), "前回の提案の本文を保持します: {}", proposals[0].body);

  poller.shutdown().await;
  server.stop().await;
}

#[tokio::test]
async fn a_pending_inquiry_without_proposals_leaves_the_list_unaffected() {
  // SCN-U1-N1: 承認待ちが無いときは既存の問い合わせ1件だけを返し、承認待ちを含まない。
  let (harness, mut poller) = polled_harness(state_with_standalone_inquiry()).await;
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let state = client
    .get_pending_decisions(GetPendingDecisionsRequest {})
    .await
    .expect("判断待ちを取得できます")
    .into_inner()
    .state
    .expect("状態があります");
  assert_eq!(state.pending_decisions.len(), 1, "問い合わせの1件だけを返します");
  assert_eq!(state.pending_decisions[0].r#type, PendingDecisionType::Inquiry as i32);
  assert!(
    !state.pending_decisions.iter().any(|d| d.r#type == PendingDecisionType::GoalRegistration as i32),
    "承認待ちが無ければその項目を含みません"
  );

  poller.shutdown().await;
  server.stop().await;
}

#[tokio::test]
async fn watch_state_delivers_a_goal_registration_proposal_on_the_same_stream() {
  // SCN-U1-P2: 接続後に承認待ちが増えると、同じストリームで一覧に届く。
  let harness = harness_with(json!({ "proposals": [] }));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let mut stream =
    client.watch_state(WatchStateRequest {}).await.expect("状態ストリーミングへ接続できます").into_inner();
  let first = timeout(Duration::from_secs(5), stream.message())
    .await
    .expect("初回の全件は即時に届きます")
    .expect("ストリームから読めます")
    .expect("初回の状態があります");
  assert!(
    !first.pending_decisions.iter().any(|d| d.r#type == PendingDecisionType::GoalRegistration as i32),
    "接続直後は承認待ちを含みません"
  );

  // 承認待ちを置いて poller を1サイクル回すと、変化通知で同じストリームへ再送される。
  let mut poller = Poller::new(
    harness.config_path().to_path_buf(),
    harness.activity.clone(),
    harness.store.clone(),
    harness.changes.clone(),
    harness.registry.clone(),
  );
  harness.rewrite_state(state_with_proposal());
  poller.read_cycle_once().await.expect("状態を変えるサイクルを実行できます");

  let second = timeout(Duration::from_secs(5), stream.message())
    .await
    .expect("承認待ちが増えれば新しい状態が届きます")
    .expect("ストリームから読めます")
    .expect("変化後の状態があります");
  let proposals: Vec<_> =
    second.pending_decisions.iter().filter(|d| d.r#type == PendingDecisionType::GoalRegistration as i32).collect();
  assert_eq!(proposals.len(), 1, "同じストリームに承認待ちが1件届きます");
  assert_eq!(proposals[0].revision, Some(3));

  poller.shutdown().await;
  server.stop().await;
}

#[tokio::test]
async fn answer_inquiry_sends_the_goal_id_that_owns_the_inquiry() {
  // SCN-U2-P1 / SCN-U2-N1: 回答は一覧の判断待ちからゴールを引き当て、その goalId を送る。
  // 要求で goal_id を省略しても、ゴールを持つ問い合わせにはゴールの識別子が渡る。
  let goal_id = "3f9c1a54-2b41-4d1e-9d2e-2c1a5b8f7e11";
  let (harness, mut poller) = polled_harness(state_with_goal_inquiry("question")).await;
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let response = client
    .answer_inquiry(AnswerInquiryRequest {
      idempotency_key: None,
      server_id:       "mac-mini".to_owned(),
      project:         "takt-dash".to_owned(),
      goal_id:         None,
      inquiry_id:      "8f14e45f-ceea-4f6a-9a3c-0f1a2b3c4d5e".to_owned(),
      text:            Some("はい".to_owned()),
      report:          None,
    })
    .await
    .expect("ゴールを持つ問い合わせに回答できます")
    .into_inner();
  assert_eq!(response.inquiry_id, "8f14e45f-ceea-4f6a-9a3c-0f1a2b3c4d5e");

  let calls = calls_named(&harness, "takt_answer_goal_question");
  assert_eq!(calls.len(), 1);
  assert_eq!(calls[0]["goalId"], json!(goal_id), "一覧の判断待ちから引き当てた goalId を送ります");
  assert_eq!(calls[0]["questionId"], json!("8f14e45f-ceea-4f6a-9a3c-0f1a2b3c4d5e"));
  assert_eq!(calls[0]["answer"], json!({ "text": "はい" }));

  poller.shutdown().await;
  server.stop().await;
}

#[tokio::test]
async fn answer_inquiry_rejects_an_inquiry_without_a_goal_without_calling_takt() {
  // takt_answer_goal_question の入力契約では goalId は必須（UUID）です。ゴールを持たない
  // 問い合わせはゴールを引き当てられないため、空文字を送らずに入力の誤りとして拒否します。
  let (harness, mut poller) = polled_harness(state_with_standalone_inquiry()).await;
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let status = client
    .answer_inquiry(AnswerInquiryRequest {
      idempotency_key: None,
      server_id:       "mac-mini".to_owned(),
      project:         "takt-dash".to_owned(),
      goal_id:         None,
      inquiry_id:      "8f14e45f-ceea-4f6a-9a3c-0f1a2b3c4d5e".to_owned(),
      text:            Some("はい".to_owned()),
      report:          None,
    })
    .await
    .expect_err("ゴールを特定できない問い合わせには回答できません")
    .code();
  assert_eq!(status, Code::InvalidArgument, "goalId を特定できない回答は入力の誤りです");
  assert!(calls_named(&harness, "takt_answer_goal_question").is_empty(), "goalId を送れない場合は takt を呼びません");

  poller.shutdown().await;
  server.stop().await;
}

#[tokio::test]
async fn answering_with_the_wrong_answer_kind_is_rejected_without_calling_takt() {
  // SCN-U4-N1: 質問に report を送ると拒否され、takt を呼ばない。
  let (harness, mut poller) = polled_harness(state_with_goal_inquiry("question")).await;
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let status = client
    .answer_inquiry(AnswerInquiryRequest {
      idempotency_key: None,
      server_id:       "mac-mini".to_owned(),
      project:         "takt-dash".to_owned(),
      goal_id:         None,
      inquiry_id:      "8f14e45f-ceea-4f6a-9a3c-0f1a2b3c4d5e".to_owned(),
      text:            None,
      report:          Some(OperationAnswerReport::Completed as i32),
    })
    .await
    .expect_err("質問に report は使えません")
    .code();
  assert_eq!(status, Code::InvalidArgument);
  assert!(calls_named(&harness, "takt_answer_goal_question").is_empty(), "種別の取り違えでは takt を呼びません");

  poller.shutdown().await;
  server.stop().await;
}

#[tokio::test]
async fn answering_an_operation_request_with_text_is_rejected_without_calling_takt() {
  // SCN-U4 の反対側: 作業の依頼に text を送ると拒否され、takt を呼ばない。
  let (harness, mut poller) = polled_harness(state_with_goal_inquiry("operation_request")).await;
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let status = client
    .answer_inquiry(AnswerInquiryRequest {
      idempotency_key: None,
      server_id:       "mac-mini".to_owned(),
      project:         "takt-dash".to_owned(),
      goal_id:         None,
      inquiry_id:      "8f14e45f-ceea-4f6a-9a3c-0f1a2b3c4d5e".to_owned(),
      text:            Some("はい".to_owned()),
      report:          None,
    })
    .await
    .expect_err("作業の依頼に text は使えません")
    .code();
  assert_eq!(status, Code::InvalidArgument);
  assert!(calls_named(&harness, "takt_answer_goal_question").is_empty(), "種別の取り違えでは takt を呼びません");

  poller.shutdown().await;
  server.stop().await;
}

#[tokio::test]
async fn answer_inquiry_sends_completed_for_a_finished_operation_request() {
  // SCN-U4-P1: Completed の写像が unable と取り違えられていないことを観測する。
  let (harness, mut poller) = polled_harness(state_with_goal_inquiry("operation_request")).await;
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  client
    .answer_inquiry(AnswerInquiryRequest {
      idempotency_key: None,
      server_id:       "mac-mini".to_owned(),
      project:         "takt-dash".to_owned(),
      goal_id:         None,
      inquiry_id:      "8f14e45f-ceea-4f6a-9a3c-0f1a2b3c4d5e".to_owned(),
      text:            None,
      report:          Some(OperationAnswerReport::Completed as i32),
    })
    .await
    .expect("実施したの回答を受け付けられます");

  let calls = calls_named(&harness, "takt_answer_goal_question");
  assert_eq!(calls.len(), 1);
  assert_eq!(calls[0]["answer"], json!({ "report": "completed" }), "Completed は report に completed を入れます");

  poller.shutdown().await;
  server.stop().await;
}

#[tokio::test]
async fn approve_goal_merge_rejects_an_unknown_until_without_calling_takt() {
  let harness = harness_with(json!({}));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  for until in ["", "unknown"] {
    let status = client
      .approve_goal_merge(ApproveGoalMergeRequest {
        idempotency_key: None,
        server_id:       "mac-mini".to_owned(),
        project:         "takt-dash".to_owned(),
        goal_id:         "g-merge".to_owned(),
        pr_number:       42,
        until:           until.to_owned(),
      })
      .await
      .expect_err("2値以外の until は拒否されます")
      .code();
    assert_eq!(status, Code::InvalidArgument, "until={until:?} は入力の誤りです");
  }
  assert!(calls_named(&harness, "takt_merge_goal_pr").is_empty(), "不正な until では takt を呼びません");

  server.stop().await;
}

#[tokio::test]
async fn list_goal_registration_proposals_selects_by_the_resolved_cwd() {
  // SCN-U7: mock は cwd で状態を選ぶ。harness が割り当てた cwd の提案だけが返る。
  // takt-dash には提案1件、takt-other には提案なしを割り当てる。
  let harness = harness_with(json!({}));
  let config = takt_dash_backend::config::load_config(harness.config_path()).expect("設定を読み込めます");
  let cwd =
    config.projects.iter().find(|p| p.name == "takt-dash").map(|p| p.cwd.clone()).expect("takt-dash の cwd があります");
  let mut proposals_by_cwd = serde_json::Map::new();
  proposals_by_cwd.insert(
    cwd.display().to_string(),
    json!([ { "revision": 3, "repositoryDirectory": cwd.display().to_string(), "summary": "新しいゴール" } ]),
  );
  proposals_by_cwd.insert("/abs/takt-other".to_owned(), json!([]));
  let mut poller = Poller::new(
    harness.config_path().to_path_buf(),
    harness.activity.clone(),
    harness.store.clone(),
    harness.changes.clone(),
    harness.registry.clone(),
  );
  harness.rewrite_state(json!({ "proposalsByCwd": proposals_by_cwd }));
  poller.read_cycle_once().await.expect("状態を取り込むサイクルを実行できます");

  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let proposals = client
    .list_goal_registration_proposals(ListGoalRegistrationProposalsRequest {
      server_id: "mac-mini".to_owned(),
      project:   "takt-dash".to_owned(),
    })
    .await
    .expect("承認待ちを取得できます")
    .into_inner()
    .proposals;
  assert_eq!(proposals.len(), 1, "cwd に対応する提案だけを返します");
  assert_eq!(proposals[0].revision, 3);

  // 道具には解決した cwd が渡っている。
  let calls = calls_named(&harness, "takt_list_goal_registration_proposals");
  assert!(!calls.is_empty(), "承認待ちの道具を呼びます");
  assert_eq!(calls[0]["cwd"], json!(cwd.display().to_string()), "解決した cwd を道具へ渡します");

  poller.shutdown().await;
  server.stop().await;
}

#[tokio::test]
async fn list_goal_registration_proposals_returns_empty_without_proposals() {
  let harness = harness_with(json!({ "proposals": [] }));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let response = client
    .list_goal_registration_proposals(ListGoalRegistrationProposalsRequest {
      server_id: "mac-mini".to_owned(),
      project:   "takt-dash".to_owned(),
    })
    .await
    .expect("承認待ちが無くてもエラーになりません")
    .into_inner();

  assert!(response.proposals.is_empty(), "承認待ちが無ければ空の並びを返します");

  server.stop().await;
}

#[tokio::test]
async fn a_tool_error_is_mapped_to_unavailable() {
  let harness = harness_with(json!({ "isError": ["takt_approve_goal_registration"] }));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let status = client
    .approve_goal_registration(ApproveGoalRegistrationRequest {
      idempotency_key: None,
      server_id:       "mac-mini".to_owned(),
      project:         "takt-dash".to_owned(),
      revision:        3,
    })
    .await
    .expect_err("道具エラーはエラーとして返ります")
    .code();

  assert_eq!(status, Code::Unavailable, "道具エラーは unavailable に写します");

  server.stop().await;
}

#[tokio::test]
async fn a_project_missing_from_the_config_is_unavailable_without_calling_takt() {
  let harness = harness_with(json!({}));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let status = client
    .approve_goal_registration(ApproveGoalRegistrationRequest {
      idempotency_key: None,
      server_id:       "mac-mini".to_owned(),
      project:         "unknown-project".to_owned(),
      revision:        3,
    })
    .await
    .expect_err("設定に無いプロジェクトは解決できません")
    .code();

  assert_eq!(status, Code::Unavailable);
  assert!(calls_named(&harness, "takt_approve_goal_registration").is_empty(), "解決できない対象では takt を呼びません");

  server.stop().await;
}

// ----------------------------------------------------------------------------
// 問い合わせの解決を (server_id, project, inquiry_id) でスコープする
// ----------------------------------------------------------------------------

/// 同じ `inquiry_id` を持ちうる2プロジェクトの土台。設定の順は B → A にして、
/// `inquiry_id` だけで引く実装が B を先に選ぶ状況を作る。
fn scope_harness() -> GrpcHarness {
  GrpcHarness::builder()
    .project(project("B", std::path::Path::new("/abs/B")))
    .project(project("A", std::path::Path::new("/abs/A")))
    .build()
}

/// 問い合わせだけを持つプロジェクトの snapshot。
fn snapshot_with(inquiries: Vec<InquiryEntry>) -> ProjectSnapshot {
  ProjectSnapshot { inquiries, ..ProjectSnapshot::empty() }
}

/// ゴールに属する未回答の問い合わせ1件。
fn inquiry_entry(inquiry_id: &str, goal_id: &str, kind: InquiryKind) -> InquiryEntry {
  InquiryEntry {
    inquiry_id: InquiryId::try_from(inquiry_id).expect("テストの inquiry ID は有効です"),
    goal_id: Some(GoalId::try_from(goal_id).expect("テストのゴール ID は有効です")),
    kind,
    body: "確認をお願いします".to_owned(),
    options: vec!["はい".to_owned(), "いいえ".to_owned()],
    answered: false,
    created_at: time("2026-11-13T07:00:00Z"),
  }
}

#[tokio::test]
async fn answer_inquiry_uses_only_the_matching_project_inquiry() {
  // 同じ inquiry_id "q-1" が A（質問）と B（作業の依頼）にあり、設定順で B が先でも、
  // A への回答は A のゴールと A の cwd で takt を1回だけ呼ぶ。
  let harness = scope_harness();
  let goal_a = "11111111-1111-4111-8111-111111111111";
  let goal_b = "22222222-2222-4222-8222-222222222222";
  harness.store.record_success(
    "A",
    snapshot_with(vec![inquiry_entry("q-1", goal_a, InquiryKind::Question)]),
    time("2026-11-13T12:00:00Z"),
  );
  harness.store.record_success(
    "B",
    snapshot_with(vec![inquiry_entry("q-1", goal_b, InquiryKind::OperationRequest)]),
    time("2026-11-13T12:00:00Z"),
  );
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let response = client
    .answer_inquiry(AnswerInquiryRequest {
      idempotency_key: None,
      server_id:       "mac-mini".to_owned(),
      project:         "A".to_owned(),
      goal_id:         None,
      inquiry_id:      "q-1".to_owned(),
      text:            Some("はい".to_owned()),
      report:          None,
    })
    .await
    .expect("対象プロジェクトの問い合わせには回答できます")
    .into_inner();
  assert_eq!(response.inquiry_id, "q-1");

  let calls = calls_named(&harness, "takt_answer_goal_question");
  assert_eq!(calls.len(), 1, "対象プロジェクトの回答だけを1回呼びます: {calls:?}");
  assert_eq!(calls[0]["goalId"], json!(goal_a), "対象プロジェクト A のゴールへ回答します");
  assert_eq!(calls[0]["questionId"], json!("q-1"));
  assert_eq!(calls[0]["answer"], json!({ "text": "はい" }));
  assert_eq!(
    calls[0]["cwd"],
    json!(harness.project_cwd("A").display().to_string()),
    "対象プロジェクト A の cwd で takt を呼びます"
  );
  assert_ne!(calls[0]["goalId"], json!(goal_b), "別プロジェクト B のゴールへ書き込みません");
  assert_eq!(harness.recorded_processes().len(), 1, "回答のために起動する takt-mcp は1つだけです");

  server.stop().await;
}

#[tokio::test]
async fn answer_inquiry_rejects_inquiry_id_of_another_project_without_calling_takt() {
  // inquiry_id "q-1" は B にだけある。A への回答はその id を引き当てられず、NOT_FOUND で
  // 拒否し、takt を呼ばない。
  let harness = scope_harness();
  harness.store.record_success("A", snapshot_with(vec![]), time("2026-11-13T12:00:00Z"));
  harness.store.record_success(
    "B",
    snapshot_with(vec![inquiry_entry("q-1", "22222222-2222-4222-8222-222222222222", InquiryKind::Question)]),
    time("2026-11-13T12:00:00Z"),
  );
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let status = client
    .answer_inquiry(AnswerInquiryRequest {
      idempotency_key: None,
      server_id:       "mac-mini".to_owned(),
      project:         "A".to_owned(),
      goal_id:         None,
      inquiry_id:      "q-1".to_owned(),
      text:            Some("はい".to_owned()),
      report:          None,
    })
    .await
    .expect_err("別プロジェクトにしかない識別子は引き当てられません")
    .code();

  assert_eq!(status, Code::NotFound, "対象プロジェクトに無い問い合わせは not_found です");
  assert!(calls_named(&harness, "takt_answer_goal_question").is_empty(), "引き当て失敗では takt を呼びません");

  server.stop().await;
}

#[tokio::test]
async fn answer_inquiry_rejects_a_missing_inquiry_with_an_explicit_goal_without_calling_takt() {
  // 対象プロジェクトに問い合わせが無ければ、要求が goal_id を明示していても NOT_FOUND で
  // 拒否し、takt を呼ばず、takt-mcp の子プロセスも起動しない。
  let harness = scope_harness();
  harness.store.record_success("A", snapshot_with(vec![]), time("2026-11-13T12:00:00Z"));
  harness.store.record_success(
    "B",
    snapshot_with(vec![inquiry_entry("q-404", "22222222-2222-4222-8222-222222222222", InquiryKind::Question)]),
    time("2026-11-13T12:00:00Z"),
  );
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let status = client
    .answer_inquiry(AnswerInquiryRequest {
      idempotency_key: None,
      server_id:       "mac-mini".to_owned(),
      project:         "A".to_owned(),
      goal_id:         Some("22222222-2222-4222-8222-222222222222".to_owned()),
      inquiry_id:      "q-404".to_owned(),
      text:            Some("はい".to_owned()),
      report:          None,
    })
    .await
    .expect_err("対象プロジェクトに無い問い合わせは回答できません")
    .code();

  assert_eq!(status, Code::NotFound, "一致0件は goal_id の明示に関わらず not_found です");
  assert!(calls_named(&harness, "takt_answer_goal_question").is_empty(), "一致0件では takt を呼びません");
  assert_eq!(harness.recorded_processes().len(), 0, "一致0件では takt-mcp の子プロセスを起動しません");

  server.stop().await;
}

#[tokio::test]
async fn answer_inquiry_rejects_ambiguous_inquiry_id_without_calling_takt() {
  // A の2つのゴールが同じ inquiry_id "q-1" を持つ。どちらか一方を選べないため
  // INVALID_ARGUMENT で拒否し、takt を呼ばない。
  let harness = scope_harness();
  harness.store.record_success(
    "A",
    snapshot_with(vec![
      inquiry_entry("q-1", "11111111-1111-4111-8111-111111111111", InquiryKind::Question),
      inquiry_entry("q-1", "33333333-3333-4333-8333-333333333333", InquiryKind::Question),
    ]),
    time("2026-11-13T12:00:00Z"),
  );
  harness.store.record_success("B", snapshot_with(vec![]), time("2026-11-13T12:00:00Z"));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let status = client
    .answer_inquiry(AnswerInquiryRequest {
      idempotency_key: None,
      server_id:       "mac-mini".to_owned(),
      project:         "A".to_owned(),
      goal_id:         None,
      inquiry_id:      "q-1".to_owned(),
      text:            Some("はい".to_owned()),
      report:          None,
    })
    .await
    .expect_err("同じプロジェクト内で重複する識別子は選べません")
    .code();

  assert_eq!(status, Code::InvalidArgument, "曖昧な問い合わせは invalid_argument です");
  assert!(calls_named(&harness, "takt_answer_goal_question").is_empty(), "曖昧な識別子では takt を呼びません");

  server.stop().await;
}

/// 記録された子プロセスが `expected` 件になるまで短い間隔で待ち、その記録を返す。
///
/// 書き込みレーンの温めは読み取りサイクルの裏で進むため、件数の観測は完了を待ってから
/// 行います。待ちは最大で約5秒です。
async fn wait_for_processes(harness: &GrpcHarness, expected: usize) -> Vec<u32> {
  for _ in 0..100 {
    let processes = harness.recorded_processes();
    if processes.len() >= expected {
      return processes;
    }
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
  }
  harness.recorded_processes()
}

#[tokio::test]
async fn write_rpcs_use_a_dedicated_write_lane_process() {
  // 読み取りは定期読み取りレーンの子プロセスを使い、書き込みは書き込みレーンの
  // 子プロセスを使う。書き込みレーンは読み取りサイクルのセットアップで温められ、
  // 続く AnswerInquiry はその子プロセスをそのまま使う。pid の記録が増えないことで、
  // 書き込みごとに新しい接続を作っていないことと、読み取りと接続を共有していない
  // ことを観測する。
  let harness = harness_with(json!({}));
  let mut poller = Poller::new(
    harness.config_path().to_path_buf(),
    harness.activity.clone(),
    harness.store.clone(),
    harness.changes.clone(),
    harness.registry.clone(),
  );
  harness.rewrite_state(state_with_goal_inquiry("question"));
  poller.read_cycle_once().await.expect("状態を取り込むサイクルを実行できます");

  let processes_after_read = wait_for_processes(&harness, 2).await;
  assert_eq!(
    processes_after_read.len(),
    2,
    "読み取りレーンと温めた書き込みレーンで takt-mcp を2つ起動します: {processes_after_read:?}"
  );

  let server = start_server(&harness).await;
  let mut client = client(server.address).await;
  client
    .answer_inquiry(AnswerInquiryRequest {
      idempotency_key: None,
      server_id:       "mac-mini".to_owned(),
      project:         "takt-dash".to_owned(),
      goal_id:         None,
      inquiry_id:      "8f14e45f-ceea-4f6a-9a3c-0f1a2b3c4d5e".to_owned(),
      text:            Some("はい".to_owned()),
      report:          None,
    })
    .await
    .expect("書き込みレーンの子プロセスで回答できます");

  let calls = calls_named(&harness, "takt_answer_goal_question");
  assert_eq!(calls.len(), 1, "回答の道具を1回だけ呼びます: {calls:?}");
  assert_eq!(
    harness.recorded_processes(),
    processes_after_read,
    "書き込みは温めた書き込みレーンの子プロセスをそのまま使います"
  );

  poller.shutdown().await;
  for pid in processes_after_read {
    assert!(!process_exists(pid), "停止後はレーンの子プロセスが残りません");
  }

  server.stop().await;
}

/// 設定ファイルの `projects[0].cwd` を `cwd` へ書き換える。
fn rewrite_project_cwd(config_path: &Path, cwd: &Path) {
  let mut value: Value =
    serde_json::from_str(&std::fs::read_to_string(config_path).expect("設定を読めます")).expect("設定を解析できます");
  let projects = value["projects"].as_array_mut().expect("プロジェクトの一覧があります");
  let project = projects.first_mut().expect("プロジェクトが1件あります");
  project["cwd"] = json!(cwd.display().to_string());
  std::fs::write(config_path, value.to_string()).expect("設定を書けます");
}

/// 設定ファイルの mock 起動引数にある `--state` の値だけを `state_path` へ書き換える。
///
/// `--calls` と `--processes` の記録先はそのまま残します。
fn rewrite_state_argument(config_path: &Path, state_path: &Path) {
  let mut value: Value =
    serde_json::from_str(&std::fs::read_to_string(config_path).expect("設定を読めます")).expect("設定を解析できます");
  let mcp_servers = value["mcpServers"].as_object_mut().expect("mcpServers があります");
  let server = mcp_servers.get_mut("mac-mini").expect("mac-mini の起動設定があります");
  let args = server["args"].as_array_mut().expect("起動引数があります");
  let index = args.iter().position(|arg| arg == "--state").expect("--state 引数があります");
  *args.get_mut(index + 1).expect("--state の値があります") = json!(state_path.display().to_string());
  std::fs::write(config_path, value.to_string()).expect("設定を書けます");
}

#[tokio::test]
async fn write_rpc_rebuilds_the_client_when_the_project_cwd_changes() {
  // 同じ gRPC サーバとレジストリのまま、プロジェクトの cwd を変える。
  // 既存クライアントを設定と比較せずに再利用する実装では、次の書き込みが旧 cwd のまま
  // 旧子プロセスを使い続けるため、記録された cwd と子プロセスの数で検出する。
  let harness = harness_with(json!({}));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  client
    .approve_goal_registration(ApproveGoalRegistrationRequest {
      idempotency_key: None,
      server_id:       "mac-mini".to_owned(),
      project:         "takt-dash".to_owned(),
      revision:        3,
    })
    .await
    .expect("最初の承認は成功します");

  let first_processes = harness.recorded_processes();
  assert_eq!(first_processes.len(), 1, "最初の書き込みで子プロセスを1つ起動します");
  let first_calls = calls_named(&harness, "takt_approve_goal_registration");
  assert_eq!(first_calls.len(), 1, "承認の道具を1回だけ呼びます: {first_calls:?}");
  let old_cwd = first_calls[0]["cwd"].as_str().expect("記録された cwd を読めます").to_owned();

  let new_directory = tempfile::tempdir().expect("新しい cwd を作れます");
  let new_cwd = new_directory.path();
  rewrite_project_cwd(harness.config_path(), new_cwd);

  client
    .approve_goal_registration(ApproveGoalRegistrationRequest {
      idempotency_key: None,
      server_id:       "mac-mini".to_owned(),
      project:         "takt-dash".to_owned(),
      revision:        4,
    })
    .await
    .expect("設定変更後の承認は成功します");

  let calls = calls_named(&harness, "takt_approve_goal_registration");
  assert_eq!(calls.len(), 2, "承認の道具を2回呼びます: {calls:?}");
  assert_eq!(
    calls[1]["cwd"],
    json!(new_cwd.display().to_string()),
    "設定変更後の書き込みは新しい cwd を道具へ渡します"
  );
  assert_ne!(calls[1]["cwd"], json!(old_cwd), "設定変更後の書き込みは旧 cwd を使いません");
  let processes = harness.recorded_processes();
  assert_eq!(processes.len(), 2, "設定変更後は新しい子プロセスで書き込みます: {processes:?}");
  assert_ne!(processes[0], processes[1], "新しい子プロセスは旧プロセスと別の pid です");

  server.stop().await;
}

#[tokio::test]
async fn write_rpc_rebuild_resets_the_baseline_before_the_next_poll_cycle() {
  // 書き込み RPC が設定変更でクライアントを作り直した場合も、次の読み取りサイクルは
  // そのプロジェクトの差分基準とスロットをリセットする。書き込み由来の作り直しを知らない
  // 実装では、旧接続先の状態との差分で一時停止の出来事を誤記録する。
  let goal = json!({
    "id": "3f9c1a54-2b41-4d1e-9d2e-2c1a5b8f7e11",
    "status": "created",
    "executionStatus": "active",
    "objective": "確認の対象",
    "integrationBranch": "takt/confirm"
  });
  let harness = harness_with(json!({ "goals": [goal] }));
  let mut poller = Poller::new(
    harness.config_path().to_path_buf(),
    harness.activity.clone(),
    harness.store.clone(),
    harness.changes.clone(),
    harness.registry.clone(),
  );
  poller.read_cycle_once().await.expect("初回のサイクルを実行できます");
  // 定期読み取りレーンと、温めた書き込みレーンの2プロセスが立つまで待ちます。
  let initial_processes = wait_for_processes(&harness, 2).await;
  assert_eq!(initial_processes.len(), 2, "初回は読み取りレーンと書き込みレーンの2つです: {initial_processes:?}");

  let alternate_directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let alternate_state = alternate_directory.path().join("state-alternate.json");
  std::fs::write(
    &alternate_state,
    json!({
      "goals": [{
        "id": "3f9c1a54-2b41-4d1e-9d2e-2c1a5b8f7e11",
        "status": "created",
        "executionStatus": "paused",
        "objective": "確認の対象",
        "integrationBranch": "takt/confirm"
      }]
    })
    .to_string(),
  )
  .expect("別の状態を書けます");
  rewrite_state_argument(harness.config_path(), &alternate_state);

  let server = start_server(&harness).await;
  let mut client = client(server.address).await;
  client
    .approve_goal_registration(ApproveGoalRegistrationRequest {
      idempotency_key: None,
      server_id:       "mac-mini".to_owned(),
      project:         "takt-dash".to_owned(),
      revision:        3,
    })
    .await
    .expect("設定変更後の書き込みは成功します");
  assert_eq!(harness.recorded_processes().len(), 3, "書き込みが設定変更後の書き込みレーンの子プロセスを作ります");

  let report = poller.read_cycle_once().await.expect("設定変更後のサイクルを実行できます");
  poller.shutdown().await;
  server.stop().await;

  assert_eq!(report.events_recorded, 0, "作り直し後の初回は基準化だけを行い、出来事を出しません");
  let events: Vec<EventKind> =
    harness.activity.recent_events(50).expect("記録した出来事を読めます").into_iter().map(|event| event.kind).collect();
  assert!(!events.contains(&EventKind::Paused), "旧接続先との差分で一時停止を誤記録しません: {events:?}");
  let slot = harness.store.slot("takt-dash").expect("スロットがあります");
  let snapshot = slot.snapshot.expect("新しい設定で読み取った状態があります");
  assert_eq!(snapshot.goals[0].execution_status, ExecutionStatus::Paused, "新しい設定の状態を読み取ります");
}

/// 完了したゴールに、状態が `question_status` の問い合わせ1件を持つ mock の状態。
fn state_with_completed_goal_inquiry(question_status: &str) -> serde_json::Value {
  json!({
    "goals": [
      {
        "id": "3f9c1a54-2b41-4d1e-9d2e-2c1a5b8f7e11",
        "status": "completed",
        "executionStatus": "active",
        "objective": "完了したゴール",
        "integrationBranch": "takt/completed"
      }
    ],
    "goalQuestions": {
      "3f9c1a54-2b41-4d1e-9d2e-2c1a5b8f7e11": [
        {
          "id": "8f14e45f-ceea-4f6a-9a3c-0f1a2b3c4d5e",
          "kind": "question",
          "status": question_status,
          "body": "完了の前の確認です",
          "createdAt": "2026-11-13T07:00:00Z"
        }
      ]
    }
  })
}

/// 判断待ちの一覧にある問い合わせの識別子を返す。
async fn pending_inquiry_ids(
  client: &mut takt_dash_backend::takt_dash::v1::takt_dash_service_client::TaktDashServiceClient<
    tonic::transport::Channel,
  >,
) -> Vec<String> {
  client
    .get_pending_decisions(GetPendingDecisionsRequest {})
    .await
    .expect("判断待ちを取得できます")
    .into_inner()
    .state
    .expect("状態があります")
    .pending_decisions
    .into_iter()
    .filter(|decision| decision.r#type == PendingDecisionType::Inquiry as i32)
    .map(|decision| decision.inquiry_id)
    .collect()
}

/// 呼び出しの記録のうち、`from` 件目より後の takt_list_goal_questions の件数を返す。
fn goal_question_reads_since(harness: &GrpcHarness, from: usize) -> usize {
  harness.recorded_calls().into_iter().skip(from).filter(|call| call.name == "takt_list_goal_questions").count()
}

#[tokio::test]
async fn answered_question_of_a_terminal_goal_leaves_pending_decisions_on_the_next_cycle() {
  // 完了したゴールの未回答の問い合わせに回答すると、次の周期でその問い合わせを読み直し、
  // 判断待ちから消える。同じ poller と同じ状態ストアの上で、回答の前から後まで一続きに観測する。
  // 回答の後も終端の読み取り済みの印を残す実装は、前回の未回答を引き継いでここで落ちる。
  let inquiry_id = "8f14e45f-ceea-4f6a-9a3c-0f1a2b3c4d5e";
  let (harness, mut poller) = polled_harness(state_with_completed_goal_inquiry("pending")).await;
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  // 回答の前：2周期目は終端の読み取りを省き、未回答の問い合わせを引き継いで判断待ちに残す。
  let before_second = harness.recorded_calls().len();
  poller.read_cycle_once().await.expect("2周期目を実行できます");
  assert_eq!(goal_question_reads_since(&harness, before_second), 0, "回答の前は完了ゴールの質問を読み直しません");
  assert_eq!(pending_inquiry_ids(&mut client).await, vec![inquiry_id.to_owned()], "回答の前は判断待ちにあります");

  client
    .answer_inquiry(AnswerInquiryRequest {
      idempotency_key: None,
      server_id:       "mac-mini".to_owned(),
      project:         "takt-dash".to_owned(),
      goal_id:         None,
      inquiry_id:      inquiry_id.to_owned(),
      text:            Some("はい".to_owned()),
      report:          None,
    })
    .await
    .expect("完了したゴールの問い合わせに回答できます");
  // mock の回答は状態を変えないため、takt が問い合わせを回答済みにしたことを状態の書き換えで模す。
  harness.rewrite_state(state_with_completed_goal_inquiry("answered"));

  // 回答の後の周期：完了ゴールでも質問を読み直し、判断待ちから消える。
  let before_refresh = harness.recorded_calls().len();
  poller.read_cycle_once().await.expect("回答の後の周期を実行できます");
  assert_eq!(
    goal_question_reads_since(&harness, before_refresh),
    1,
    "回答の後の周期では完了ゴールの質問を読み直します"
  );
  assert!(pending_inquiry_ids(&mut client).await.is_empty(), "回答した問い合わせは判断待ちから消えます");

  // その次の周期：読み直した完了ゴールは、従来どおり読み取りを省く。
  let before_steady = harness.recorded_calls().len();
  poller.read_cycle_once().await.expect("その次の周期を実行できます");
  assert_eq!(goal_question_reads_since(&harness, before_steady), 0, "読み直した後の周期では再び読み取りを省きます");
  assert!(pending_inquiry_ids(&mut client).await.is_empty(), "回答した問い合わせは判断待ちに戻りません");

  poller.shutdown().await;
  server.stop().await;
}

#[tokio::test]
async fn an_answer_with_an_unknown_outcome_forces_a_reread_of_a_terminal_goal_question() {
  // takt が回答を受け取ったまま応答しないと、回答が保存されたかどうか分からない。次の周期で
  // 完了ゴールの質問を読み直させ、利用者が一覧で結果を確かめられるようにする。
  let inquiry_id = "8f14e45f-ceea-4f6a-9a3c-0f1a2b3c4d5e";
  let (harness, mut poller) = polled_harness(state_with_completed_goal_inquiry("pending")).await;
  let mut hanging = state_with_completed_goal_inquiry("pending");
  hanging["hang"] = json!(["takt_answer_goal_question"]);
  harness.rewrite_state(hanging);
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;
  assert_eq!(pending_inquiry_ids(&mut client).await, vec![inquiry_id.to_owned()]);

  let status = client
    .answer_inquiry(AnswerInquiryRequest {
      idempotency_key: None,
      server_id:       "mac-mini".to_owned(),
      project:         "takt-dash".to_owned(),
      goal_id:         None,
      inquiry_id:      inquiry_id.to_owned(),
      text:            Some("はい".to_owned()),
      report:          None,
    })
    .await
    .expect_err("takt が応答しません");
  assert_eq!(status.code(), Code::DeadlineExceeded);
  // takt は回答を保存していた。
  harness.rewrite_state(state_with_completed_goal_inquiry("answered"));

  let before_refresh = harness.recorded_calls().len();
  poller.read_cycle_once().await.expect("回答の後の周期を実行できます");
  assert_eq!(goal_question_reads_since(&harness, before_refresh), 1, "結果が不明の回答のあとは質問を読み直します");
  assert!(pending_inquiry_ids(&mut client).await.is_empty(), "保存されていた回答は判断待ちから消えます");

  poller.shutdown().await;
  server.stop().await;
}

#[tokio::test]
async fn failed_answer_to_a_terminal_goal_question_does_not_force_a_reread() {
  // 回答が takt で失敗したときは、完了ゴールの質問の読み取りを省いたままにする。
  // 回答を送る前、または失敗した回答でも読み直させる実装はここで落ちる。
  let inquiry_id = "8f14e45f-ceea-4f6a-9a3c-0f1a2b3c4d5e";
  let (harness, mut poller) = polled_harness(state_with_completed_goal_inquiry("pending")).await;
  let mut failing = state_with_completed_goal_inquiry("pending");
  failing["isError"] = json!(["takt_answer_goal_question"]);
  harness.rewrite_state(failing);
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  client
    .answer_inquiry(AnswerInquiryRequest {
      idempotency_key: None,
      server_id:       "mac-mini".to_owned(),
      project:         "takt-dash".to_owned(),
      goal_id:         None,
      inquiry_id:      inquiry_id.to_owned(),
      text:            Some("はい".to_owned()),
      report:          None,
    })
    .await
    .expect_err("takt が失敗した回答はエラーです");

  let before_next = harness.recorded_calls().len();
  poller.read_cycle_once().await.expect("失敗した回答の後の周期を実行できます");
  assert_eq!(goal_question_reads_since(&harness, before_next), 0, "失敗した回答では完了ゴールの質問を読み直しません");
  assert_eq!(
    pending_inquiry_ids(&mut client).await,
    vec![inquiry_id.to_owned()],
    "未回答の問い合わせは判断待ちに残ります"
  );

  poller.shutdown().await;
  server.stop().await;
}

#[tokio::test]
async fn write_over_http_read_project_routes_to_the_stdio_write_server() {
  // 読み取りを HTTP へ切り替えたプロジェクトでも、書き込みは `writeServer` の stdio で成功し、
  // 書き込み道具が HTTP サーバーへ渡らないことを観測する。書き込み先は読み取り先と別 id。
  let harness = GrpcHarness::builder()
    .single_project(project("takt-dash", std::path::Path::new("/abs/takt-dash")), server_meta(), json!({}))
    .http_read()
    .build();
  harness.store.record_success(
    "takt-dash",
    snapshot_with(vec![inquiry_entry(
      "8f14e45f-ceea-4f6a-9a3c-0f1a2b3c4d5e",
      "11111111-1111-4111-8111-111111111111",
      InquiryKind::Question,
    )]),
    time("2026-11-13T12:00:00Z"),
  );
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  client
    .answer_inquiry(AnswerInquiryRequest {
      idempotency_key: None,
      server_id:       "mac-mini".to_owned(),
      project:         "takt-dash".to_owned(),
      goal_id:         Some("11111111-1111-4111-8111-111111111111".to_owned()),
      inquiry_id:      "8f14e45f-ceea-4f6a-9a3c-0f1a2b3c4d5e".to_owned(),
      text:            Some("はい".to_owned()),
      report:          None,
    })
    .await
    .expect("HTTP 読み取りのプロジェクトでも writeServer の stdio で回答できます");

  let calls = calls_named(&harness, "takt_answer_goal_question");
  assert_eq!(calls.len(), 1, "書き込みは stdio の子プロセスへ渡ります");
  let http_calls = harness.http_recorded_calls();
  assert!(
    !http_calls.iter().any(|call| call["name"] == "takt_answer_goal_question"),
    "書き込み道具を HTTP サーバーへ渡しません: {http_calls:?}"
  );

  server.stop().await;
}

#[tokio::test]
async fn write_over_http_routes_to_the_http_write_server() {
  // 書き込み先にも常駐 HTTP サーバーを指すプロジェクトでは、書き込み道具が HTTP サーバーへ
  // 渡り、stdio の子プロセスへは渡らないことを観測する。
  let harness = GrpcHarness::builder()
    .single_project(project("takt-dash", std::path::Path::new("/abs/takt-dash")), server_meta(), json!({}))
    .http_read()
    .http_write()
    .build();
  harness.store.record_success(
    "takt-dash",
    snapshot_with(vec![inquiry_entry(
      "8f14e45f-ceea-4f6a-9a3c-0f1a2b3c4d5e",
      "11111111-1111-4111-8111-111111111111",
      InquiryKind::Question,
    )]),
    time("2026-11-13T12:00:00Z"),
  );
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  client
    .answer_inquiry(AnswerInquiryRequest {
      idempotency_key: None,
      server_id:       "mac-mini".to_owned(),
      project:         "takt-dash".to_owned(),
      goal_id:         Some("11111111-1111-4111-8111-111111111111".to_owned()),
      inquiry_id:      "8f14e45f-ceea-4f6a-9a3c-0f1a2b3c4d5e".to_owned(),
      text:            Some("はい".to_owned()),
      report:          None,
    })
    .await
    .expect("書き込み先の HTTP サーバーで回答できます");

  let http_calls = harness.http_recorded_calls();
  assert_eq!(
    http_calls.iter().filter(|call| call["name"] == "takt_answer_goal_question").count(),
    1,
    "書き込みは HTTP サーバーへ渡ります: {http_calls:?}"
  );
  let calls = calls_named(&harness, "takt_answer_goal_question");
  assert!(calls.is_empty(), "書き込み道具を stdio の子プロセスへ渡しません");

  server.stop().await;
}

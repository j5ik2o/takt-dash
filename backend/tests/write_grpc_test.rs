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

use common::*;
use grpc_harness::{client, start_server, GrpcHarness};
use serde_json::json;
use takt_dash_backend::{
  model::{InquiryEntry, InquiryKind, ProjectSnapshot},
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
      goals:         vec![],
      tasks:         vec![],
      inquiries:     vec![InquiryEntry {
        inquiry_id: "8f14e45f-ceea-4f6a-9a3c-0f1a2b3c4d5e".to_owned(),
        goal_id:    Some("3f9c1a54-2b41-4d1e-9d2e-2c1a5b8f7e11".to_owned()),
        kind:       InquiryKind::Question,
        body:       "続けてよいですか".to_owned(),
        options:    vec!["はい".to_owned(), "いいえ".to_owned()],
        answered:   false,
        created_at: time("2026-11-13T07:00:00Z"),
      }],
      runs:          vec![],
      notifications: vec![],
      proposals:     vec![],
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
      server_id:  "mac-mini".to_owned(),
      project:    "takt-dash".to_owned(),
      goal_id:    Some("3f9c1a54-2b41-4d1e-9d2e-2c1a5b8f7e11".to_owned()),
      inquiry_id: inquiry.inquiry_id.clone(),
      text:       Some("はい".to_owned()),
      report:     None,
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
      goals:         vec![],
      tasks:         vec![],
      inquiries:     vec![InquiryEntry {
        inquiry_id: "answered-id".to_owned(),
        goal_id:    Some("3f9c1a54-2b41-4d1e-9d2e-2c1a5b8f7e11".to_owned()),
        kind:       InquiryKind::Question,
        body:       "回答済み".to_owned(),
        options:    vec![],
        answered:   true,
        created_at: time("2026-11-13T07:00:00Z"),
      }],
      runs:          vec![],
      notifications: vec![],
      proposals:     vec![],
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
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let response = client
    .answer_inquiry(AnswerInquiryRequest {
      server_id:  "mac-mini".to_owned(),
      project:    "takt-dash".to_owned(),
      goal_id:    Some("3f9c1a54-2b41-4d1e-9d2e-2c1a5b8f7e11".to_owned()),
      inquiry_id: "8f14e45f-ceea-4f6a-9a3c-0f1a2b3c4d5e".to_owned(),
      text:       Some("はい".to_owned()),
      report:     None,
    })
    .await
    .expect("回答を受け付けられます")
    .into_inner();

  assert_eq!(response.inquiry_id, "8f14e45f-ceea-4f6a-9a3c-0f1a2b3c4d5e", "応答の識別子をそのまま返します");

  let calls = calls_named(&harness, "takt_answer_goal_question");
  assert_eq!(calls.len(), 1, "回答の道具を1回だけ呼びます: {calls:?}");
  let arguments = &calls[0];
  assert_eq!(arguments["goalId"], json!("3f9c1a54-2b41-4d1e-9d2e-2c1a5b8f7e11"));
  assert_eq!(arguments["questionId"], json!("8f14e45f-ceea-4f6a-9a3c-0f1a2b3c4d5e"));
  assert_eq!(arguments["answer"], json!({ "text": "はい" }), "answer はオブジェクトで text を持つ必要があります");

  server.stop().await;
}

#[tokio::test]
async fn answer_inquiry_sends_unable_for_an_operation_request() {
  let harness = harness_with(json!({}));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  client
    .answer_inquiry(AnswerInquiryRequest {
      server_id:  "mac-mini".to_owned(),
      project:    "takt-dash".to_owned(),
      goal_id:    Some("3f9c1a54-2b41-4d1e-9d2e-2c1a5b8f7e11".to_owned()),
      inquiry_id: "8f14e45f-ceea-4f6a-9a3c-0f1a2b3c4d5e".to_owned(),
      text:       None,
      report:     Some(OperationAnswerReport::Unable as i32),
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
      server_id:  "mac-mini".to_owned(),
      project:    "takt-dash".to_owned(),
      goal_id:    Some("3f9c1a54-2b41-4d1e-9d2e-2c1a5b8f7e11".to_owned()),
      inquiry_id: String::new(),
      text:       Some("はい".to_owned()),
      report:     None,
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
      server_id:  "mac-mini".to_owned(),
      project:    "takt-dash".to_owned(),
      goal_id:    Some("3f9c1a54-2b41-4d1e-9d2e-2c1a5b8f7e11".to_owned()),
      inquiry_id: "8f14e45f-ceea-4f6a-9a3c-0f1a2b3c4d5e".to_owned(),
      text:       None,
      report:     None,
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
      server_id: "mac-mini".to_owned(),
      project:   "takt-dash".to_owned(),
      goal_id:   "g-merge".to_owned(),
      pr_number: 42,
      until:     "merge".to_owned(),
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
      server_id: "mac-mini".to_owned(),
      project:   "takt-dash".to_owned(),
      goal_id:   "g-merge".to_owned(),
      pr_number: 0,
      until:     "merge".to_owned(),
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
      server_id: "mac-mini".to_owned(),
      project:   "takt-dash".to_owned(),
      revision:  3,
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
async fn reject_goal_registration_calls_reject_and_not_approve() {
  let harness = harness_with(json!({}));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let response = client
    .reject_goal_registration(RejectGoalRegistrationRequest {
      server_id: "mac-mini".to_owned(),
      project:   "takt-dash".to_owned(),
      revision:  3,
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
      server_id: "mac-mini".to_owned(),
      project:   "takt-dash".to_owned(),
      revision:  0,
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
      server_id:  "mac-mini".to_owned(),
      project:    "takt-dash".to_owned(),
      goal_id:    None,
      inquiry_id: "8f14e45f-ceea-4f6a-9a3c-0f1a2b3c4d5e".to_owned(),
      text:       Some("はい".to_owned()),
      report:     None,
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
      server_id:  "mac-mini".to_owned(),
      project:    "takt-dash".to_owned(),
      goal_id:    None,
      inquiry_id: "8f14e45f-ceea-4f6a-9a3c-0f1a2b3c4d5e".to_owned(),
      text:       Some("はい".to_owned()),
      report:     None,
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
      server_id:  "mac-mini".to_owned(),
      project:    "takt-dash".to_owned(),
      goal_id:    None,
      inquiry_id: "8f14e45f-ceea-4f6a-9a3c-0f1a2b3c4d5e".to_owned(),
      text:       None,
      report:     Some(OperationAnswerReport::Completed as i32),
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
      server_id:  "mac-mini".to_owned(),
      project:    "takt-dash".to_owned(),
      goal_id:    None,
      inquiry_id: "8f14e45f-ceea-4f6a-9a3c-0f1a2b3c4d5e".to_owned(),
      text:       Some("はい".to_owned()),
      report:     None,
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
      server_id:  "mac-mini".to_owned(),
      project:    "takt-dash".to_owned(),
      goal_id:    None,
      inquiry_id: "8f14e45f-ceea-4f6a-9a3c-0f1a2b3c4d5e".to_owned(),
      text:       None,
      report:     Some(OperationAnswerReport::Completed as i32),
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
        server_id: "mac-mini".to_owned(),
        project:   "takt-dash".to_owned(),
        goal_id:   "g-merge".to_owned(),
        pr_number: 42,
        until:     until.to_owned(),
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
      server_id: "mac-mini".to_owned(),
      project:   "takt-dash".to_owned(),
      revision:  3,
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
      server_id: "mac-mini".to_owned(),
      project:   "unknown-project".to_owned(),
      revision:  3,
    })
    .await
    .expect_err("設定に無いプロジェクトは解決できません")
    .code();

  assert_eq!(status, Code::Unavailable);
  assert!(calls_named(&harness, "takt_approve_goal_registration").is_empty(), "解決できない対象では takt を呼びません");

  server.stop().await;
}

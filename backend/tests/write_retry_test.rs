//! 書き込み系 unary RPC の再試行が、takt への書き込みを重ねないことを検証する（issue #305）。
//!
//! takt 側では書き込みが済んでいるのに、呼び出し側には応答が期限内に届かない状況を mock MCP
//! サーバで作る。mock は道具呼び出しを受け取った時点で記録するため、記録の件数が takt へ届いた
//! 書き込みの回数になる。実 takt には接続しない。

mod common;
mod grpc_harness;

use common::*;
use grpc_harness::{client, start_server, GrpcHarness};
use serde_json::{json, Value};
use takt_dash_backend::{
  domain::{GoalId, InquiryId, RegistrationRevision},
  model::{InquiryEntry, InquiryKind, ProjectSnapshot, RegistrationProposalEntry},
  takt_dash::v1::{AnswerInquiryRequest, ApproveGoalMergeRequest, ApproveGoalRegistrationRequest},
};
use tokio::time::Duration;
use tonic::{Code, Request};

const INQUIRY_ID: &str = "8f14e45f-ceea-4f6a-9a3c-0f1a2b3c4d5e";
const GOAL_ID: &str = "3f9c1a54-2b41-4d1e-9d2e-2c1a5b8f7e11";
/// 利用者の待てる時間。mock の応答はこれより遅れる。
const CALLER_DEADLINE: Duration = Duration::from_millis(300);
/// mock が応答を遅らせる時間。呼び出しの記録（書き込みの適用）は遅らせない。
const SLOW_REPLY_MS: u64 = 1500;

fn harness_with(state: Value) -> GrpcHarness {
  let harness = GrpcHarness::builder()
    .single_project(project("takt-dash", std::path::Path::new("/abs/takt-dash")), server_meta(), state)
    .build();
  harness.store.record_success(
    "takt-dash",
    ProjectSnapshot {
      inquiries: vec![InquiryEntry {
        inquiry_id: InquiryId::try_from(INQUIRY_ID).expect("テストの inquiry ID は有効です"),
        goal_id:    Some(GoalId::try_from(GOAL_ID).expect("テストのゴール ID は有効です")),
        kind:       InquiryKind::Question,
        body:       "続けてよいですか".to_owned(),
        options:    vec![],
        answered:   false,
        created_at: time("2026-11-13T07:00:00Z"),
      }],
      proposals: vec![RegistrationProposalEntry {
        revision:             RegistrationRevision::try_from(3).expect("テストの revision は有効です"),
        repository_directory: "/abs/takt-dash".to_owned(),
        summary:              "新しいゴール".to_owned(),
      }],
      ..ProjectSnapshot::empty()
    },
    time("2026-11-13T12:00:00Z"),
  );
  harness
}

fn call_count(harness: &GrpcHarness, name: &str) -> usize {
  harness.recorded_calls().into_iter().filter(|call| call.name == name).count()
}

fn answer(text: &str, key: Option<&str>) -> AnswerInquiryRequest {
  AnswerInquiryRequest {
    server_id:       "mac-mini".to_owned(),
    project:         "takt-dash".to_owned(),
    goal_id:         Some(GOAL_ID.to_owned()),
    inquiry_id:      INQUIRY_ID.to_owned(),
    text:            Some(text.to_owned()),
    report:          None,
    idempotency_key: key.map(str::to_owned),
  }
}

fn merge(key: Option<&str>) -> ApproveGoalMergeRequest {
  ApproveGoalMergeRequest {
    server_id:       "mac-mini".to_owned(),
    project:         "takt-dash".to_owned(),
    goal_id:         GOAL_ID.to_owned(),
    pr_number:       42,
    until:           "merge".to_owned(),
    idempotency_key: key.map(str::to_owned),
  }
}

fn approval(key: Option<&str>) -> ApproveGoalRegistrationRequest {
  ApproveGoalRegistrationRequest {
    server_id:       "mac-mini".to_owned(),
    project:         "takt-dash".to_owned(),
    revision:        3,
    idempotency_key: key.map(str::to_owned),
  }
}

fn within<T>(message: T, deadline: Duration) -> Request<T> {
  let mut request = Request::new(message);
  request.set_timeout(deadline);
  request
}

#[tokio::test]
async fn retrying_an_answer_whose_response_timed_out_does_not_answer_twice() {
  let harness = harness_with(json!({ "delays": { "takt_answer_goal_question": SLOW_REPLY_MS } }));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let first = client.answer_inquiry(within(answer("はい", Some("answer-1")), CALLER_DEADLINE)).await;
  assert!(first.is_err(), "応答が期限内に届かないため、最初の呼び出しは失敗します");
  assert_eq!(call_count(&harness, "takt_answer_goal_question"), 1, "takt 側では回答が適用済みです");

  let retried = client
    .answer_inquiry(answer("はい", Some("answer-1")))
    .await
    .expect("同じ鍵の再試行は、先に適用された回答の結果を受け取ります")
    .into_inner();

  assert_eq!(retried.inquiry_id, INQUIRY_ID);
  assert_eq!(call_count(&harness, "takt_answer_goal_question"), 1, "再試行で回答を重ねません");

  server.stop().await;
}

#[tokio::test]
async fn retrying_a_merge_approval_whose_response_timed_out_does_not_merge_twice() {
  let harness = harness_with(json!({ "delays": { "takt_merge_goal_pr": SLOW_REPLY_MS } }));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let first = client.approve_goal_merge(within(merge(Some("merge-1")), CALLER_DEADLINE)).await;
  assert!(first.is_err(), "応答が期限内に届かないため、最初の呼び出しは失敗します");

  client.approve_goal_merge(merge(Some("merge-1"))).await.expect("同じ鍵の再試行は、先の承認の結果を受け取ります");

  assert_eq!(call_count(&harness, "takt_merge_goal_pr"), 1, "再試行でマージの承認を重ねません");

  server.stop().await;
}

#[tokio::test]
async fn concurrent_requests_with_the_same_key_write_once() {
  let harness = harness_with(json!({ "delays": { "takt_approve_goal_registration": 500 } }));
  let server = start_server(&harness).await;
  let mut first_client = client(server.address).await;
  let mut second_client = client(server.address).await;

  let (first, second) = tokio::join!(
    first_client.approve_goal_registration(approval(Some("approve-1"))),
    second_client.approve_goal_registration(approval(Some("approve-1"))),
  );

  let first = first.expect("先の要求は承認の結果を受け取ります").into_inner();
  let second = second.expect("競合した同じ要求も同じ結果を受け取ります").into_inner();
  assert_eq!(first, second);
  assert_eq!(call_count(&harness, "takt_approve_goal_registration"), 1, "競合した同じ操作は1回だけ書き込みます");

  server.stop().await;
}

#[tokio::test]
async fn a_different_key_is_a_separate_operation() {
  let harness = harness_with(json!({}));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  client.approve_goal_merge(merge(Some("merge-1"))).await.expect("1回目の承認");
  client.approve_goal_merge(merge(Some("merge-2"))).await.expect("別の鍵の承認は別の操作です");

  assert_eq!(call_count(&harness, "takt_merge_goal_pr"), 2, "別の操作は妨げません");

  server.stop().await;
}

#[tokio::test]
async fn requests_without_a_key_keep_writing_each_time() {
  let harness = harness_with(json!({}));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  client.approve_goal_merge(merge(None)).await.expect("1回目の承認");
  client.approve_goal_merge(merge(None)).await.expect("鍵のない既存クライアントの2回目の承認");

  assert_eq!(call_count(&harness, "takt_merge_goal_pr"), 2, "鍵のない要求の挙動は変えません");

  server.stop().await;
}

#[tokio::test]
async fn a_rejected_write_can_be_retried_with_the_same_key() {
  let harness = harness_with(json!({ "isError": ["takt_approve_goal_registration"] }));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let failed = client.approve_goal_registration(approval(Some("approve-1"))).await.expect_err("道具が拒否します");
  assert_eq!(failed.code(), Code::Unavailable, "takt が拒否した失敗は、結果不明ではなく通常の失敗です");

  harness.rewrite_state(json!({}));
  client
    .approve_goal_registration(approval(Some("approve-1")))
    .await
    .expect("適用されなかった操作は、同じ鍵でやり直せます");

  assert_eq!(call_count(&harness, "takt_approve_goal_registration"), 2, "失敗が確定した操作は送り直します");

  server.stop().await;
}

#[tokio::test]
async fn reusing_a_key_for_a_different_request_is_rejected_without_writing() {
  let harness = harness_with(json!({}));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  client.answer_inquiry(answer("はい", Some("answer-1"))).await.expect("1回目の回答");
  let status =
    client.answer_inquiry(answer("いいえ", Some("answer-1"))).await.expect_err("同じ鍵で別の内容は受け付けません");

  assert_eq!(status.code(), Code::InvalidArgument);
  assert_eq!(call_count(&harness, "takt_answer_goal_question"), 1, "内容の違う要求は書き込みません");

  server.stop().await;
}

#[tokio::test]
async fn a_malformed_key_is_rejected_without_writing() {
  let harness = harness_with(json!({}));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  for key in ["", "  ", "改行\nを含む鍵", &"k".repeat(129)] {
    let status = client.approve_goal_merge(merge(Some(key))).await.expect_err("不正な鍵は受け付けません");
    assert_eq!(status.code(), Code::InvalidArgument, "鍵 {key:?}");
  }
  assert_eq!(call_count(&harness, "takt_merge_goal_pr"), 0, "不正な鍵の要求は書き込みません");

  server.stop().await;
}

#[tokio::test]
async fn a_write_takt_never_acknowledges_reports_an_unknown_outcome_and_is_not_resent() {
  // takt は呼び出しを受け取るが応答しない。backend の期限（15 秒）で時間切れになる。
  let harness = harness_with(json!({ "hang": ["takt_merge_goal_pr"] }));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let status = client.approve_goal_merge(merge(Some("merge-1"))).await.expect_err("takt が応答しません");
  assert_eq!(status.code(), Code::DeadlineExceeded, "時間切れは接続の失敗と区別します: {status:?}");
  assert!(status.message().contains("結果は不明"), "結果が不明であることを伝えます: {}", status.message());
  assert_eq!(call_count(&harness, "takt_merge_goal_pr"), 1, "takt は呼び出しを受け取っています");

  harness.rewrite_state(json!({}));
  let retried = client.approve_goal_merge(merge(Some("merge-1"))).await.expect_err("結果不明の操作は送り直しません");
  assert_eq!(retried.code(), Code::DeadlineExceeded);
  assert!(retried.message().contains("再送していません"), "再送しなかったことを伝えます: {}", retried.message());
  assert_eq!(call_count(&harness, "takt_merge_goal_pr"), 1, "同じ鍵の再試行で書き込みを重ねません");

  client.approve_goal_merge(merge(Some("merge-2"))).await.expect("状態を確かめたあとの新しい操作は通ります");
  assert_eq!(call_count(&harness, "takt_merge_goal_pr"), 2);

  server.stop().await;
}

#[tokio::test]
async fn a_write_that_was_never_sent_can_be_retried_with_the_same_key() {
  // 常駐 HTTP サーバーが最初のセッションの確立に応答しない。backend の期限（15 秒）で時間切れに
  // なるが、書き込みの道具は一度も送られていない。
  let harness = GrpcHarness::builder()
    .single_project(project("takt-dash", std::path::Path::new("/abs/takt-dash")), server_meta(), json!({}))
    .http_read()
    .http_write()
    .http_behavior("hang-initialize-once")
    .build();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;
  let merges = || harness.http_recorded_calls().iter().filter(|call| call["name"] == "takt_merge_goal_pr").count();

  let status = client.approve_goal_merge(merge(Some("merge-1"))).await.expect_err("接続を確立できません");
  assert_eq!(status.code(), Code::Unavailable, "送る前の失敗は結果不明ではありません: {status:?}");
  assert_eq!(merges(), 0, "書き込みは takt へ送られていません");

  client.approve_goal_merge(merge(Some("merge-1"))).await.expect("送られなかった操作は、同じ鍵でやり直せます");
  assert_eq!(merges(), 1, "やり直しで書き込みが1回だけ届きます");

  server.stop().await;
}

#[tokio::test]
async fn a_write_whose_connection_drops_after_it_was_received_is_not_resent() {
  // 常駐 HTTP サーバーが書き込みを受け取ったあと、応答を返す前に接続を切る。
  let harness = GrpcHarness::builder()
    .single_project(project("takt-dash", std::path::Path::new("/abs/takt-dash")), server_meta(), json!({}))
    .http_read()
    .http_write()
    .http_behavior("drop-connection-once")
    .build();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;
  let merges = || harness.http_recorded_calls().iter().filter(|call| call["name"] == "takt_merge_goal_pr").count();

  let status = client.approve_goal_merge(merge(Some("merge-1"))).await.expect_err("応答を受け取れません");
  assert_eq!(status.code(), Code::DeadlineExceeded, "受け取られた書き込みは接続の失敗と区別します: {status:?}");
  assert!(status.message().contains("結果は不明"), "{}", status.message());
  assert_eq!(merges(), 1, "takt は書き込みを受け取っています");

  let retried = client.approve_goal_merge(merge(Some("merge-1"))).await.expect_err("結果不明の操作は送り直しません");
  assert!(retried.message().contains("再送していません"), "{}", retried.message());
  assert_eq!(merges(), 1, "同じ鍵の再試行で書き込みを重ねません");

  server.stop().await;
}

#[tokio::test]
async fn a_retry_replays_the_recorded_approval_after_the_proposal_left_the_pending_list() {
  // 承認が済むと、登録案は判断待ちの一覧から消える。応答を失った利用者の同じ鍵のやり直しは、
  // 一覧との照合（issue #319）で断らず、先の書き込みの結果を返す。
  let harness = harness_with(json!({}));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let first =
    client.approve_goal_registration(approval(Some("approve-1"))).await.expect("最初の承認は成功します").into_inner();
  harness.store.record_success("takt-dash", ProjectSnapshot::empty(), time("2026-11-13T12:01:00Z"));
  let retried = client
    .approve_goal_registration(approval(Some("approve-1")))
    .await
    .expect("同じ鍵のやり直しは、記録した結果を受け取ります")
    .into_inner();

  assert_eq!(retried, first);
  assert_eq!(call_count(&harness, "takt_approve_goal_registration"), 1, "やり直しは takt へ送り直しません");

  let fresh = client
    .approve_goal_registration(approval(Some("approve-2")))
    .await
    .expect_err("新しい操作は、一覧に無い revision として断ります");
  assert_eq!(fresh.code(), Code::FailedPrecondition);
  assert_eq!(call_count(&harness, "takt_approve_goal_registration"), 1, "断った操作は takt へ送りません");

  server.stop().await;
}

#[tokio::test]
async fn a_write_rejected_by_the_pending_list_is_not_recorded_under_its_key() {
  // 一覧との照合で断った操作は、書き込みを適用していないと分かっている。台帳に残さず、一覧が
  // 追いついたあとの同じ鍵のやり直しを通す。
  let harness = harness_with(json!({}));
  harness.store.record_success("takt-dash", ProjectSnapshot::empty(), time("2026-11-13T12:00:00Z"));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let rejected =
    client.approve_goal_registration(approval(Some("approve-1"))).await.expect_err("一覧に無い revision は断ります");
  assert_eq!(rejected.code(), Code::FailedPrecondition);
  assert_eq!(call_count(&harness, "takt_approve_goal_registration"), 0);

  harness.store.record_success(
    "takt-dash",
    ProjectSnapshot {
      proposals: vec![RegistrationProposalEntry {
        revision:             RegistrationRevision::try_from(3).expect("テストの revision は有効です"),
        repository_directory: "/abs/takt-dash".to_owned(),
        summary:              "新しいゴール".to_owned(),
      }],
      ..ProjectSnapshot::empty()
    },
    time("2026-11-13T12:01:00Z"),
  );
  client
    .approve_goal_registration(approval(Some("approve-1")))
    .await
    .expect("一覧に現れたあとの同じ鍵のやり直しは通ります");
  assert_eq!(call_count(&harness, "takt_approve_goal_registration"), 1);

  server.stop().await;
}

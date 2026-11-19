mod common;
mod grpc_harness;

use std::path::Path;

use common::{project, server_meta};
use grpc_harness::{client, start_server, GrpcHarness};
use serde_json::{json, Value};
use takt_dash_backend::{poller::Poller, takt_dash::v1::GetActivityRequest};

async fn saved_goal_event(objective: &str, omissions: Value) -> String {
  let harness = GrpcHarness::builder()
    .single_project(project("takt-dash", Path::new("/abs/takt-dash")), server_meta(), json!({}))
    .build();
  let mut poller = Poller::new(
    harness.config_path().to_path_buf(),
    harness.activity.clone(),
    harness.store.clone(),
    harness.changes.clone(),
    harness.registry.clone(),
  );
  poller.read_cycle_once().await.expect("比較元を読み取れます");
  harness.rewrite_state(json!({ "goals": [{
    "id": "g1", "status": "created", "executionStatus": "active", "objective": objective,
    "truncated": true, "omissions": omissions
  }] }));
  poller.read_cycle_once().await.expect("登録されたゴールを読み取れます");
  poller.shutdown().await;
  let server = start_server(&harness).await;
  let response = client(server.address)
    .await
    .get_activity(GetActivityRequest {})
    .await
    .expect("保存済み活動を取得できます")
    .into_inner();
  server.stop().await;
  assert_eq!(response.events.len(), 1);
  response.events[0].body.clone()
}

#[tokio::test]
async fn source_objective_omission_is_visible_in_saved_activity() {
  let body = saved_goal_event("確認。", json!({ "objective": { "totalBytes": 90 } })).await;

  assert!(body.contains("確認。"));
  assert!(body.contains('…') || body.contains("省略"), "保存前に受信済み目的の省略を明示します: {body}");
}

#[tokio::test]
async fn omissions_for_other_goal_fields_do_not_mark_saved_objective() {
  let body = saved_goal_event("確認。", json!({ "questions": { "totalCount": 100 } })).await;

  assert_eq!(body, "ゴール「確認。」が登録されました。");
}

#[tokio::test]
async fn list_preview_limit_does_not_shorten_the_original_activity_body() {
  let objective = format!("確認。{}末尾", "あ".repeat(300));
  let body = saved_goal_event(&objective, json!({ "objective": { "totalBytes": objective.len() } })).await;

  assert_eq!(body, format!("ゴール「{objective}」が登録されました。"));
}

#[tokio::test]
async fn blocked_notification_reads_the_full_record_before_saving_the_event() {
  let full = format!("確認。{}末尾", "あ".repeat(3000));
  let harness = GrpcHarness::builder()
    .single_project(project("takt-dash", Path::new("/abs/takt-dash")), server_meta(), json!({}))
    .build();
  let mut poller = Poller::new(
    harness.config_path().to_path_buf(),
    harness.activity.clone(),
    harness.store.clone(),
    harness.changes.clone(),
    harness.registry.clone(),
  );
  poller.read_cycle_once().await.expect("比較元を読み取れます");
  harness.rewrite_state(json!({
    "goals": [{ "id": "11111111-1111-4111-8111-111111111111", "status": "created", "executionStatus": "active", "objective": "目的" }],
    "notifications": { "11111111-1111-4111-8111-111111111111": [{ "id": "33333333-3333-4333-8333-333333333333", "kind": "blocked", "body": "確認。",
      "recordedAt": "2026-11-19T00:00:00Z" }] },
    "notificationOmissions": [{ "path": "items[].body", "totalBytes": full.len(), "returnedBytes": 9 }],
    "goalDetails": { "11111111-1111-4111-8111-111111111111": { "notifications": [] } },
    "goalRecordPages": {
      "11111111-1111-4111-8111-111111111111:33333333-3333-4333-8333-333333333333:0": { "content": &full[..4095], "nextOffset": 4095 },
      "11111111-1111-4111-8111-111111111111:33333333-3333-4333-8333-333333333333:4095": { "content": &full[4095..], "nextOffset": null }
    }
  }));
  poller.read_cycle_once().await.expect("通知を読み取れます");
  poller.shutdown().await;
  let server = start_server(&harness).await;
  let response = client(server.address)
    .await
    .get_activity(GetActivityRequest {})
    .await
    .expect("保存済み活動を取得できます")
    .into_inner();
  server.stop().await;

  let event = response.events.iter().find(|e| e.body.starts_with("manager")).expect("blockedの活動があります");
  assert!(event.body.ends_with(&full));
  let calls = harness.recorded_calls();
  let pages: Vec<_> =
    calls.iter().filter(|call| call.name == "takt_get_goal" && call.arguments["recordId"].is_string()).collect();
  assert_eq!(pages.len(), 2);
  for (page, offset) in pages.iter().zip([0, 4095]) {
    assert_eq!(page.arguments["goalId"], "11111111-1111-4111-8111-111111111111");
    assert_eq!(page.arguments["recordId"], "33333333-3333-4333-8333-333333333333");
    assert_eq!(page.arguments["path"], "notifications.body");
    assert_eq!(page.arguments["textOffset"], offset);
  }
}

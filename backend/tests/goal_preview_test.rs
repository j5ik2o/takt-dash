//! MCP の一覧入力から gRPC 応答まで、preview の本文と切り詰めの印を検証する。

mod common;
mod grpc_harness;

use std::path::Path;

use common::{project, server_meta};
use grpc_harness::{client, start_server, GrpcHarness};
use prost::Message;
use serde_json::{json, Value};
use takt_dash_backend::{
  poller::Poller,
  takt_dash::v1::{GetGoalsRequest, Goal},
};

async fn listed_goals(goals: Value) -> Vec<Goal> {
  let harness = GrpcHarness::builder()
    .single_project(project("takt-dash", Path::new("/abs/takt-dash")), server_meta(), json!({ "goals": goals }))
    .build();
  let mut poller = Poller::new(
    harness.config_path().to_path_buf(),
    harness.activity.clone(),
    harness.store.clone(),
    harness.changes.clone(),
    harness.registry.clone(),
  );
  poller.read_cycle_once().await.expect("MCP の状態を読み取れます");
  poller.shutdown().await;
  let server = start_server(&harness).await;
  let result = client(server.address)
    .await
    .get_goals(GetGoalsRequest {})
    .await
    .expect("ゴール一覧を取得できます")
    .into_inner()
    .goals;
  server.stop().await;
  result
}

fn mcp_goal(id: &str, objective: &str) -> Value {
  json!({ "id": id, "status": "created", "executionStatus": "active", "objective": objective })
}

#[tokio::test]
async fn preview_respects_byte_limit_and_utf8_boundaries() {
  let cases = [
    ("below", "a".repeat(255), "a".repeat(255), false),
    ("exact", "a".repeat(256), "a".repeat(256), false),
    ("above", "a".repeat(257), "a".repeat(256), true),
    ("utf8", format!("{}あ", "a".repeat(255)), "a".repeat(255), true),
    ("utf8-exact", format!("a{}", "あ".repeat(85)), format!("a{}", "あ".repeat(85)), false),
    ("utf8-four-byte", format!("{}🦀", "a".repeat(253)), "a".repeat(253), true),
  ];
  let input: Vec<Value> = cases.iter().map(|(id, text, _, _)| mcp_goal(id, text)).collect();
  let goals = listed_goals(json!(input)).await;
  assert_eq!(goals.len(), cases.len());
  for (id, _, expected, truncated) in cases {
    let goal = goals.iter().find(|goal| goal.goal_id == id).expect("入力のゴールを返します");
    assert_eq!(goal.name, expected, "{id} の preview");
    assert_eq!(goal.name_truncated, truncated, "{id} の切り詰めの印");
  }
}

#[tokio::test]
async fn preview_uses_objective_omissions_instead_of_goal_truncated() {
  let text = "a".repeat(256);
  let input: Vec<Value> = [("complete", 256), ("omitted", 257)]
    .iter()
    .map(|(id, total_bytes)| {
      json!({
        "id": id, "status": "created", "executionStatus": "active", "objective": text,
        "truncated": true, "omissions": { "objective": { "totalBytes": total_bytes } }
      })
    })
    .collect();
  let goals = listed_goals(json!(input)).await;
  assert_eq!(goals.len(), 2);
  for (id, truncated) in [("complete", false), ("omitted", true)] {
    let goal = goals.iter().find(|goal| goal.goal_id == id).expect("入力のゴールを返します");
    assert_eq!(goal.name, text);
    assert_eq!(goal.name_truncated, truncated, "{id} は objective の元バイト数で判定します");
  }
}

#[tokio::test]
async fn preview_ignores_omissions_for_other_fields() {
  let input = json!([{
    "id": "g1", "status": "created", "executionStatus": "active", "objective": "確認。",
    "truncated": true,
    "omissions": { "questions": { "totalCount": 100 }, "constraints": { "totalBytes": 1000 } }
  }]);
  let goals = listed_goals(input).await;

  assert_eq!(goals[0].name, "確認。");
  assert!(!goals[0].name_truncated);
}

// 変更前の Goal.name の wire 契約だけを読む旧クライアント。
#[derive(Clone, PartialEq, Message)]
struct OldGoal {
  #[prost(string, tag = "4")]
  name: String,
}

#[test]
fn old_goal_wire_defaults_truncation_to_false() {
  let old = OldGoal { name: "a".repeat(256) };
  let decoded = Goal::decode(old.encode_to_vec().as_slice()).expect("旧応答を復元できます");
  assert_eq!(decoded.name, old.name);
  assert!(!decoded.name_truncated);
}

#[test]
fn old_reader_preserves_name_from_new_goal_wire() {
  let goal = Goal { name: "a".repeat(256), name_truncated: true, ..Goal::default() };
  let decoded = OldGoal::decode(goal.encode_to_vec().as_slice()).expect("旧クライアントが新応答を読めます");
  assert_eq!(decoded.name, goal.name);
}

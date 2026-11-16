//! gRPC の GetGoals が、状態ストアの全プロジェクトのゴールを、サーバー・プロジェクト・
//! 状態別のタスク件数・マージ待ち・プルリクエスト付きで1回ずつ返すことを検証する。

mod common;
mod grpc_harness;

use std::path::Path;

use common::*;
use grpc_harness::{client, start_server, GrpcHarness};
use serde_json::json;
use takt_dash_backend::{
  config::{Project, ServerMeta},
  model::{ExecutionStatus, GoalEntry, GoalStatus, ProjectSnapshot, TaskStatus},
  takt_dash::v1::{
    ExecutionStatus as ProtoExecutionStatus, GetGoalsRequest, GoalStatus as ProtoGoalStatus,
    TaskStatus as ProtoTaskStatus,
  },
};

fn studio_meta() -> ServerMeta {
  ServerMeta { name: "j5ik2o-mac-studio".to_owned(), kind: "Mac Studio".to_owned(), concurrency: 8 }
}

fn two_project_harness() -> GrpcHarness {
  let takt_dash = project("takt-dash", Path::new("/abs/takt-dash"));
  let esa = Project { server: "mac-studio".to_owned(), ..project("esa", Path::new("/abs/esa")) };
  let harness = GrpcHarness::builder()
    .single_project(takt_dash, server_meta(), json!({}))
    .single_project(esa, studio_meta(), json!({}))
    .build();
  let g1 = GoalEntry { awaiting_merge_at: Some(time("2026-11-13T10:00:00Z")), ..merge_goal("g1", 12) };
  let snapshot_1 = ProjectSnapshot {
    goals: vec![g1],
    tasks: vec![
      task_for_goal("implement", "g1", TaskStatus::Running, false),
      task_for_goal("merge", "g1", TaskStatus::Completed, true),
      task_for_goal("other-goal", "g2", TaskStatus::Failed, false),
    ],
    ..ProjectSnapshot::empty()
  };
  harness.store.record_success("takt-dash", snapshot_1, time("2026-11-13T12:00:00Z"));
  let snapshot_2 = ProjectSnapshot {
    goals: vec![goal("g2", GoalStatus::Created, ExecutionStatus::Paused)],
    tasks: vec![task_for_goal("design", "g2", TaskStatus::Completed, false)],
    ..ProjectSnapshot::empty()
  };
  harness.store.record_success("esa", snapshot_2, time("2026-11-13T12:00:00Z"));
  harness
}

fn timestamp(text: &str) -> prost_types::Timestamp {
  let time = time(text);
  prost_types::Timestamp { seconds: time.timestamp(), nanos: time.timestamp_subsec_nanos() as i32 }
}

#[tokio::test]
async fn get_goals_returns_every_goal_once_with_server_project_and_counts() {
  let harness = two_project_harness();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let goals = client.get_goals(GetGoalsRequest {}).await.expect("ゴール一覧を取得できます").into_inner().goals;

  assert_eq!(goals.len(), 2, "全サーバー・全プロジェクトのゴールを1回ずつ返します: {goals:?}");
  let g1 = goals.iter().find(|goal| goal.goal_id == "g1").expect("g1 を含みます");
  assert_eq!(g1.server_id, "mac-mini");
  assert_eq!(g1.project, "takt-dash");
  assert_eq!(g1.status, ProtoGoalStatus::AwaitingMerge as i32);
  assert_eq!(g1.execution_status, ProtoExecutionStatus::Active as i32);
  let pull_request = g1.pull_request.as_ref().expect("マージ待ちのゴールはプルリクエストを持ちます");
  assert_eq!(pull_request.number, 12);
  assert_eq!(pull_request.url, "https://example.test/pr/12");
  assert_eq!(g1.awaiting_merge_at, Some(timestamp("2026-11-13T10:00:00Z")));
  assert_eq!(g1.merge_task_last, Some(ProtoTaskStatus::Completed as i32), "仕上げタスクの最後の状態です");

  let counts = g1.task_counts.as_ref().expect("状態別の件数を持ちます");
  assert_eq!(counts.running, 1, "g1 の running の件数だけを数えます");
  assert_eq!(counts.completed, 1, "g1 の completed の件数だけを数えます");
  assert_eq!(counts.failed, 0, "他ゴールの failed を g1 の件数に混ぜません");
  assert_eq!(counts.pending, 0);

  let g2 = goals.iter().find(|goal| goal.goal_id == "g2").expect("g2 を含みます");
  assert_eq!(g2.server_id, "mac-studio");
  assert_eq!(g2.project, "esa");
  assert_eq!(g2.status, ProtoGoalStatus::Created as i32);
  assert_eq!(g2.execution_status, ProtoExecutionStatus::Paused as i32);
  assert!(g2.pull_request.is_none(), "プルリクエストのないゴールは空です");
  assert!(g2.awaiting_merge_at.is_none(), "マージ待ちでないゴールは起点時刻を持ちません");
  assert_eq!(g2.merge_task_last, None, "仕上げタスクのないゴールは最後の状態を持ちません");
  let counts = g2.task_counts.as_ref().expect("状態別の件数を持ちます");
  assert_eq!(counts.completed, 1, "g2 の completed の件数です");

  server.stop().await;
}

#[tokio::test]
async fn get_goals_returns_an_empty_list_when_the_store_is_empty() {
  let harness = GrpcHarness::builder()
    .single_project(project("takt-dash", Path::new("/abs/takt-dash")), server_meta(), json!({}))
    .build();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let goals = client.get_goals(GetGoalsRequest {}).await.expect("ゴールがなくても成功します").into_inner().goals;

  assert!(goals.is_empty(), "ストアが空なら空の一覧を返します: {goals:?}");

  server.stop().await;
}

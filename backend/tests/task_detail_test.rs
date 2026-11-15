//! gRPC の GetTaskDetail が、mock MCP サーバの takt_list_tasks を全ページ取得し、
//! プロジェクトとタスク名（任意のゴール ID）でタスク詳細を組み立てることを検証する。

mod common;
mod grpc_harness;

use std::path::Path;

use common::*;
use grpc_harness::{client, start_server, GrpcHarness};
use serde_json::{json, Value};
use takt_dash_backend::takt_dash::v1::{GetTaskDetailRequest, TaskStatus as ProtoTaskStatus};

const TASK_COUNT: usize = 120;
const TARGET_INDEX: usize = 109;

fn tasks() -> Vec<Value> {
  let mut tasks = Vec::new();
  for index in 0..TASK_COUNT {
    let mut task = json!({
      "name": format!("task-{index}"),
      "goalId": "g1",
      "status": if index == TARGET_INDEX { "completed" } else { "pending" },
      "summary": format!("要約 {index}"),
      "startedAt": "2026-11-13T09:00:00Z",
      "runSlug": format!("run-{index}"),
    });
    if index == TARGET_INDEX {
      task["mergeTask"] = json!(true);
      task["prNumber"] = json!(42);
    }
    tasks.push(task);
  }
  tasks
}

fn task_harness() -> GrpcHarness {
  GrpcHarness::builder()
    .single_project(project("takt-dash", Path::new("/abs/takt-dash")), server_meta(), json!({ "tasks": tasks() }))
    .build()
}

fn timestamp(text: &str) -> prost_types::Timestamp {
  let time = time(text);
  prost_types::Timestamp { seconds: time.timestamp(), nanos: time.timestamp_subsec_nanos() as i32 }
}

#[tokio::test]
async fn get_task_detail_collects_a_task_beyond_the_first_page() {
  let harness = task_harness();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let detail = client
    .get_task_detail(GetTaskDetailRequest {
      project:   "takt-dash".to_owned(),
      task_name: "task-109".to_owned(),
      goal_id:   Some("g1".to_owned()),
    })
    .await
    .expect("2 ページ目より後にあるタスクも取得できます")
    .into_inner();

  assert_eq!(detail.project, "takt-dash");
  assert_eq!(detail.task_name, "task-109");
  assert_eq!(detail.goal_id.as_deref(), Some("g1"));
  assert_eq!(detail.status, ProtoTaskStatus::Completed as i32);
  assert_eq!(detail.summary, "要約 109");
  assert_eq!(detail.started_at, Some(timestamp("2026-11-13T09:00:00Z")));
  assert_eq!(detail.run_slug.as_deref(), Some("run-109"));
  assert_eq!(detail.pull_request_number, Some(42), "仕上げタスクのプルリクエスト番号を返します");

  server.stop().await;
}

#[tokio::test]
async fn get_task_detail_finds_a_task_without_a_goal_id() {
  let harness = task_harness();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let detail = client
    .get_task_detail(GetTaskDetailRequest {
      project:   "takt-dash".to_owned(),
      task_name: "task-109".to_owned(),
      goal_id:   None,
    })
    .await
    .expect("ゴール ID を指定しなくてもタスク名で取得できます")
    .into_inner();

  assert_eq!(detail.task_name, "task-109");
  assert_eq!(detail.goal_id.as_deref(), Some("g1"));

  server.stop().await;
}

#[tokio::test]
async fn get_task_detail_returns_not_found_when_the_goal_does_not_match() {
  let harness = task_harness();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let status = client
    .get_task_detail(GetTaskDetailRequest {
      project:   "takt-dash".to_owned(),
      task_name: "task-109".to_owned(),
      goal_id:   Some("g2".to_owned()),
    })
    .await
    .expect_err("ゴール ID が一致しないタスクは見つかりません");

  assert_eq!(status.code(), tonic::Code::NotFound, "ゴール不一致は Not Found です: {status:?}");

  server.stop().await;
}

#[tokio::test]
async fn get_task_detail_returns_not_found_for_an_unknown_task() {
  let harness = task_harness();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let status = client
    .get_task_detail(GetTaskDetailRequest {
      project:   "takt-dash".to_owned(),
      task_name: "does-not-exist".to_owned(),
      goal_id:   None,
    })
    .await
    .expect_err("存在しないタスクはエラーです");

  assert_eq!(status.code(), tonic::Code::NotFound, "未知のタスクは Not Found です: {status:?}");

  server.stop().await;
}

#[tokio::test]
async fn get_task_detail_returns_not_found_for_an_unknown_project() {
  let harness = task_harness();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let status = client
    .get_task_detail(GetTaskDetailRequest {
      project:   "does-not-exist".to_owned(),
      task_name: "task-109".to_owned(),
      goal_id:   None,
    })
    .await
    .expect_err("設定にないプロジェクトはエラーです");

  assert_eq!(status.code(), tonic::Code::NotFound, "未知のプロジェクトは Not Found です: {status:?}");

  server.stop().await;
}

#[tokio::test]
async fn get_task_detail_rejects_an_empty_project_as_invalid_argument() {
  let harness = task_harness();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let status = client
    .get_task_detail(GetTaskDetailRequest {
      project:   String::new(),
      task_name: "task-109".to_owned(),
      goal_id:   None,
    })
    .await
    .expect_err("空のプロジェクトは拒否します");

  assert_eq!(status.code(), tonic::Code::InvalidArgument, "空のプロジェクトは Invalid Argument です: {status:?}");

  server.stop().await;
}

#[tokio::test]
async fn get_task_detail_rejects_an_empty_task_name_as_invalid_argument() {
  let harness = task_harness();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let status = client
    .get_task_detail(GetTaskDetailRequest {
      project:   "takt-dash".to_owned(),
      task_name: String::new(),
      goal_id:   None,
    })
    .await
    .expect_err("空のタスク名は拒否します");

  assert_eq!(status.code(), tonic::Code::InvalidArgument, "空のタスク名は Invalid Argument です: {status:?}");

  server.stop().await;
}

#[tokio::test]
async fn get_task_detail_stops_following_pages_at_the_limit_instead_of_returning_a_partial_task() {
  let tasks: Vec<Value> = (0..6000)
    .map(|index| {
      json!({
        "name": format!("overflow-task-{index}"),
        "goalId": "g1",
        "status": "pending",
        "summary": format!("要約 {index}"),
      })
    })
    .collect();
  let harness = GrpcHarness::builder()
    .single_project(project("takt-dash", Path::new("/abs/takt-dash")), server_meta(), json!({ "tasks": tasks }))
    .build();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let status = client
    .get_task_detail(GetTaskDetailRequest {
      project:   "takt-dash".to_owned(),
      task_name: "overflow-task-5999".to_owned(),
      goal_id:   None,
    })
    .await
    .expect_err("ページ上限を超えた応答は部分結果ではなくエラーにします");

  assert_eq!(status.code(), tonic::Code::Internal, "ページ上限の超過は Internal です: {status:?}");

  server.stop().await;
}

#[tokio::test]
async fn get_task_detail_maps_a_takt_mcp_tool_error_to_unavailable() {
  let harness = GrpcHarness::builder()
    .single_project(
      project("takt-dash", Path::new("/abs/takt-dash")),
      server_meta(),
      json!({ "tasks": [], "isError": ["takt_list_tasks"] }),
    )
    .build();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let status = client
    .get_task_detail(GetTaskDetailRequest {
      project:   "takt-dash".to_owned(),
      task_name: "task-109".to_owned(),
      goal_id:   None,
    })
    .await
    .expect_err("道具エラーは失敗になります");

  assert_eq!(status.code(), tonic::Code::Unavailable, "takt-mcp の道具エラーは Unavailable です: {status:?}");

  server.stop().await;
}

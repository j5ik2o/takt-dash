//! ゴール詳細とタスク詳細の読み取りが、ドメイン型で生成できない識別子を invalid_argument
//! として拒否することを
//! 検証する。既存の書き込みハンドラと同じ入力の境界に揃っていることを確かめる。

mod common;
mod grpc_harness;

use std::path::Path;

use common::*;
use grpc_harness::{client, start_server, GrpcHarness};
use serde_json::json;
use takt_dash_backend::takt_dash::v1::{GetGoalDetailRequest, GetTaskDetailRequest};

const INVALID_IDENTIFIER: &str = "bad\u{1}id";

fn harness() -> GrpcHarness {
  GrpcHarness::builder()
    .single_project(project("takt-dash", Path::new("/abs/takt-dash")), server_meta(), json!({}))
    .build()
}

#[tokio::test]
async fn get_goal_detail_rejects_a_goal_id_that_fails_domain_validation() {
  let server = start_server(&harness()).await;
  let mut client = client(server.address).await;

  let status = client
    .get_goal_detail(GetGoalDetailRequest { goal_id: INVALID_IDENTIFIER.to_owned() })
    .await
    .expect_err("ドメイン型で生成できない goalId は拒否します");

  assert_eq!(
    status.code(),
    tonic::Code::InvalidArgument,
    "ドメイン型で生成できない goalId は Invalid Argument です: {status:?}"
  );
  server.stop().await;
}

#[tokio::test]
async fn get_task_detail_rejects_a_project_that_fails_domain_validation() {
  let server = start_server(&harness()).await;
  let mut client = client(server.address).await;

  let status = client
    .get_task_detail(GetTaskDetailRequest {
      project:   INVALID_IDENTIFIER.to_owned(),
      task_name: "task-1".to_owned(),
      goal_id:   None,
    })
    .await
    .expect_err("ドメイン型で生成できない project は拒否します");

  assert_eq!(
    status.code(),
    tonic::Code::InvalidArgument,
    "ドメイン型で生成できない project は Invalid Argument です: {status:?}"
  );
  server.stop().await;
}

#[tokio::test]
async fn get_task_detail_rejects_a_task_name_that_fails_domain_validation() {
  let server = start_server(&harness()).await;
  let mut client = client(server.address).await;

  let status = client
    .get_task_detail(GetTaskDetailRequest {
      project:   "takt-dash".to_owned(),
      task_name: INVALID_IDENTIFIER.to_owned(),
      goal_id:   None,
    })
    .await
    .expect_err("ドメイン型で生成できない taskName は拒否します");

  assert_eq!(
    status.code(),
    tonic::Code::InvalidArgument,
    "ドメイン型で生成できない taskName は Invalid Argument です: {status:?}"
  );
  server.stop().await;
}

#[tokio::test]
async fn get_task_detail_rejects_a_goal_id_that_fails_domain_validation() {
  let server = start_server(&harness()).await;
  let mut client = client(server.address).await;

  let status = client
    .get_task_detail(GetTaskDetailRequest {
      project:   "takt-dash".to_owned(),
      task_name: "task-1".to_owned(),
      goal_id:   Some(INVALID_IDENTIFIER.to_owned()),
    })
    .await
    .expect_err("ドメイン型で生成できない goalId は拒否します");

  assert_eq!(
    status.code(),
    tonic::Code::InvalidArgument,
    "ドメイン型で生成できない goalId は Invalid Argument です: {status:?}"
  );
  server.stop().await;
}

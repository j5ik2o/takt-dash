//! gRPC の GetRunDetail が、mock MCP サーバの takt_get_run 応答から GetRunDetailResponse を
//! 組み立てることと、状態ストアにない runSlug を Not Found 相当のステータスで返すことを検証する。

mod common;
mod grpc_harness;

use std::path::Path;

use chrono::{Duration, Utc};
use common::*;
use grpc_harness::{client, start_server, GrpcHarness};
use serde_json::json;
use takt_dash_backend::{
  model::{ExecutionStatus, GoalStatus, ProjectSnapshot},
  takt_dash::v1::{GetRunDetailRequest, RunStatus},
};

fn project_dir() -> &'static Path {
  Path::new("/abs/takt-dash")
}

/// 状態ストアに run-1 を持つ 1 プロジェクト分の状態を入れる。
fn seeded_harness(state: serde_json::Value) -> GrpcHarness {
  let harness =
    GrpcHarness::builder().single_project(project("takt-dash", project_dir()), server_meta(), state).build();
  let snapshot = ProjectSnapshot {
    goals:         vec![goal("g1", GoalStatus::Created, ExecutionStatus::Active)],
    tasks:         vec![],
    inquiries:     vec![],
    runs:          vec![run("run-1", "implement", time("2026-11-13T09:06:00Z"))],
    notifications: vec![],
  };
  harness.store.record_success("takt-dash", snapshot, time("2026-11-13T12:00:00Z"));
  harness
}

/// `run_state()` の `runs.run-1.lastLogAt` だけを差し替えた状態を作る。log_silent
/// の判定を観測するために使う。
fn run_state_with_last_log_at(last_log_at: &str) -> serde_json::Value {
  let mut state = run_state();
  state["runs"]["run-1"]["lastLogAt"] = serde_json::Value::String(last_log_at.to_owned());
  state
}

fn run_state() -> serde_json::Value {
  json!({
    "tasks": [
      { "name": "implement", "goalId": "g1", "status": "running", "startedAt": "2026-11-13T09:00:00Z", "runSlug": "run-1", "currentStep": "implement" }
    ],
    "runs": {
      "run-1": {
        "runSlug": "run-1",
        "sessionId": "sess-1",
        "summary": "実装の run",
        "workflow": "development-core",
        "status": "running",
        "startedAt": "2026-11-13T09:00:00Z",
        "currentStep": "implement",
        "currentIteration": 3,
        "currentStepStartedAt": "2026-11-13T09:05:00Z",
        "currentStepExecutionCount": 2,
        "phase": 1,
        "logTail": "最後のログの行です。",
        "lastLogAt": "2026-11-13T09:06:00Z",
        "taskText": "タスクの本文です。",
        "stepLogs": [
          { "step": "plan", "persona": "architect", "status": "completed", "content": "計画の本文", "timestamp": "2026-11-13T09:01:00Z", "iteration": 1 },
          { "step": "implement", "persona": "coder", "status": "running", "content": "実装の本文", "timestamp": "2026-11-13T09:06:00Z", "iteration": 2 }
        ],
        "reports": [
          { "filename": "plan.md", "content": "計画のレポート", "sizeBytes": 120 }
        ],
        "instructions": [
          { "instructionId": 7, "issuedAt": "2026-11-13T08:50:00Z", "state": "applied", "content": "指示の本文" }
        ]
      }
    }
  })
}

#[tokio::test]
async fn get_run_detail_assembles_the_response_from_the_mock_takt_get_run() {
  let harness = seeded_harness(run_state());
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let detail = client
    .get_run_detail(GetRunDetailRequest { run_slug: "run-1".to_owned() })
    .await
    .expect("run 詳細を取得できます")
    .into_inner();

  assert_eq!(detail.run_slug, "run-1");
  assert_eq!(detail.session_id, "sess-1");
  assert_eq!(detail.server_id, "mac-mini");
  assert_eq!(detail.project, "takt-dash");
  assert_eq!(detail.goal_id, "g1");
  assert_eq!(detail.task_name, "implement");
  assert_eq!(detail.task_text, "タスクの本文です。");
  assert_eq!(detail.summary, "実装の run");
  assert_eq!(detail.workflow, "development-core");
  assert_eq!(detail.status, RunStatus::Running as i32);
  assert_eq!(detail.current_step.as_deref(), Some("implement"));
  assert_eq!(detail.current_iteration, Some(3));
  assert_eq!(detail.current_step_started_at, Some(timestamp("2026-11-13T09:05:00Z")));
  assert_eq!(detail.current_step_execution_count, Some(2));
  assert_eq!(detail.phase, Some(1));
  assert_eq!(detail.log_tail, "最後のログの行です。");
  assert_eq!(detail.last_log_at, Some(timestamp("2026-11-13T09:06:00Z")));

  assert_eq!(detail.step_logs.len(), 2);
  assert_eq!(detail.step_logs[0].step_name, "plan");
  assert_eq!(detail.step_logs[0].persona, "architect");
  assert_eq!(detail.step_logs[0].content, "計画の本文");
  assert_eq!(detail.step_logs[0].iteration, 1);
  assert_eq!(detail.step_logs[1].step_name, "implement");
  assert_eq!(detail.step_logs[1].status, "running");

  assert_eq!(detail.reports.len(), 1);
  assert_eq!(detail.reports[0].filename, "plan.md");
  assert_eq!(detail.reports[0].content, "計画のレポート");
  assert_eq!(detail.reports[0].size_bytes, 120);

  assert_eq!(detail.instructions.len(), 1);
  assert_eq!(detail.instructions[0].instruction_id, 7);
  assert_eq!(detail.instructions[0].state, "applied");
  assert_eq!(detail.instructions[0].content, "指示の本文");

  assert!(detail.server_connected, "読めていたプロジェクトは接続ありです");
  assert_eq!(detail.server_last_read_at, Some(timestamp("2026-11-13T12:00:00Z")));

  server.stop().await;
}

#[tokio::test]
async fn get_run_detail_reports_server_disconnected_while_keeping_last_read_at() {
  let harness = seeded_harness(run_state());
  harness.store.record_failure("takt-dash");
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let detail = client
    .get_run_detail(GetRunDetailRequest { run_slug: "run-1".to_owned() })
    .await
    .expect("応答がない間も run 詳細を組み立てられます")
    .into_inner();

  assert!(!detail.server_connected, "応答がなければ server_connected は偽です");
  assert_eq!(detail.server_last_read_at, Some(timestamp("2026-11-13T12:00:00Z")), "最後に読めた時刻を返します");

  server.stop().await;
}

#[tokio::test]
async fn get_run_detail_returns_not_found_for_an_unknown_run_slug() {
  let harness = seeded_harness(run_state());
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let status = client
    .get_run_detail(GetRunDetailRequest { run_slug: "does-not-exist".to_owned() })
    .await
    .expect_err("状態ストアにない runSlug はエラーです");

  assert_eq!(status.code(), tonic::Code::NotFound, "見つからない runSlug は Not Found です: {status:?}");

  server.stop().await;
}

#[tokio::test]
async fn get_run_detail_rejects_an_empty_run_slug_as_invalid_argument() {
  let harness = seeded_harness(run_state());
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let status = client
    .get_run_detail(GetRunDetailRequest { run_slug: String::new() })
    .await
    .expect_err("空の runSlug は拒否します");

  assert_eq!(status.code(), tonic::Code::InvalidArgument, "空の runSlug は Invalid Argument です: {status:?}");

  server.stop().await;
}

fn timestamp(text: &str) -> prost_types::Timestamp {
  let time = time(text);
  prost_types::Timestamp { seconds: time.timestamp(), nanos: time.timestamp_subsec_nanos() as i32 }
}

#[tokio::test]
async fn get_run_detail_marks_log_silent_false_when_the_last_log_is_within_the_threshold() {
  // 最後のログが今（既定 10 分の閾値の内側）なら log_silent は偽。lastLogAt を started_at へ
  // 誤配線すると、このケースで偽のままとなり誤りを検出できないため、下の外側ケースと対で観測する。
  let last_log_at = Utc::now() - Duration::minutes(1);
  let harness = seeded_harness(run_state_with_last_log_at(&last_log_at.to_rfc3339()));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let detail = client
    .get_run_detail(GetRunDetailRequest { run_slug: "run-1".to_owned() })
    .await
    .expect("run 詳細を取得できます")
    .into_inner();

  assert!(!detail.log_silent, "閾値内側の最後のログは無音ではありません");

  server.stop().await;
}

#[tokio::test]
async fn get_run_detail_marks_log_silent_true_when_the_last_log_exceeds_the_threshold() {
  // 最後のログが 30 分前（既定 10 分の閾値の外側）なら log_silent は真。
  let last_log_at = Utc::now() - Duration::minutes(30);
  let harness = seeded_harness(run_state_with_last_log_at(&last_log_at.to_rfc3339()));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let detail = client
    .get_run_detail(GetRunDetailRequest { run_slug: "run-1".to_owned() })
    .await
    .expect("run 詳細を取得できます")
    .into_inner();

  assert!(detail.log_silent, "閾値外側の最後のログは無音です");

  server.stop().await;
}

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
  model::{ExecutionStatus, GoalStatus, ProjectSnapshot, TaskEntry, TaskStatus},
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
    proposals:     vec![],
  };
  harness.store.record_success("takt-dash", snapshot, time("2026-11-13T12:00:00Z"));
  harness
}

/// `run_state()` の `runs.run-1.lastLogAt` と末尾の stepLog の timestamp を同じ値へ差し替えた
/// 状態を作る。詳細の `last_log_at` は応答の `lastLogAt` を最優先するため、末尾 stepLog も
/// 揃えて log_silent の閾値判定だけを観測できるようにする。
fn run_state_with_last_log_at(last_log_at: &str) -> serde_json::Value {
  let mut state = run_state();
  state["runs"]["run-1"]["lastLogAt"] = serde_json::Value::String(last_log_at.to_owned());
  let step_logs = state["runs"]["run-1"]["stepLogs"].as_array_mut().expect("stepLogs は配列です");
  if let Some(last) = step_logs.last_mut() {
    last["timestamp"] = serde_json::Value::String(last_log_at.to_owned());
  }
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

#[tokio::test]
async fn 完了した実行の詳細は実行中一覧になくても取得できる() {
  // 状態ストアの runs が空でも、完了タスクの runSlug からプロジェクトを解決して
  // 詳細を返します。mock の応答とタスク一覧にゴールとタスク名がないため、状態ストアの
  // 完了タスクからそれらを補うことも観測します。
  let harness = GrpcHarness::builder()
    .single_project(
      project("takt-dash", project_dir()),
      server_meta(),
      json!({
        "runs": { "run-1": {
          "runSlug": "run-1",
          "sessionId": "sess-1",
          "summary": "完了した run",
          "status": "completed",
          "startedAt": "2026-11-13T09:00:00Z",
          "currentStep": "review",
          "lastLogAt": "2026-11-13T09:30:00Z",
          "stepLogs": []
        } }
      }),
    )
    .build();
  let snapshot = ProjectSnapshot {
    goals:         vec![],
    tasks:         vec![TaskEntry {
      name:         "implement".to_owned(),
      goal_id:      Some("g1".to_owned()),
      status:       TaskStatus::Completed,
      started_at:   Some(time("2026-11-13T09:00:00Z")),
      run_slug:     Some("run-1".to_owned()),
      current_step: Some("review".to_owned()),
      merge_task:   false,
    }],
    inquiries:     vec![],
    runs:          vec![],
    notifications: vec![],
    proposals:     vec![],
  };
  harness.store.record_success("takt-dash", snapshot, time("2026-11-13T12:00:00Z"));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let detail = client
    .get_run_detail(GetRunDetailRequest { run_slug: "run-1".to_owned() })
    .await
    .expect("完了後も同じ runSlug で詳細を取得できます")
    .into_inner();

  assert_eq!(detail.run_slug, "run-1");
  assert_eq!(detail.project, "takt-dash", "完了タスクの runSlug からプロジェクトを解決します");
  assert_eq!(detail.goal_id, "g1", "状態ストアの完了タスクからゴールを補います");
  assert_eq!(detail.task_name, "implement", "状態ストアの完了タスクからタスク名を補います");
  assert_eq!(detail.status, RunStatus::Completed as i32);

  server.stop().await;
}

/// 51 件の stepLogs を持つ状態を作る。先頭ページは 50 件で、51 件目（2 ページ目）の
/// timestamp を `latest_step_at` にする。`last_log_at` を `null` にすると応答の `lastLogAt` を
/// 持たない状態を作れる。
fn run_state_with_latest_step_on_the_second_page(
  last_log_at: serde_json::Value,
  latest_step_at: &str,
) -> serde_json::Value {
  let mut step_logs = Vec::new();
  for index in 0..50 {
    step_logs.push(json!({
      "step": format!("step{index}"),
      "persona": "coder",
      "status": "completed",
      "content": format!("本文 {index}"),
      "timestamp": format!("2026-11-13T08:{index:02}:00Z"),
      "iteration": 1
    }));
  }
  step_logs.push(json!({
    "step": "step50",
    "persona": "coder",
    "status": "running",
    "content": "本文 50",
    "timestamp": latest_step_at,
    "iteration": 2
  }));
  json!({
    "tasks": [
      { "name": "implement", "goalId": "g1", "status": "running", "startedAt": "2026-11-13T09:00:00Z", "runSlug": "run-1", "currentStep": "step50" }
    ],
    "runs": {
      "run-1": {
        "runSlug": "run-1",
        "sessionId": "sess-1",
        "summary": "実装の run",
        "workflow": "development-core",
        "status": "running",
        "startedAt": "2026-11-13T09:00:00Z",
        "currentStep": "step50",
        "currentIteration": 2,
        "currentStepStartedAt": "2026-11-13T09:05:00Z",
        "currentStepExecutionCount": 2,
        "phase": 1,
        "logTail": "最後のログの行です。",
        "lastLogAt": last_log_at,
        "taskText": "タスクの本文です。",
        "stepLogs": step_logs,
        "reports": [],
        "instructions": []
      }
    }
  })
}

#[tokio::test]
async fn get_run_detail_last_log_at_prefers_the_response_last_log_at_over_a_newer_step_log() {
  // 応答の lastLogAt が 2 ページ目の stepLog より古くても、詳細の last_log_at は lastLogAt を
  // 採る。stepLog を優先する実装はここで新しい stepLog の時刻を返して落ちる。
  let harness = seeded_harness(run_state_with_latest_step_on_the_second_page(
    json!("2026-11-13T09:10:00Z"),
    "2026-11-13T09:40:00Z",
  ));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let detail = client
    .get_run_detail(GetRunDetailRequest { run_slug: "run-1".to_owned() })
    .await
    .expect("run 詳細を取得できます")
    .into_inner();

  assert_eq!(
    detail.last_log_at,
    Some(timestamp("2026-11-13T09:10:00Z")),
    "詳細の最終ログ時刻は応答の lastLogAt を最優先します"
  );

  server.stop().await;
}

#[tokio::test]
async fn get_run_detail_log_silent_follows_the_response_last_log_at_over_a_recent_step_log() {
  // lastLogAt は無音の閾値（既定 10 分）を超える過去だが、2 ページ目の stepLog が今（1 分前）に
  // ある。詳細の log_silent は lastLogAt に従って真になる。stepLog
  // を優先する実装は偽を返して落ちる。
  let recent = (Utc::now() - Duration::minutes(1)).to_rfc3339();
  let stale_last_log_at = (Utc::now() - Duration::minutes(30)).to_rfc3339();
  let harness = seeded_harness(run_state_with_latest_step_on_the_second_page(json!(stale_last_log_at), &recent));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let detail = client
    .get_run_detail(GetRunDetailRequest { run_slug: "run-1".to_owned() })
    .await
    .expect("run 詳細を取得できます")
    .into_inner();

  assert!(detail.log_silent, "lastLogAt が閾値外側なら stepLog の新しさに関わらず無音です");

  server.stop().await;
}

#[tokio::test]
async fn get_run_detail_last_log_at_follows_the_newest_step_log_on_the_second_page_without_last_log_at() {
  // 応答の lastLogAt が無いとき、先頭ページだけでなく 2 ページ目（51 件目）の stepLog の時刻を
  // 最終ログ時刻に採る。先頭ページの末尾だけを見る実装は 08:49 を返して落ちる。
  let harness =
    seeded_harness(run_state_with_latest_step_on_the_second_page(serde_json::Value::Null, "2026-11-13T09:40:00Z"));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let detail = client
    .get_run_detail(GetRunDetailRequest { run_slug: "run-1".to_owned() })
    .await
    .expect("run 詳細を取得できます")
    .into_inner();

  assert_eq!(
    detail.last_log_at,
    Some(timestamp("2026-11-13T09:40:00Z")),
    "lastLogAt が無ければ全ページの stepLogs の最新時刻を採ります"
  );
  assert_eq!(detail.step_logs.len(), 51, "2 ページ目の stepLogs を重複させず全 51 件を返します");

  server.stop().await;
}

#[tokio::test]
async fn get_run_detail_last_log_at_ignores_a_step_log_without_timestamp() {
  // 2 ページ目に timestamp を持たない stepLog があっても、欠落時刻（UNIX_EPOCH）を最新として
  // 採らない。末尾の stepLog をそのまま最新とみなす実装は UNIX_EPOCH を返して落ちる。
  let mut state = run_state_with_latest_step_on_the_second_page(serde_json::Value::Null, "2026-11-13T09:40:00Z");
  let step_logs = state["runs"]["run-1"]["stepLogs"].as_array_mut().expect("stepLogs は配列です");
  step_logs.push(json!({ "step": "step51", "persona": "coder", "status": "running", "content": "時刻なし" }));
  let harness = seeded_harness(state);
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let detail = client
    .get_run_detail(GetRunDetailRequest { run_slug: "run-1".to_owned() })
    .await
    .expect("run 詳細を取得できます")
    .into_inner();

  assert_eq!(
    detail.last_log_at,
    Some(timestamp("2026-11-13T09:40:00Z")),
    "timestamp の無い stepLog は最新時刻の候補にしません"
  );

  server.stop().await;
}

/// 3 件の stepLogs がすべて timestamp を持たない状態を作る。`current_step_started_at` が
/// `Some` なら `currentStepStartedAt` を、`None` なら応答から当該キーを省く。
fn run_state_with_all_step_logs_missing_timestamp(current_step_started_at: Option<&str>) -> serde_json::Value {
  let mut run = json!({
    "runSlug": "run-1",
    "sessionId": "sess-1",
    "summary": "実装の run",
    "workflow": "development-core",
    "status": "running",
    "startedAt": "2026-11-13T09:00:00Z",
    "currentStep": "implement",
    "stepLogs": [
      { "step": "plan", "persona": "architect", "status": "completed", "content": "計画の本文", "iteration": 1 },
      { "step": "implement", "persona": "coder", "status": "running", "content": "実装の本文", "iteration": 2 },
      { "step": "implement", "persona": "coder", "status": "running", "content": "実装の続き", "iteration": 2 }
    ],
    "reports": [],
    "instructions": []
  });
  if let Some(started_at) = current_step_started_at {
    run["currentStepStartedAt"] = serde_json::Value::String(started_at.to_owned());
  }
  json!({
    "tasks": [
      { "name": "implement", "goalId": "g1", "status": "running", "startedAt": "2026-11-13T09:00:00Z", "runSlug": "run-1", "currentStep": "implement" }
    ],
    "runs": { "run-1": run }
  })
}

#[tokio::test]
async fn get_run_detail_last_log_at_uses_current_step_started_at_when_no_step_log_has_a_timestamp() {
  // 全 stepLog が timestamp 欠落、lastLogAt 無し、currentStepStartedAt 有り。欠落時刻を最新と
  // みなす実装はここで UNIX_EPOCH を採るが、第 2 候補は currentStepStartedAt でなければならない。
  let harness = seeded_harness(run_state_with_all_step_logs_missing_timestamp(Some("2026-11-13T09:05:00Z")));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let detail = client
    .get_run_detail(GetRunDetailRequest { run_slug: "run-1".to_owned() })
    .await
    .expect("run 詳細を取得できます")
    .into_inner();

  assert_eq!(
    detail.last_log_at,
    Some(timestamp("2026-11-13T09:05:00Z")),
    "時刻付き stepLog が無いときは currentStepStartedAt を採り、欠落時刻は採りません"
  );

  server.stop().await;
}

#[tokio::test]
async fn get_run_detail_last_log_at_ignores_missing_timestamps_and_falls_back_to_started_at() {
  // 全 stepLog が timestamp 欠落、lastLogAt も currentStepStartedAt も無し。除外を落とした実装は
  // 欠落時刻（UNIX_EPOCH）を最新として採るが、run の開始時刻に落ちなければならない。
  let harness = seeded_harness(run_state_with_all_step_logs_missing_timestamp(None));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let detail = client
    .get_run_detail(GetRunDetailRequest { run_slug: "run-1".to_owned() })
    .await
    .expect("run 詳細を取得できます")
    .into_inner();

  assert_eq!(
    detail.last_log_at,
    Some(timestamp("2026-11-13T09:00:00Z")),
    "時刻付き stepLog も currentStepStartedAt も無いときは startedAt を採ります"
  );

  server.stop().await;
}

//! gRPC の GetRunDetail が、mock MCP サーバの takt_get_run 応答から GetRunDetailResponse を
//! 組み立てることと、状態ストアにない runSlug を Not Found 相当のステータスで返すことを検証する。

mod common;
mod grpc_harness;

use std::{io::Write, path::Path};

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

fn write_mock_state(directory: &Path, state: serde_json::Value) -> std::path::PathBuf {
  let path = directory.join("state.json");
  let mut file = std::fs::File::create(&path).expect("状態ファイルを作れます");
  file.write_all(state.to_string().as_bytes()).expect("状態ファイルへ書けます");
  path
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

/// `run_state()` の `runs.run-1` から、`startedAt` を消して `startTime` だけを持つ実行中の run
/// にする。実 takt-mcp が実行中の run で返す形（issue #33）です。タスク側の `startedAt` も
/// 消し、`startedAt` 由来の補完が働かないことを確かめられるようにします。
fn run_state_with_start_time_only(start_time: &str) -> serde_json::Value {
  let mut state = run_state();
  let mut run = state["runs"]["run-1"].clone();
  run.as_object_mut().expect("run-1 はオブジェクトです").remove("startedAt");
  run["startTime"] = serde_json::Value::String(start_time.to_owned());
  state["runs"]["run-1"] = run;
  let tasks = state["tasks"].as_array_mut().expect("tasks は配列です");
  for task in tasks {
    task.as_object_mut().expect("task はオブジェクトです").remove("startedAt");
  }
  state
}

/// `run_state()` の `runs.run-1` から開始時刻のフィールド（`startedAt`/`startTime`）をすべて
/// 消し、`lastLogAt` も消す。開始時刻も最後のログの時刻も無い run を表します。
fn run_state_without_any_start_or_log_time() -> serde_json::Value {
  let mut state = run_state_with_start_time_only("2026-11-13T09:00:00Z");
  let mut run = state["runs"]["run-1"].clone();
  let run = run.as_object_mut().expect("run-1 はオブジェクトです");
  run.remove("startTime");
  run.remove("lastLogAt");
  run.remove("currentStepStartedAt");
  state["runs"]["run-1"] = serde_json::Value::Object(run.clone());
  state
}

#[tokio::test]
async fn a_run_without_any_start_time_is_listed_and_reaches_run_detail() {
  // 実行中の run が開始時刻も最後のログの時刻も持たない場合でも、一覧の読み取りが失敗せず
  // run が状態ストアに入り、詳細取得に到達できることを観測する。一覧が解析エラーで全体
  // 失敗すると run が状態ストアに現れず GetRunDetail が NOT_FOUND になるため、その経路を閉じる。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state_path = write_mock_state(
    directory.path(),
    json!({
      "tasks": [ { "name": "implement", "status": "running", "runSlug": "run-1" } ],
      "runs": { "run-1": { "runSlug": "run-1", "currentStep": "implement", "stepLogs": [] } }
    }),
  );
  let mut reader = mock_client(mock_config(directory.path(), &state_path, None));
  let written = tool_set(&mut reader, directory.path())
    .read_snapshot(None)
    .await
    .expect("開始時刻もログ時刻も無い run でも一覧を読み取れます");
  reader.shutdown().await;
  assert_eq!(written.runs.len(), 1, "開始時刻が無くても run は一覧に含まれます");
  assert_eq!(written.runs[0].started_at, None, "開始時刻は未設定として保持します");

  let harness = seeded_harness(run_state_without_any_start_or_log_time());
  harness.store.record_success("takt-dash", written, time("2026-11-13T12:00:00Z"));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;
  let detail = client
    .get_run_detail(GetRunDetailRequest { run_slug: "run-1".to_owned() })
    .await
    .expect("一覧に載った run は詳細取得に到達できます")
    .into_inner();
  assert_eq!(detail.started_at, None, "詳細の開始時刻も未設定で返ります");
  assert_eq!(detail.run_slug, "run-1");

  server.stop().await;
}

#[tokio::test]
async fn get_run_detail_reads_start_time_when_started_at_is_absent() {
  // 実行中の run は開始時刻を `startTime` で返す。`startedAt` だけを読む実装では解析エラーに
  // なり詳細が取れないため、開始時刻として `startTime` の値を返すことを観測する。
  let harness = seeded_harness(run_state_with_start_time_only("2026-11-13T08:40:00Z"));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let detail = client
    .get_run_detail(GetRunDetailRequest { run_slug: "run-1".to_owned() })
    .await
    .expect("startTime だけを持つ run でも詳細を取得できます")
    .into_inner();

  assert_eq!(detail.started_at, Some(timestamp("2026-11-13T08:40:00Z")), "開始時刻は startTime の値です");

  server.stop().await;
}

#[tokio::test]
async fn get_run_detail_prefers_started_at_over_start_time() {
  // 両方ある場合は `startedAt` を採る（既存の観測可能な挙動を維持する）。
  let mut state = run_state();
  state["runs"]["run-1"]["startTime"] = serde_json::Value::String("2026-11-13T08:40:00Z".to_owned());
  let harness = seeded_harness(state);
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let detail = client
    .get_run_detail(GetRunDetailRequest { run_slug: "run-1".to_owned() })
    .await
    .expect("run 詳細を取得できます")
    .into_inner();

  assert_eq!(detail.started_at, Some(timestamp("2026-11-13T09:00:00Z")), "startedAt を startTime で上書きしません");

  server.stop().await;
}

#[tokio::test]
async fn get_run_detail_returns_without_start_time_instead_of_failing() {
  // 開始時刻がどのフィールドにも無くても詳細は成功し、開始時刻は未設定で返る。他の項目は
  // 従来どおり返るので、ページ全体が 500 にならないことを観測する。
  let harness = seeded_harness(run_state_without_any_start_or_log_time());
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let detail = client
    .get_run_detail(GetRunDetailRequest { run_slug: "run-1".to_owned() })
    .await
    .expect("開始時刻が無い run でも詳細を取得できます")
    .into_inner();

  assert_eq!(detail.started_at, None, "開始時刻が取れないときは未設定で返します");
  assert_eq!(detail.run_slug, "run-1");
  assert_eq!(detail.task_name, "implement", "開始時刻が無くても他の項目は返ります");
  assert_eq!(detail.step_logs.len(), 2, "ステップの一覧も返ります");

  server.stop().await;
}

#[tokio::test]
async fn get_run_detail_does_not_mark_log_silent_when_no_log_time_and_no_start_time() {
  // 最後のログの時刻も開始時刻も無い run で、無音と誤判定しない。開始時刻を最後のログの
  // 時刻の代用にすると、古い開始時刻で無音と判定され誤る。最後のログが分からないことは
  // 「無音」ではない。
  let harness = seeded_harness(run_state_without_any_start_or_log_time());
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let detail = client
    .get_run_detail(GetRunDetailRequest { run_slug: "run-1".to_owned() })
    .await
    .expect("run 詳細を取得できます")
    .into_inner();

  assert!(!detail.log_silent, "最後のログの時刻が分からない run を無音と判定しません");

  server.stop().await;
}

#[tokio::test]
async fn get_run_detail_rejects_a_malformed_started_at() {
  // 欠損の許容は、不正な値の許容に読み替えない。RFC3339 でない `startedAt` は従来どおり
  // 解析エラー（INTERNAL）で詳細取得が失敗する。
  let mut state = run_state();
  state["runs"]["run-1"]["startedAt"] = serde_json::Value::String("not-a-time".to_owned());
  let harness = seeded_harness(state);
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let status = client
    .get_run_detail(GetRunDetailRequest { run_slug: "run-1".to_owned() })
    .await
    .expect_err("不正な開始時刻は解析エラーです");

  assert_eq!(status.code(), tonic::Code::Internal, "不正な開始時刻は内部エラーです: {status:?}");

  server.stop().await;
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
async fn get_run_detail_keeps_server_connected_while_the_project_is_delayed() {
  let harness = seeded_harness(run_state());
  harness.store.record_failure("takt-dash", time("2026-11-13T12:00:30Z"));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let detail = client
    .get_run_detail(GetRunDetailRequest { run_slug: "run-1".to_owned() })
    .await
    .expect("遅延中も run 詳細を組み立てられます")
    .into_inner();

  assert!(detail.server_connected, "遅延中は接続中として扱います");
  assert_eq!(detail.server_last_read_at, Some(timestamp("2026-11-13T12:00:00Z")), "最後に読めた時刻を返します");

  server.stop().await;
}

#[tokio::test]
async fn get_run_detail_reports_server_disconnected_while_keeping_last_read_at() {
  let harness = seeded_harness(run_state());
  let failed_at = time("2026-11-13T12:02:00Z");
  for _ in 0..3 {
    harness.store.record_failure("takt-dash", failed_at);
  }
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

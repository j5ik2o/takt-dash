//! gRPC の GetRunDetail が、状態ストアと run の保存場所（`meta.json`、本体ログ、`reports/`、
//! プロジェクト側の `interventions.jsonl`）から GetRunDetailResponse を組み立て、ローカルで読めた
//! run では追加の MCP
//! 読取を進行図の定義に限定することを検証する。本体ログは末尾の一定量だけを読み、
//! 同じサーバーへの2回目の要求は続きだけを読む。worktree の run、再開した run（source_run_slug）、
//! 掃除済みの worktree の run も扱う。
//!
//! 状態ストアにない runSlug は Not Found 相当のステータスで返す。
//!
//! ローカルで保存場所を読めない run は takt_get_run の値で返し、それでも読めなければ従来の
//! ステータスで返す。

mod common;
mod grpc_harness;

use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};
use common::*;
use grpc_harness::{client, start_server, GrpcHarness};
use serde_json::{json, Value};
use takt_dash_backend::{
  domain::{GoalId, RunSlug, TaskName},
  model::{ExecutionStatus, GoalStatus, ProjectSnapshot, TaskEntry, TaskStatus},
  poller::Poller,
  takt_dash::v1::{GetRunDetailRequest, GetRunDetailResponse, RunStatus},
};

const WORKFLOW: &str = "development-core";
/// 受入条件の「本体ログ 8MB 以上」を満たす fixture の大きさ。
const LARGE_BODY_LOG_BYTES: u64 = 9 * 1024 * 1024;

fn project_dir() -> &'static Path {
  Path::new("/abs/takt-dash")
}

/// takt-mcp が返す状態。GetRunDetail はこの状態を読まない。定期読み取り（poller）を回すテストが
/// 状態ストアへタスクを入れるためだけに使う。
fn mcp_state() -> Value {
  json!({
    "tasks": [
      { "name": "implement", "goalId": "g1", "status": "running", "startedAt": "2026-11-13T08:30:00Z", "runSlug": "run-1", "currentStep": "implement" }
    ]
  })
}

/// 状態ストアに run-1（session_id・summary・ゴールを持つ）を持つ 1 プロジェクト分の土台を作る。
fn seeded_harness(state: Value) -> GrpcHarness {
  let harness =
    GrpcHarness::builder().single_project(project("takt-dash", project_dir()), server_meta(), state).build();
  let mut stored_run = run("run-1", "implement", time("2026-11-13T09:06:00Z"));
  stored_run.session_id = "sess-1".to_owned();
  stored_run.summary = "実装の run".to_owned();
  stored_run.goal_id = Some(GoalId::try_from("g1").expect("テストのゴール ID は有効です"));
  let snapshot = ProjectSnapshot {
    goals: vec![goal("g1", GoalStatus::Created, ExecutionStatus::Active)],
    tasks: vec![],
    inquiries: vec![],
    runs: vec![stored_run],
    notifications: vec![],
    proposals: vec![],
    ..ProjectSnapshot::empty()
  };
  harness.store.record_success("takt-dash", snapshot, time("2026-11-13T12:00:00Z"));
  harness
}

/// 土台のプロジェクトの cwd に、対応表に run-1 だけを載せた run の保存場所を作る。
fn run_files(harness: &GrpcHarness) -> RunFixture {
  let fixture = RunFixture::new(harness.project_dir());
  fixture.write_tasks(&[RunTaskLine { run_slug: Some("run-1"), source_run_slug: None }]);
  fixture
}

/// 実行中の run-1 を、plan と implement の step_complete、現在ステップ implement の step_start
/// で作る。本体ログの更新時刻は step より古い 09:00 に固定する。
fn running_run_files(harness: &GrpcHarness) -> RunFixture {
  let fixture = run_files(harness);
  fixture.write_meta("run-1", &running_meta("implement", 3, WORKFLOW, "2026-11-13T09:00:00Z"));
  fixture.append_records("run-1", &[
    workflow_start_record(WORKFLOW),
    step_complete_record("plan", "architect", 1, "2026-11-13T09:01:00Z"),
    step_start_record("implement", WORKFLOW, 3, "2026-11-13T09:05:00Z", &["implement"]),
  ]);
  fixture.set_body_log_modified("run-1", time("2026-11-13T09:00:00Z"));
  fixture
}

/// プロジェクトの cwd と並ぶ、takt の既定の worktree 置き場（`<cwd>/../takt-worktrees`）の下に
/// worktree のディレクトリを作らずにパスだけを返す。
fn worktree_path(harness: &GrpcHarness, name: &str) -> PathBuf {
  harness.project_dir().parent().expect("プロジェクトの cwd には親があります").join("takt-worktrees").join(name)
}

async fn request_detail(harness: &GrpcHarness, run_slug: &str) -> Result<GetRunDetailResponse, tonic::Status> {
  let server = start_server(harness).await;
  let mut client = client(server.address).await;
  let result = client.get_run_detail(GetRunDetailRequest { run_slug: run_slug.to_owned() }).await;
  server.stop().await;
  result.map(tonic::Response::into_inner)
}

/// run の本文はローカルから読み、追加の MCP 読取は図の定義だけであることを確かめる。
fn assert_only_workflow_definition_reads(harness: &GrpcHarness, processes_before: usize, calls_before: usize) {
  let processes = harness.recorded_processes();
  let calls = harness.recorded_calls();
  let added = &calls[calls_before..];
  assert!(added.iter().all(|call| call.name == "takt_get_workflow"), "run の内容を MCP で再取得しません: {added:?}");
  assert_eq!(processes.len() - processes_before, added.len(), "各詳細要求は定義取得用の接続を終了します");
}

fn timestamp(text: &str) -> prost_types::Timestamp {
  proto_timestamp(time(text))
}

fn proto_timestamp(time: DateTime<Utc>) -> prost_types::Timestamp {
  prost_types::Timestamp { seconds: time.timestamp(), nanos: time.timestamp_subsec_nanos() as i32 }
}

fn step_log_names(detail: &GetRunDetailResponse) -> Vec<String> {
  detail.step_logs.iter().map(|log| log.step_name.clone()).collect()
}

#[tokio::test]
async fn workflow_definition_pages_and_local_execution_evidence_reach_detail_and_overview() {
  use sha2::{Digest, Sha256};

  let mut state = mcp_state();
  let mut nodes = vec![
    json!({ "name": "gate", "kind": "system", "rules": [{}, { "next": "fix" }, { "next": "finish" }] }),
    json!({ "name": "fix", "kind": "agent", "rules": [{ "next": "gate" }] }),
  ];
  nodes.extend((0..51).map(|index| json!({ "name": format!("unused-{index}"), "kind": "agent", "rules": [] })));
  nodes.push(json!({ "name": "finish", "kind": "agent", "rules": [{ "next": "COMPLETE" }, { "next": "ABORT" }] }));
  state["workflows"] = json!({ WORKFLOW: { "initialStep": "gate", "steps": nodes } });
  let harness = seeded_harness(state);
  let fixture = run_files(&harness);
  let mut meta = running_meta("fix", 9, WORKFLOW, "2026-11-13T09:00:00Z");
  meta["stepExecutions"] = json!([
    { "identity": format!("{:x}", Sha256::digest(serde_json::to_vec(&(Vec::<String>::new(), WORKFLOW, "gate")).unwrap())), "workflow": WORKFLOW, "step": "gate", "count": 3 },
    { "identity": format!("{:x}", Sha256::digest(serde_json::to_vec(&(Vec::<String>::new(), WORKFLOW, "fix")).unwrap())), "workflow": WORKFLOW, "step": "fix", "count": 1 }
  ]);
  fixture.write_meta("run-1", &meta);
  let mut gate = step_complete_record("gate", "reviewer", 8, "2026-11-13T09:02:00Z");
  gate["matchedRuleIndex"] = json!(1);
  fixture.append_records("run-1", &[
    workflow_start_record(WORKFLOW),
    gate,
    step_start_record("fix", WORKFLOW, 9, "2026-11-13T09:03:00Z", &["fix"]),
  ]);
  let mut poller = Poller::new(
    harness.config_path().to_path_buf(),
    harness.activity.clone(),
    harness.store.clone(),
    harness.changes.clone(),
    harness.registry.clone(),
  );
  poller.read_cycle_once().await.unwrap();
  poller.shutdown().await;
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;
  let overview =
    client.get_overview(takt_dash_backend::takt_dash::v1::GetOverviewRequest {}).await.unwrap().into_inner();
  let detail = client.get_run_detail(GetRunDetailRequest { run_slug: "run-1".to_owned() }).await.unwrap().into_inner();
  server.stop().await;
  let progress = detail.progress.expect("詳細に進行図があります");
  assert_eq!(progress.initial_step.as_deref(), Some("gate"));
  assert_eq!(progress.definition.len(), 54);
  assert_eq!(progress.definition.last().unwrap().name, "finish");
  assert_eq!(progress.definition[0].kind, "system");
  assert_eq!(progress.definition[0].rules[0].next, None);
  assert_eq!(progress.definition[0].rules[1].next.as_deref(), Some("fix"));
  assert_eq!(progress.backbone, ["gate", "finish"]);
  assert_eq!(progress.current_position.unwrap().value, 1);
  assert_eq!(progress.nodes[0].execution_count.unwrap().value, 3);
  assert_eq!(
    progress.edges.iter().find(|edge| edge.source == "gate" && edge.target == "fix").unwrap().traversals.unwrap().value,
    1
  );
  assert!(progress.edges.iter().all(|edge| edge.target != "ABORT"));
  let run = &overview.runs[0];
  assert_eq!(run.current_step_number, Some(2), "既存位置は定義順のままです");
  assert_eq!(run.total_steps, Some(54));
  assert_eq!(run.current_iteration, Some(9));
  assert_eq!(run.progress.as_ref().unwrap().current_position.unwrap().value, 1);
  assert_eq!(run.progress.as_ref().unwrap().backbone, progress.backbone);
  assert!(harness
    .recorded_calls()
    .iter()
    .any(|call| call.name == "takt_get_workflow" && call.arguments["offset"] == 50));
}

#[tokio::test]
async fn remote_progress_does_not_fabricate_counts_or_traversals_missing_from_the_mcp_contract() {
  let mut state = remote_run_state();
  state["workflows"] = json!({ "remote-workflow": { "initialStep": "remote-plan", "steps": [
    { "name": "remote-plan", "kind": "agent", "rules": [{ "next": "remote-implement" }] },
    { "name": "remote-implement", "kind": "agent", "rules": [{ "next": "COMPLETE" }] }
  ] } });
  let harness = seeded_harness(state);
  let detail = request_detail(&harness, "run-1").await.unwrap();
  assert_detail_from_takt_get_run(&detail, &harness);
  let progress = detail.progress.unwrap();
  assert_eq!(progress.backbone, ["remote-plan", "remote-implement"]);
  assert!(progress.nodes.iter().all(|node| node.execution_count.is_none()));
  assert!(progress.edges.iter().all(|edge| edge.traversals.is_none()));
}

#[tokio::test]
async fn invalid_progress_counts_do_not_discard_readable_run_detail() {
  let mut state = mcp_state();
  state["workflows"] = json!({ WORKFLOW: { "initialStep": "implement", "steps": [{ "name": "implement", "kind": "agent", "rules": [{ "next": "COMPLETE" }] }] } });
  let harness = seeded_harness(state);
  let fixture = running_run_files(&harness);
  let mut meta = running_meta("implement", 3, WORKFLOW, "2026-11-13T09:00:00Z");
  meta["stepExecutions"] =
    json!([{ "identity": "a".repeat(64), "workflow": WORKFLOW, "step": "implement", "count": -1 }]);
  fixture.write_meta("run-1", &meta);
  let detail = request_detail(&harness, "run-1").await.unwrap();
  assert_eq!(detail.current_step.as_deref(), Some("implement"));
  assert_eq!(step_log_names(&detail), ["plan"]);
  assert!(detail.progress.unwrap().nodes[0].execution_count.is_none());
}

#[tokio::test]
async fn get_run_detail_assembles_the_response_from_the_store_and_the_run_files() {
  let harness = seeded_harness(mcp_state());
  let fixture = running_run_files(&harness);
  let mut meta = running_meta("implement", 3, WORKFLOW, "2026-11-13T09:00:00Z");
  meta["task"] = json!("タスクの本文です。");
  fixture.write_meta("run-1", &meta);
  fixture.write_report("run-1", "plan.md", "計画のレポート");
  fixture.write_interventions("run-1", &[
    issued_event(1, "2026-11-13T08:50:00Z", "指示の本文"),
    issued_event(2, "2026-11-13T08:55:00Z", "二つ目の指示"),
    delivered_event(&[1], "same_session", "2026-11-13T08:51:00Z", "implement"),
  ]);

  let detail = request_detail(&harness, "run-1").await.expect("run 詳細を取得できます");

  // 保存場所から読む値。
  assert_eq!(detail.status, RunStatus::Running as i32, "status は meta.json の値です");
  assert_eq!(detail.current_step.as_deref(), Some("implement"));
  assert_eq!(detail.current_iteration, Some(3));
  assert_eq!(detail.phase, Some(1));
  assert_eq!(detail.started_at, Some(timestamp("2026-11-13T09:00:00Z")), "開始時刻は meta.json の startTime です");
  assert_eq!(detail.current_step_started_at, Some(timestamp("2026-11-13T09:05:00Z")));
  assert_eq!(detail.last_log_at, Some(timestamp("2026-11-13T09:05:00Z")));
  assert_eq!(step_log_names(&detail), vec!["plan".to_owned()], "step_logs は本体ログの step_complete です");
  assert_eq!(detail.step_logs[0].persona, "architect");
  assert_eq!(detail.step_logs[0].status, "done");
  assert_eq!(detail.step_logs[0].iteration, 1);
  assert_eq!(detail.step_logs[0].timestamp, Some(timestamp("2026-11-13T09:01:00Z")));
  assert_eq!(detail.step_logs[0].content, "", "ステップの本文は持ちません");
  assert_eq!(detail.workflow, WORKFLOW);
  assert_eq!(detail.task_text, "タスクの本文です。", "タスク本文は meta.json の task です");
  assert!(!detail.logs_removed, "ログが残っている run は logs_removed を立てません");

  // ログ末尾は本体ログの末尾の記録を「時刻 種類 ステップ」の行にしたものです。
  let tail_lines: Vec<&str> = detail.log_tail.lines().collect();
  assert_eq!(
    tail_lines[tail_lines.len().saturating_sub(2)..].to_vec(),
    vec!["2026-11-13T09:01:00Z step_complete plan", "2026-11-13T09:05:00Z step_start implement"],
    "ログ末尾の最後の2行は本体ログの最後の2件です: {:?}",
    detail.log_tail
  );

  // レポートと指示書は一覧だけを返す。本文は GetRunContent で取得する。
  assert_eq!(detail.reports.len(), 1);
  assert_eq!(detail.reports[0].filename, "plan.md");
  assert_eq!(detail.reports[0].size_bytes, "計画のレポート".len() as i64, "大きさはファイルのバイト数です");
  let instructions: Vec<(i32, &str, Option<prost_types::Timestamp>)> = detail
    .instructions
    .iter()
    .map(|instruction| (instruction.instruction_id, instruction.state.as_str(), instruction.issued_at))
    .collect();
  assert_eq!(
    instructions,
    vec![
      (1, "deliveredSameSession", Some(timestamp("2026-11-13T08:50:00Z"))),
      (2, "pending", Some(timestamp("2026-11-13T08:55:00Z"))),
    ],
    "指示書は interventions.jsonl のイベントを takt と同じ規則で畳み込んだ状態です"
  );

  // 状態ストアから補う値。
  assert_eq!(detail.run_slug, "run-1");
  assert_eq!(detail.session_id, "sess-1", "session_id は状態ストアの run の値です");
  assert_eq!(detail.summary, "実装の run", "summary は状態ストアの run の値です");
  assert_eq!(detail.server_id, "mac-mini");
  assert_eq!(detail.project, "takt-dash");
  assert_eq!(detail.goal_id, "g1");
  assert_eq!(detail.task_name, "implement");
  assert!(detail.server_connected, "読めていたプロジェクトは接続ありです");
  assert_eq!(detail.server_last_read_at, Some(timestamp("2026-11-13T12:00:00Z")));
}

#[tokio::test]
async fn get_run_detail_reads_only_the_workflow_definition_from_takt_mcp() {
  let harness = seeded_harness(mcp_state());
  let fixture = running_run_files(&harness);
  fixture.write_report("run-1", "plan.md", "計画のレポート");
  fixture.write_interventions("run-1", &[issued_event(1, "2026-11-13T08:50:00Z", "指示の本文")]);
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  for _ in 0..2 {
    client.get_run_detail(GetRunDetailRequest { run_slug: "run-1".to_owned() }).await.expect("run 詳細を取得できます");
  }
  server.stop().await;

  assert_eq!(harness.recorded_calls().len(), 2, "2回の詳細要求で定義を2回読みます");
  assert_only_workflow_definition_reads(&harness, 0, 0);
}

#[tokio::test]
async fn initial_detail_progress_keeps_completions_outside_the_display_window() {
  let mut state = mcp_state();
  state["workflows"] = json!({WORKFLOW:{"initialStep":"head", "steps":[
    {"name":"head","rules":[{"next":"tail"}]}, {"name":"tail","rules":[{"next":"COMPLETE"}]}
  ]}});
  let harness = seeded_harness(state);
  let fixture = run_files(&harness);
  fixture.write_meta("run-1", &running_meta("tail", 2, WORKFLOW, "2026-11-13T09:00:00Z"));
  let mut head = step_complete_record("head", "architect", 1, "2026-11-13T09:01:00Z");
  head["workflow"] = json!(WORKFLOW);
  head["matchedRuleIndex"] = json!(0);
  fixture.append_records("run-1", &[workflow_start_record(WORKFLOW), head]);
  append_filler_records(&fixture, "run-1", 2 * 1024 * 1024);
  fixture.append_records("run-1", &[step_complete_record("tail", "coder", 2, "2026-11-13T09:30:00Z")]);
  let detail = request_detail(&harness, "run-1").await.unwrap();
  let graph = detail.progress.unwrap();
  assert_eq!(graph.nodes[0].state, takt_dash_backend::takt_dash::v1::WorkflowNodeState::Completed as i32);
  assert_eq!(graph.edges[0].traversals.unwrap().value, 1);
  assert_eq!(graph.nodes[0].log_index, None);
  assert_eq!(detail.step_logs.len(), 1);
  assert_eq!(graph.nodes[1].log_index, Some(0));
  let mut poller = Poller::new(
    harness.config_path().to_path_buf(),
    harness.activity.clone(),
    harness.store.clone(),
    harness.changes.clone(),
    harness.registry.clone(),
  );
  poller.read_cycle_once().await.unwrap();
  poller.shutdown().await;
  let server = start_server(&harness).await;
  let mut api = client(server.address).await;
  let overview = api.get_overview(takt_dash_backend::takt_dash::v1::GetOverviewRequest {}).await.unwrap().into_inner();
  let same_history = overview.runs[0].progress.as_ref().unwrap();
  assert_eq!(same_history.edges, graph.edges);
  assert_eq!(
    same_history.nodes.iter().map(|node| node.state).collect::<Vec<_>>(),
    graph.nodes.iter().map(|node| node.state).collect::<Vec<_>>()
  );
  server.stop().await;
}

#[tokio::test]
async fn malformed_line_keeps_windowed_completions_and_unknown_edges_in_overview_and_detail() {
  use std::io::Write;
  let mut state = mcp_state();
  state["workflows"] = json!({WORKFLOW:{"initialStep":"head","steps":[
    {"name":"head","rules":[{"next":"tail"},{"next":"head"}]},
    {"name":"tail","rules":[{"next":"COMPLETE"}]}
  ]}});
  let harness = seeded_harness(state);
  let fixture = run_files(&harness);
  fixture.write_meta("run-1", &running_meta("tail", 3, WORKFLOW, "2026-11-13T09:00:00Z"));
  let mut completion = step_complete_record("head", "coder", 1, "2026-11-13T09:01:00Z");
  completion["workflow"] = json!(WORKFLOW);
  completion["matchedRuleIndex"] = json!(0);
  fixture.append_records("run-1", &[workflow_start_record(WORKFLOW), completion.clone()]);
  std::fs::OpenOptions::new()
    .append(true)
    .open(fixture.body_log("run-1"))
    .unwrap()
    .write_all(b"invalid JSON\n")
    .unwrap();
  completion["iteration"] = json!(2);
  fixture.append_records("run-1", &[completion]);
  append_filler_records(&fixture, "run-1", 2 * 1024 * 1024);
  fixture.append_records("run-1", &[step_complete_record("tail", "coder", 3, "2026-11-13T09:30:00Z")]);
  let mut poller = Poller::new(
    harness.config_path().to_path_buf(),
    harness.activity.clone(),
    harness.store.clone(),
    harness.changes.clone(),
    harness.registry.clone(),
  );
  poller.read_cycle_once().await.unwrap();
  poller.shutdown().await;
  let server = start_server(&harness).await;
  let mut api = client(server.address).await;
  let detail = api.get_run_detail(GetRunDetailRequest { run_slug: "run-1".into() }).await.unwrap().into_inner();
  let graph = detail.progress.unwrap();
  assert_eq!(graph.edges[0].traversals.unwrap().value, 2);
  assert!(graph.edges[1].traversals.is_none());
  assert_eq!(graph.nodes[0].state, takt_dash_backend::takt_dash::v1::WorkflowNodeState::Completed as i32);
  assert_eq!(graph.nodes[0].log_index, None);
  assert_eq!(detail.step_logs.len(), 1);
  assert_eq!(graph.nodes[1].log_index, Some(0));
  let overview = api.get_overview(takt_dash_backend::takt_dash::v1::GetOverviewRequest {}).await.unwrap().into_inner();
  assert_eq!(overview.runs[0].progress.as_ref().unwrap().edges, graph.edges);
  let repeated = api.get_run_detail(GetRunDetailRequest { run_slug: "run-1".into() }).await.unwrap().into_inner();
  assert_eq!(repeated.progress.unwrap(), graph);
  let cursor = api
    .get_run_updates(takt_dash_backend::takt_dash::v1::GetRunUpdatesRequest {
      run_slug: "run-1".into(),
      ..Default::default()
    })
    .await
    .unwrap()
    .into_inner();
  let partial = json!({"type":"step_complete","step":"head","workflow":WORKFLOW,"iteration":4,"matchedRuleIndex":0,"timestamp":"2026-11-13T09:31:00Z"}).to_string();
  let mut output = std::fs::OpenOptions::new().append(true).open(fixture.body_log("run-1")).unwrap();
  output.write_all(partial.as_bytes()).unwrap();
  let unfinished = api.get_run_detail(GetRunDetailRequest { run_slug: "run-1".into() }).await.unwrap().into_inner();
  assert_eq!(unfinished.progress.unwrap().edges[0].traversals.unwrap().value, 2);
  output.write_all(b"\n").unwrap();
  let updates = api
    .get_run_updates(takt_dash_backend::takt_dash::v1::GetRunUpdatesRequest {
      run_slug:       "run-1".into(),
      epoch:          Some(cursor.epoch),
      after_sequence: cursor.lines.last().map(|line| line.sequence),
    })
    .await
    .unwrap()
    .into_inner();
  assert!(!updates.reset);
  assert_eq!(updates.epoch, cursor.epoch);
  assert_eq!(updates.lines.len(), 1);
  assert!(updates.lines[0].text.ends_with("step_complete head"));
  for _ in 0..2 {
    let appended = api.get_run_detail(GetRunDetailRequest { run_slug: "run-1".into() }).await.unwrap().into_inner();
    let progress = appended.progress.unwrap();
    assert_eq!(progress.edges[0].traversals.unwrap().value, 3);
    assert_eq!(progress.nodes[0].log_index, Some(1));
    assert_eq!(appended.step_logs[1].step_name, "head");
  }
  server.stop().await;
}

#[tokio::test]
async fn get_run_detail_reads_only_the_tail_of_a_large_body_log() {
  // 9 MiB の本体ログの先頭付近に head-marker、末尾に tail-marker の step_complete を置く。
  // 先頭から全体を読む実装は head-marker を step_logs に含めて落ちる。途中から読み始めたことで
  // 集計が始まらない実装（workflow_start を待つ実装）は tail-marker を落として落ちる。
  let harness = seeded_harness(mcp_state());
  let fixture = run_files(&harness);
  fixture.write_meta("run-1", &running_meta("implement", 1, WORKFLOW, "2026-11-13T09:00:00Z"));
  fixture.append_records("run-1", &[
    workflow_start_record(WORKFLOW),
    step_complete_record("head-marker", "architect", 1, "2026-11-13T09:01:00Z"),
  ]);
  append_filler_records(&fixture, "run-1", LARGE_BODY_LOG_BYTES);
  fixture.append_records("run-1", &[step_complete_record("tail-marker", "coder", 1, "2026-11-13T09:30:00Z")]);
  let size = std::fs::metadata(fixture.body_log("run-1")).expect("本体ログの大きさを読めます").len();
  assert!(size >= 8 * 1024 * 1024, "fixture の本体ログは 8MB 以上です: {size}");

  let detail = request_detail(&harness, "run-1").await.expect("大きな本体ログの run 詳細を取得できます");

  assert_eq!(
    step_log_names(&detail),
    vec!["tail-marker".to_owned()],
    "末尾の範囲の step_complete だけを集計し、先頭付近の head-marker は読みません"
  );
  assert!(
    detail.log_tail.lines().any(|line| line == "2026-11-13T09:30:00Z step_complete tail-marker"),
    "ログ末尾に tail-marker の行があります: {:?}",
    detail.log_tail
  );
  assert!(
    !detail.log_tail.lines().any(|line| line.split(' ').any(|part| part == "head-marker")),
    "ログ末尾に先頭付近の head-marker の行はありません: {:?}",
    detail.log_tail
  );
}

#[tokio::test]
async fn running_run_detail_with_a_large_body_log_returns_within_two_seconds() {
  // 受入条件「実行中の run の詳細が2秒以内に返る」のローカル fixture での計測。読み位置を持たない
  // 初回の要求が最も重いため、毎回新しいサーバーで初回の要求を10回測り、最大値を出力する。
  let harness = seeded_harness(mcp_state());
  let fixture = run_files(&harness);
  fixture.write_meta("run-1", &running_meta("implement", 1, WORKFLOW, "2026-11-13T09:00:00Z"));
  fixture.append_records("run-1", &[workflow_start_record(WORKFLOW)]);
  append_filler_records(&fixture, "run-1", LARGE_BODY_LOG_BYTES);
  fixture.append_records("run-1", &[step_complete_record("tail-marker", "coder", 1, "2026-11-13T09:30:00Z")]);

  let mut elapsed = Vec::new();
  for _ in 0..10 {
    let server = start_server(&harness).await;
    let mut client = client(server.address).await;
    let started = std::time::Instant::now();
    client.get_run_detail(GetRunDetailRequest { run_slug: "run-1".to_owned() }).await.expect("run 詳細を取得できます");
    elapsed.push(started.elapsed());
    server.stop().await;
  }
  let max = elapsed.iter().max().copied().expect("10回測りました");
  eprintln!("GetRunDetail（本体ログ {LARGE_BODY_LOG_BYTES} バイト、10回）の最大応答時間: {max:?}、各回: {elapsed:?}");

  assert!(max < std::time::Duration::from_secs(2), "10回の最大応答時間は2秒未満です: {max:?}");
}

#[tokio::test]
async fn get_run_detail_reads_a_run_in_its_worktree() {
  // 対応表の worktree_path にある worktree 側の .takt/runs/run-1 から meta とステップを読む。
  // プロジェクト側にも同名の run を置き、プロジェクト側を読む実装は "project-step" を返して落ちる。
  let harness = seeded_harness(mcp_state());
  let worktree = worktree_path(&harness, "wt-1");
  let project_fixture = RunFixture::new(harness.project_dir());
  project_fixture.write_worktree_tasks(&[WorktreeTaskLine {
    run_slug:        Some("run-1"),
    source_run_slug: None,
    worktree_path:   &worktree,
  }]);
  project_fixture.write_meta("run-1", &running_meta("project-step", 1, WORKFLOW, "2026-11-13T08:00:00Z"));
  project_fixture.append_records("run-1", &[
    workflow_start_record(WORKFLOW),
    step_complete_record("project-step", "coder", 1, "2026-11-13T08:01:00Z"),
  ]);
  let worktree_fixture = RunFixture::new(&worktree);
  worktree_fixture.write_meta("run-1", &running_meta("review", 2, WORKFLOW, "2026-11-13T09:00:00Z"));
  worktree_fixture.append_records("run-1", &[
    workflow_start_record(WORKFLOW),
    step_complete_record("worktree-step", "coder", 1, "2026-11-13T09:01:00Z"),
  ]);

  let detail = request_detail(&harness, "run-1").await.expect("worktree の run 詳細を取得できます");

  assert!(!detail.logs_removed);
  assert_eq!(detail.current_step.as_deref(), Some("review"), "状態は worktree 側の meta.json の値です");
  assert_eq!(step_log_names(&detail), vec!["worktree-step".to_owned()], "ステップは worktree 側の本体ログです");
  assert_only_workflow_definition_reads(&harness, 0, 0);
}

#[tokio::test]
async fn get_run_detail_reads_a_resumed_run_from_its_source_run_slug() {
  // 再開した run は、対応表で source_run_slug が run-source の項目と worktree_path から、worktree
  // 側の run-source のログを読む。ゴールとタスク名は状態ストアの、source_run_slug
  // が一致するタスクから取る。
  let harness =
    GrpcHarness::builder().single_project(project("takt-dash", project_dir()), server_meta(), json!({})).build();
  let snapshot = ProjectSnapshot {
    goals: vec![],
    tasks: vec![TaskEntry {
      name:            TaskName::try_from("resume-task").expect("テストのタスク名は有効です"),
      goal_id:         Some(GoalId::try_from("g-resume").expect("テストのゴール ID は有効です")),
      status:          TaskStatus::Running,
      started_at:      Some(time("2026-11-13T09:00:00Z")),
      run_slug:        None,
      source_run_slug: Some("run-source".to_owned()),
      current_step:    Some("review".to_owned()),
      merge_task:      false,
    }],
    inquiries: vec![],
    runs: vec![],
    notifications: vec![],
    proposals: vec![],
    ..ProjectSnapshot::empty()
  };
  harness.store.record_success("takt-dash", snapshot, time("2026-11-13T12:00:00Z"));
  let worktree = worktree_path(&harness, "wt-resume");
  RunFixture::new(harness.project_dir()).write_worktree_tasks(&[WorktreeTaskLine {
    run_slug:        None,
    source_run_slug: Some("run-source"),
    worktree_path:   &worktree,
  }]);
  let worktree_fixture = RunFixture::new(&worktree);
  worktree_fixture.write_meta("run-source", &running_meta("review", 1, WORKFLOW, "2026-11-13T09:00:00Z"));
  worktree_fixture.append_records("run-source", &[
    workflow_start_record(WORKFLOW),
    step_complete_record("resumed-step", "reviewer", 1, "2026-11-13T09:10:00Z"),
  ]);

  let detail = request_detail(&harness, "run-source").await.expect("再開した run の詳細を取得できます");

  assert_eq!(detail.run_slug, "run-source");
  assert!(!detail.logs_removed);
  assert_eq!(step_log_names(&detail), vec!["resumed-step".to_owned()], "worktree 側の run-source の本体ログです");
  assert_eq!(detail.goal_id, "g-resume", "ゴールは source_run_slug が一致するタスクから取ります");
  assert_eq!(detail.task_name, "resume-task", "タスク名は source_run_slug が一致するタスクから取ります");
  assert_only_workflow_definition_reads(&harness, 0, 0);
}

#[tokio::test]
async fn get_run_detail_reports_logs_removed_for_a_cleaned_worktree() {
  // 対応表に worktree_path があり、そのディレクトリもプロジェクト側の run のデータも無い run は、
  // エラーではなく logs_removed を立てた詳細を返す。ゴールとタスク名と状態は状態ストアの値です。
  let harness = seeded_harness(mcp_state());
  let worktree = worktree_path(&harness, "wt-cleaned");
  RunFixture::new(harness.project_dir()).write_worktree_tasks(&[WorktreeTaskLine {
    run_slug:        Some("run-1"),
    source_run_slug: None,
    worktree_path:   &worktree,
  }]);
  assert!(!worktree.exists(), "掃除済みの worktree は残っていません");

  let detail = request_detail(&harness, "run-1").await.expect("掃除済みの worktree の run も詳細を返します");

  assert!(detail.logs_removed, "ログが残っていないことを logs_removed で返します");
  assert!(detail.step_logs.is_empty(), "ステップの記録は空です");
  assert!(detail.reports.is_empty(), "レポートの一覧は空です");
  assert_eq!(detail.log_tail, "", "ログ末尾は空です");
  assert_eq!(detail.status, RunStatus::Running as i32, "状態は状態ストアの run の値です");
  assert_eq!(detail.goal_id, "g1");
  assert_eq!(detail.task_name, "implement");
  assert_eq!(detail.session_id, "sess-1");
  assert_only_workflow_definition_reads(&harness, 0, 0);
}

#[tokio::test]
async fn second_run_detail_on_the_same_server_adds_only_the_appended_step_complete() {
  // 同じサーバーへの1回目の要求の後に step_complete を追記し、2回目の要求で追記分が加わる。
  let harness = seeded_harness(mcp_state());
  let fixture = running_run_files(&harness);
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let first = client
    .get_run_detail(GetRunDetailRequest { run_slug: "run-1".to_owned() })
    .await
    .expect("1回目の run 詳細を取得できます")
    .into_inner();
  assert_eq!(step_log_names(&first), vec!["plan".to_owned()]);

  fixture.append_records("run-1", &[step_complete_record("implement", "coder", 3, "2026-11-13T09:20:00Z")]);
  let second = client
    .get_run_detail(GetRunDetailRequest { run_slug: "run-1".to_owned() })
    .await
    .expect("2回目の run 詳細を取得できます")
    .into_inner();
  server.stop().await;

  assert_eq!(
    step_log_names(&second),
    vec!["plan".to_owned(), "implement".to_owned()],
    "追記した step_complete だけが加わり、既読の step を重複させません"
  );
}

#[tokio::test]
async fn second_run_detail_on_the_same_server_does_not_read_the_already_read_part_again() {
  // 1回目の要求で読んだ範囲を、inode と大きさを変えずに同じバイト長の別の文字列へ書き換える。
  // 要求のたびに先頭から読み直す実装は2回目に "PLAN" を返して落ちる。
  let harness = seeded_harness(mcp_state());
  let fixture = running_run_files(&harness);
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  client
    .get_run_detail(GetRunDetailRequest { run_slug: "run-1".to_owned() })
    .await
    .expect("1回目の run 詳細を取得できます");
  fixture.overwrite_body_log_in_place("run-1", "\"plan\"", "\"PLAN\"");
  let second = client
    .get_run_detail(GetRunDetailRequest { run_slug: "run-1".to_owned() })
    .await
    .expect("2回目の run 詳細を取得できます")
    .into_inner();
  server.stop().await;

  assert_eq!(step_log_names(&second), vec!["plan".to_owned()], "既読範囲は読み直しません");
}

#[tokio::test]
async fn run_detail_on_the_same_server_reads_the_new_cwd_after_the_project_cwd_changes() {
  // 同じサーバーで、旧 cwd の run を読んだ後に設定のプロジェクトの cwd を書き換える。2回目の要求は
  // 新しい cwd の保存場所から読む。旧 cwd の集計を使い続ける実装は2回目も "plan" を返して落ちる。
  let harness = seeded_harness(mcp_state());
  running_run_files(&harness);
  let new_root = tempfile::tempdir().expect("新しい cwd の一時ディレクトリを作れます");
  let new_cwd = std::fs::canonicalize(new_root.path()).expect("実パスを解決できます").join("project");
  std::fs::create_dir_all(&new_cwd).expect("新しい cwd を作れます");
  let new_fixture = RunFixture::new(&new_cwd);
  new_fixture.write_tasks(&[RunTaskLine { run_slug: Some("run-1"), source_run_slug: None }]);
  new_fixture.write_meta("run-1", &running_meta("review", 1, WORKFLOW, "2026-11-13T10:00:00Z"));
  new_fixture.append_records("run-1", &[
    workflow_start_record(WORKFLOW),
    step_complete_record("review", "reviewer", 1, "2026-11-13T10:05:00Z"),
  ]);
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let first = client
    .get_run_detail(GetRunDetailRequest { run_slug: "run-1".to_owned() })
    .await
    .expect("旧 cwd の run 詳細を取得できます")
    .into_inner();
  assert_eq!(step_log_names(&first), vec!["plan".to_owned()]);

  let new_cwd_text = new_cwd.display().to_string();
  harness.rewrite_projects(|projects, _| {
    projects[0]["cwd"] = json!(new_cwd_text);
  });
  let second = client
    .get_run_detail(GetRunDetailRequest { run_slug: "run-1".to_owned() })
    .await
    .expect("新しい cwd の run 詳細を取得できます")
    .into_inner();
  server.stop().await;

  assert_eq!(step_log_names(&second), vec!["review".to_owned()], "書き換え後の cwd の本体ログを読みます");
  assert_eq!(second.current_step.as_deref(), Some("review"), "状態も新しい cwd の meta.json から読みます");
}

#[tokio::test]
async fn get_run_detail_keeps_server_connected_while_the_project_is_delayed() {
  let harness = seeded_harness(mcp_state());
  running_run_files(&harness);
  harness.store.record_failure("takt-dash", time("2026-11-13T12:00:30Z"));

  let detail = request_detail(&harness, "run-1").await.expect("遅延中も run 詳細を組み立てられます");

  assert!(detail.server_connected, "遅延中は接続中として扱います");
  assert_eq!(detail.server_last_read_at, Some(timestamp("2026-11-13T12:00:00Z")), "最後に読めた時刻を返します");
}

#[tokio::test]
async fn get_run_detail_reports_server_disconnected_while_keeping_last_read_at() {
  let harness = seeded_harness(mcp_state());
  running_run_files(&harness);
  let failed_at = time("2026-11-13T12:02:00Z");
  for _ in 0..3 {
    harness.store.record_failure("takt-dash", failed_at);
  }

  let detail = request_detail(&harness, "run-1").await.expect("応答がない間も run 詳細を組み立てられます");

  assert!(!detail.server_connected, "応答がなければ server_connected は偽です");
  assert_eq!(detail.server_last_read_at, Some(timestamp("2026-11-13T12:00:00Z")), "最後に読めた時刻を返します");
}

#[tokio::test]
async fn get_run_detail_returns_not_found_for_an_unknown_run_slug() {
  let harness = seeded_harness(mcp_state());
  running_run_files(&harness);

  let status = request_detail(&harness, "does-not-exist").await.expect_err("状態ストアにない runSlug はエラーです");

  assert_eq!(status.code(), tonic::Code::NotFound, "見つからない runSlug は Not Found です: {status:?}");
}

#[tokio::test]
async fn get_run_detail_returns_not_found_for_a_run_missing_from_the_task_table() {
  // 状態ストアにはあるが、tasks.yaml の対応表に載っておらず、takt_get_run でも読めない run は
  // Not Found です。
  let mut state = mcp_state();
  state["isError"] = json!(["takt_get_run"]);
  let harness = seeded_harness(state);
  let fixture = RunFixture::new(harness.project_dir());
  fixture.write_tasks(&[RunTaskLine { run_slug: Some("other-run"), source_run_slug: None }]);

  let status = request_detail(&harness, "run-1").await.expect_err("対応表に無い run は詳細を返しません");

  assert_eq!(status.code(), tonic::Code::NotFound, "対応表に無い run は Not Found です: {status:?}");
}

/// takt_get_run が run-1 をリモートのサーバーの値で返す mock の状態。ローカルの保存場所の値と
/// 区別できるよう、workflow・現在ステップ・ステップの記録を固有の値にする。
fn remote_run_state() -> Value {
  let mut state = mcp_state();
  state["runs"] = json!({
    "run-1": {
      "runSlug": "run-1",
      "status": "running",
      "taskText": "リモートのタスク本文",
      "workflow": "remote-workflow",
      "currentStep": "remote-implement",
      "currentIteration": 4,
      "phase": 2,
      "startedAt": "2026-11-13T09:00:00Z",
      "currentStepStartedAt": "2026-11-13T09:20:00Z",
      "currentStepExecutionCount": 2,
      "stepLogs": [
        { "step": "remote-plan", "persona": "architect", "status": "done", "timestamp": "2026-11-13T09:10:00Z", "iteration": 1 }
      ],
      "reports": [ { "filename": "remote-plan.md", "sizeBytes": 12 } ],
      "instructions": [
        { "instructionId": 1, "issuedAt": "2026-11-13T09:15:00Z", "state": "deliveredNextStep" },
        { "instructionId": 2, "issuedAt": "2026-11-13T09:25:00Z", "state": "pending" }
      ]
    }
  });
  state
}

#[tokio::test]
async fn local_detail_fills_only_an_empty_summary_and_keeps_the_full_task() {
  for saved_summary in [None, Some("実装の run")] {
    let harness = seeded_harness(mcp_state());
    if saved_summary.is_none() {
      let mut task = task("implement", TaskStatus::Completed);
      task.run_slug = Some(RunSlug::try_from("run-1").expect("有効なslugです"));
      harness.store.record_success(
        "takt-dash",
        ProjectSnapshot { tasks: vec![task], ..ProjectSnapshot::empty() },
        time("2026-11-13T12:00:00Z"),
      );
    }
    let fixture = running_run_files(&harness);
    let mut meta = running_meta("implement", 3, WORKFLOW, "2026-11-13T09:00:00Z");
    meta["task"] = json!("確認する。\n末尾");
    fixture.write_meta("run-1", &meta);

    let detail = request_detail(&harness, "run-1").await.expect("ローカル詳細を取得できます");

    assert_eq!(detail.summary, saved_summary.unwrap_or("確認する。…"));
    assert_eq!(detail.task_text, "確認する。\n末尾");
  }
}

#[tokio::test]
async fn remote_detail_fills_only_an_empty_summary_and_keeps_all_task_pages() {
  let full = format!("確認する。\n{}末尾", "あ".repeat(3000));
  for saved_summary in [None, Some("実装の run")] {
    let mut state = remote_run_state();
    state["runs"]["run-1"]["taskText"] = json!(full);
    state["runs"]["run-1"]["taskOmission"] = json!({ "truncated": true, "totalBytes": full.len(), "returnedBytes": 0 });
    let harness = seeded_harness(state);
    if saved_summary.is_none() {
      let mut task = task("implement", TaskStatus::Completed);
      task.run_slug = Some(RunSlug::try_from("run-1").expect("有効なslugです"));
      harness.store.record_success(
        "takt-dash",
        ProjectSnapshot { tasks: vec![task], ..ProjectSnapshot::empty() },
        time("2026-11-13T12:00:00Z"),
      );
    }

    let detail = request_detail(&harness, "run-1").await.expect("リモート詳細を取得できます");

    assert_eq!(detail.summary, saved_summary.unwrap_or("確認する。…"));
    assert_eq!(detail.task_text, full);
    let offsets: Vec<_> = harness
      .recorded_calls()
      .into_iter()
      .filter(|call| call.name == "takt_get_run" && call.arguments["view"] == "task")
      .map(|call| call.arguments["offset"].as_u64().expect("本文のoffsetがあります"))
      .collect();
    assert!(offsets.len() > 1);
    assert_eq!(offsets[0], 0);
    assert!(offsets.windows(2).all(|pair| pair[0] < pair[1]));
  }
}

/// takt_get_run の値で組み立てた詳細であることを確かめる。
fn assert_detail_from_takt_get_run(detail: &GetRunDetailResponse, harness: &GrpcHarness) {
  assert_eq!(detail.status, RunStatus::Running as i32);
  assert_eq!(detail.workflow, "remote-workflow", "workflow は takt_get_run の値です");
  assert_eq!(detail.current_step.as_deref(), Some("remote-implement"));
  assert_eq!(detail.current_iteration, Some(4));
  assert_eq!(detail.phase, Some(2));
  assert_eq!(detail.started_at, Some(timestamp("2026-11-13T09:00:00Z")));
  assert_eq!(detail.current_step_started_at, Some(timestamp("2026-11-13T09:20:00Z")));
  assert_eq!(detail.current_step_execution_count, Some(2));
  assert_eq!(step_log_names(detail), vec!["remote-plan".to_owned()], "step_logs は takt_get_run の stepLogs です");
  let reports: Vec<(&str, i64)> =
    detail.reports.iter().map(|report| (report.filename.as_str(), report.size_bytes)).collect();
  assert_eq!(reports, vec![("remote-plan.md", 12)]);
  assert_eq!(detail.summary, "実装の run", "summary は従来どおり状態ストアの run の値です");
  assert_eq!(detail.task_text, "リモートのタスク本文", "task_text は takt_get_run の view task の値です");
  assert_eq!(
    detail.log_tail, "2026-11-13T09:10:00Z step_complete remote-plan",
    "log_tail は takt_get_run の stepLogs から組み立てた末尾です"
  );
  let instructions: Vec<(i32, &str)> =
    detail.instructions.iter().map(|instruction| (instruction.instruction_id, instruction.state.as_str())).collect();
  assert_eq!(
    instructions,
    vec![(1, "deliveredNextStep"), (2, "pending")],
    "instructions は takt_get_run の view instructions の値です"
  );
  assert_eq!(detail.instructions[0].issued_at, Some(timestamp("2026-11-13T09:15:00Z")));
  assert_eq!(detail.instructions[1].issued_at, Some(timestamp("2026-11-13T09:25:00Z")));
  assert!(!detail.logs_removed);
  let calls = harness.recorded_calls();
  assert!(
    calls.iter().any(|call| call.name == "takt_get_run" && call.arguments["runSlug"] == "run-1"),
    "ローカルで読めない run は takt_get_run で読みます: {calls:?}"
  );
  assert!(
    calls.iter().any(|call| call.name == "takt_get_run" && call.arguments["view"] == "instructions"),
    "指示書の一覧は view instructions で読みます: {calls:?}"
  );
  assert!(
    calls.iter().any(|call| call.name == "takt_get_run" && call.arguments["view"] == "task"),
    "タスク本文は view task で読みます: {calls:?}"
  );
}

#[tokio::test]
async fn get_run_detail_reads_a_run_without_a_local_task_table_from_takt_get_run() {
  // リモートのサーバーのプロジェクトは、ローカルに tasks.yaml が無く保存場所を読めない。その run の
  // 詳細は takt_get_run の値で返す。ローカルの読み取りだけに頼る実装は Unavailable で落ちる。
  let harness = seeded_harness(remote_run_state());

  let detail = request_detail(&harness, "run-1").await.expect("ローカルで読めない run の詳細も取得できます");

  assert_detail_from_takt_get_run(&detail, &harness);
}

#[tokio::test]
async fn get_run_detail_reads_a_run_missing_from_the_task_table_from_takt_get_run() {
  // ローカルの tasks.yaml に載っていない run の詳細も、takt_get_run の値で返す。
  let harness = seeded_harness(remote_run_state());
  let fixture = RunFixture::new(harness.project_dir());
  fixture.write_tasks(&[RunTaskLine { run_slug: Some("other-run"), source_run_slug: None }]);

  let detail = request_detail(&harness, "run-1").await.expect("対応表に無い run の詳細も取得できます");

  assert_detail_from_takt_get_run(&detail, &harness);
}

#[tokio::test]
async fn get_run_detail_keeps_unavailable_when_neither_the_task_table_nor_takt_get_run_can_be_read() {
  // ローカルの tasks.yaml を読めず、takt_get_run も失敗する run は、従来どおり Unavailable です。
  let mut state = remote_run_state();
  state["isError"] = json!(["takt_get_run"]);
  let harness = seeded_harness(state);

  let status = request_detail(&harness, "run-1").await.expect_err("どちらでも読めない run は詳細を返しません");

  assert_eq!(status.code(), tonic::Code::Unavailable, "読めない run は Unavailable です: {status:?}");
}

#[tokio::test]
async fn get_run_detail_succeeds_with_a_partial_last_instruction_line() {
  // takt が interventions.jsonl へ追記している途中の、改行で終わらない最後の行があっても、run
  // 詳細は 読めて、完成した行の指示書を返す。
  let harness = seeded_harness(mcp_state());
  running_run_files(&harness);
  let interventions = harness.project_dir().join(".takt").join("runs").join("run-1").join("interventions.jsonl");
  std::fs::write(
    &interventions,
    format!("{}\n{}", issued_event(1, "2026-11-13T08:50:00Z", "指示の本文"), r#"{"type":"delivered","instr"#),
  )
  .expect("interventions.jsonl を書けます");

  let detail = request_detail(&harness, "run-1").await.expect("書きかけの行があっても run 詳細を取得できます");

  let instructions: Vec<(i32, &str)> =
    detail.instructions.iter().map(|instruction| (instruction.instruction_id, instruction.state.as_str())).collect();
  assert_eq!(instructions, vec![(1, "pending")]);
}

#[tokio::test]
async fn get_run_detail_rejects_an_empty_run_slug_as_invalid_argument() {
  let harness = seeded_harness(mcp_state());

  let status = request_detail(&harness, "").await.expect_err("空の runSlug は拒否します");

  assert_eq!(status.code(), tonic::Code::InvalidArgument, "空の runSlug は Invalid Argument です: {status:?}");
}

#[tokio::test]
async fn get_run_detail_rejects_a_run_slug_that_is_not_a_single_directory_name() {
  let harness = seeded_harness(mcp_state());
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  for run_slug in ["runs/1", "runs\\1", "..", "."] {
    let status = client
      .get_run_detail(GetRunDetailRequest { run_slug: run_slug.to_owned() })
      .await
      .expect_err("run ディレクトリ名にならない runSlug は拒否します");

    assert_eq!(
      status.code(),
      tonic::Code::InvalidArgument,
      "runSlug={run_slug:?} は入力の誤りとして拒否します: {status:?}"
    );
  }

  server.stop().await;
}

#[tokio::test]
async fn get_run_detail_rejects_a_run_meta_without_a_status() {
  let harness = seeded_harness(mcp_state());
  let fixture = run_files(&harness);
  let mut meta = running_meta("implement", 3, WORKFLOW, "2026-11-13T09:00:00Z");
  meta.as_object_mut().expect("meta はオブジェクトです").remove("status");
  fixture.write_meta("run-1", &meta);
  fixture.append_records("run-1", &[workflow_start_record(WORKFLOW)]);

  let status = request_detail(&harness, "run-1").await.expect_err("run の状態が欠落した meta.json は失敗します");

  assert_eq!(
    status.code(),
    tonic::Code::Internal,
    "欠落した run の状態は Running に丸めず、内部エラーにします: {status:?}"
  );
}

#[tokio::test]
async fn get_run_detail_rejects_a_malformed_start_time() {
  // 欠損の許容は、不正な値の許容に読み替えない。RFC3339 でない startTime は内部エラーです。
  let harness = seeded_harness(mcp_state());
  let fixture = run_files(&harness);
  fixture.write_meta("run-1", &running_meta("implement", 3, WORKFLOW, "not-a-time"));
  fixture.append_records("run-1", &[workflow_start_record(WORKFLOW)]);

  let status = request_detail(&harness, "run-1").await.expect_err("不正な開始時刻は失敗します");

  assert_eq!(status.code(), tonic::Code::Internal, "不正な開始時刻は内部エラーです: {status:?}");
}

#[tokio::test]
async fn a_run_without_any_start_time_is_listed_and_reaches_run_detail() {
  // 開始時刻（meta の startTime とタスクの startedAt）も本体ログも無い実行中の run でも、一覧の
  // 読み取りが失敗せず run が状態ストアに入り、詳細取得に到達する。最後のログが分からない run を
  // 無音と判定しない。
  let mut state = mcp_state();
  state["tasks"][0].as_object_mut().expect("task はオブジェクトです").remove("startedAt");
  let harness =
    GrpcHarness::builder().single_project(project("takt-dash", project_dir()), server_meta(), state).build();
  let fixture = run_files(&harness);
  let mut meta = running_meta("implement", 1, WORKFLOW, "2026-11-13T09:00:00Z");
  meta.as_object_mut().expect("meta はオブジェクトです").remove("startTime");
  fixture.write_meta("run-1", &meta);
  let mut poller = Poller::new(
    harness.config_path().to_path_buf(),
    harness.activity.clone(),
    harness.store.clone(),
    harness.changes.clone(),
    harness.registry.clone(),
  );
  poller.read_cycle_once().await.expect("1サイクルを実行できます");
  poller.shutdown().await;
  let listed = harness
    .store
    .slot("takt-dash")
    .and_then(|slot| slot.snapshot)
    .and_then(|snapshot| snapshot.runs.into_iter().find(|run| run.run_slug == "run-1"))
    .expect("開始時刻が無くても run は一覧に含まれます");
  assert_eq!(listed.started_at, None, "開始時刻は未設定として保持します");

  let detail = request_detail(&harness, "run-1").await.expect("一覧に載った run は詳細取得に到達できます");

  assert_eq!(detail.started_at, None, "詳細の開始時刻も未設定で返ります");
  assert_eq!(detail.last_log_at, None, "最後のログの時刻も未設定です");
  assert!(!detail.log_silent, "最後のログの時刻が分からない run を無音と判定しません");
  assert_eq!(detail.task_name, "implement", "開始時刻が無くても他の項目は返ります");
}

#[tokio::test]
async fn get_run_detail_marks_log_silent_false_when_the_last_log_is_within_the_threshold() {
  // 最後のログが 1 分前（既定 10 分の閾値の内側）なら log_silent
  // は偽。下の外側ケースと対で観測する。
  let last_log_at = Utc::now() - Duration::minutes(1);
  let harness = seeded_harness(mcp_state());
  let fixture = run_files(&harness);
  fixture.write_meta("run-1", &running_meta("implement", 1, WORKFLOW, "2026-11-13T09:00:00Z"));
  fixture.append_records("run-1", &[
    workflow_start_record(WORKFLOW),
    step_complete_record("plan", "architect", 1, &last_log_at.to_rfc3339()),
  ]);
  fixture.set_body_log_modified("run-1", last_log_at);

  let detail = request_detail(&harness, "run-1").await.expect("run 詳細を取得できます");

  assert!(!detail.log_silent, "閾値内側の最後のログは無音ではありません");
}

#[tokio::test]
async fn get_run_detail_marks_log_silent_true_when_the_last_log_exceeds_the_threshold() {
  // 最後のログ（step と本体ログの更新時刻）が 30 分前なら log_silent は真。
  let last_log_at = Utc::now() - Duration::minutes(30);
  let harness = seeded_harness(mcp_state());
  let fixture = run_files(&harness);
  fixture.write_meta("run-1", &running_meta("implement", 1, WORKFLOW, "2026-11-13T09:00:00Z"));
  fixture.append_records("run-1", &[
    workflow_start_record(WORKFLOW),
    step_complete_record("plan", "architect", 1, &last_log_at.to_rfc3339()),
  ]);
  fixture.set_body_log_modified("run-1", last_log_at);

  let detail = request_detail(&harness, "run-1").await.expect("run 詳細を取得できます");

  assert!(detail.log_silent, "閾値外側の最後のログは無音です");
}

#[tokio::test]
async fn get_run_detail_last_log_at_follows_a_log_file_update_newer_than_any_step() {
  // 本体ログの更新時刻が step_complete と現在ステップの開始より新しければ、その時刻を採る。
  let harness = seeded_harness(mcp_state());
  let fixture = running_run_files(&harness);
  fixture.set_body_log_modified("run-1", time("2026-11-13T09:30:00Z"));

  let detail = request_detail(&harness, "run-1").await.expect("run 詳細を取得できます");

  assert_eq!(detail.last_log_at, Some(timestamp("2026-11-13T09:30:00Z")));
}

#[tokio::test]
async fn get_run_detail_last_log_at_ignores_a_step_complete_without_timestamp() {
  // timestamp を持たない step_complete
  // は最新時刻の候補にしない。欠落時刻（UNIX_EPOCH）を採る実装や、
  // 末尾の step をそのまま最新とみなす実装は落ちる。
  let harness = seeded_harness(mcp_state());
  let fixture = run_files(&harness);
  fixture.write_meta("run-1", &running_meta("implement", 1, WORKFLOW, "2026-11-13T09:00:00Z"));
  let mut without_timestamp = step_complete_record("implement", "coder", 1, "2026-11-13T09:00:00Z");
  without_timestamp.as_object_mut().expect("record はオブジェクトです").remove("timestamp");
  fixture.append_records("run-1", &[
    workflow_start_record(WORKFLOW),
    step_complete_record("plan", "architect", 1, "2026-11-13T09:40:00Z"),
    without_timestamp,
  ]);
  fixture.set_body_log_modified("run-1", time("2026-11-13T09:10:00Z"));

  let detail = request_detail(&harness, "run-1").await.expect("run 詳細を取得できます");

  assert_eq!(
    detail.last_log_at,
    Some(timestamp("2026-11-13T09:40:00Z")),
    "timestamp の無い step_complete は最新時刻の候補にしません"
  );
  assert_eq!(step_log_names(&detail), vec!["plan".to_owned(), "implement".to_owned()]);
}

#[tokio::test]
async fn run_list_and_run_detail_resolve_the_same_last_log_at() {
  // 一覧（poller）と詳細（GetRunDetail）が同じ run の最終ログ時刻を同じ規則で決め、無音判定が
  // 食い違わない。
  let harness =
    GrpcHarness::builder().single_project(project("takt-dash", project_dir()), server_meta(), mcp_state()).build();
  let fixture = running_run_files(&harness);
  fixture.set_body_log_modified("run-1", time("2026-11-13T09:03:00Z"));
  let mut poller = Poller::new(
    harness.config_path().to_path_buf(),
    harness.activity.clone(),
    harness.store.clone(),
    harness.changes.clone(),
    harness.registry.clone(),
  );
  poller.read_cycle_once().await.expect("1サイクルを実行できます");
  poller.shutdown().await;
  let listed = harness
    .store
    .slot("takt-dash")
    .and_then(|slot| slot.snapshot)
    .and_then(|snapshot| snapshot.runs.into_iter().find(|run| run.run_slug == "run-1"))
    .expect("running の run が一覧にあります");

  let detail = request_detail(&harness, "run-1").await.expect("run 詳細を取得できます");

  assert_eq!(listed.last_log_at, Some(time("2026-11-13T09:05:00Z")), "一覧は現在ステップの開始を採ります");
  assert_eq!(detail.last_log_at, listed.last_log_at.map(proto_timestamp), "詳細も一覧と同じ時刻です");
}

#[tokio::test]
async fn 完了した実行の詳細は実行中一覧になくても取得できる() {
  // 状態ストアの runs が空でも、完了タスクの runSlug からプロジェクトを解決して詳細を返します。
  // 状態ストアの完了タスクからゴールとタスク名を補います。
  let harness =
    GrpcHarness::builder().single_project(project("takt-dash", project_dir()), server_meta(), json!({})).build();
  let fixture = run_files(&harness);
  let mut meta = running_meta("review", 2, WORKFLOW, "2026-11-13T09:00:00Z");
  meta["status"] = json!("completed");
  fixture.write_meta("run-1", &meta);
  fixture.append_records("run-1", &[workflow_start_record(WORKFLOW)]);
  let snapshot = ProjectSnapshot {
    goals: vec![],
    tasks: vec![TaskEntry {
      name:            TaskName::try_from("implement").expect("テストのタスク名は有効です"),
      goal_id:         Some(GoalId::try_from("g1").expect("テストのゴール ID は有効です")),
      status:          TaskStatus::Completed,
      started_at:      Some(time("2026-11-13T09:00:00Z")),
      run_slug:        Some(RunSlug::try_from("run-1").expect("テストの run slug は有効です")),
      source_run_slug: None,
      current_step:    Some("review".to_owned()),
      merge_task:      false,
    }],
    inquiries: vec![],
    runs: vec![],
    notifications: vec![],
    proposals: vec![],
    ..ProjectSnapshot::empty()
  };
  harness.store.record_success("takt-dash", snapshot, time("2026-11-13T12:00:00Z"));

  let detail = request_detail(&harness, "run-1").await.expect("完了後も同じ runSlug で詳細を取得できます");

  assert_eq!(detail.run_slug, "run-1");
  assert_eq!(detail.project, "takt-dash", "完了タスクの runSlug からプロジェクトを解決します");
  assert_eq!(detail.goal_id, "g1", "状態ストアの完了タスクからゴールを補います");
  assert_eq!(detail.task_name, "implement", "状態ストアの完了タスクからタスク名を補います");
  assert_eq!(detail.status, RunStatus::Completed as i32, "status は meta.json の値です");
}

#[tokio::test]
async fn get_run_detail_resolves_a_completed_task_that_only_has_a_source_run_slug() {
  // 完了タスクが runSlug を持たず sourceRunSlug だけを持つ場合でも、状態ストアの投影から元 run の
  // プロジェクトを解決し、対応表の source_run_slug から run の保存場所を解決できる。別の slug では
  // NotFound になることを対で観測する。
  let harness = GrpcHarness::builder()
    .single_project(
      project("takt-dash", project_dir()),
      server_meta(),
      json!({
        "goals": [],
        "tasks": [
          { "name": "implement", "goalId": "g1", "status": "completed", "startedAt": "2026-11-13T09:00:00Z", "sourceRunSlug": "run-source" }
        ]
      }),
    )
    .build();
  let fixture = RunFixture::new(harness.project_dir());
  fixture.write_tasks(&[RunTaskLine { run_slug: None, source_run_slug: Some("run-source") }]);
  let mut meta = running_meta("review", 1, WORKFLOW, "2026-11-13T09:00:00Z");
  meta["status"] = json!("completed");
  fixture.write_meta("run-source", &meta);
  fixture.append_records("run-source", &[workflow_start_record(WORKFLOW)]);
  let mut poller = Poller::new(
    harness.config_path().to_path_buf(),
    harness.activity.clone(),
    harness.store.clone(),
    harness.changes.clone(),
    harness.registry.clone(),
  );
  poller.read_cycle_once().await.expect("1サイクルを実行できます");
  poller.shutdown().await;
  let processes_before = harness.recorded_processes().len();
  let calls_before = harness.recorded_calls().len();

  let detail = request_detail(&harness, "run-source")
    .await
    .expect("sourceRunSlug だけを持つ完了タスクからも run 詳細を取得できます");
  assert_eq!(detail.run_slug, "run-source");
  assert_eq!(detail.project, "takt-dash", "sourceRunSlug からプロジェクトを解決します");
  assert_eq!(detail.goal_id, "g1", "ゴールは sourceRunSlug が一致する完了タスクから取ります");
  assert_eq!(detail.task_name, "implement", "タスク名は sourceRunSlug が一致する完了タスクから取ります");
  assert_only_workflow_definition_reads(&harness, processes_before, calls_before);

  let status =
    request_detail(&harness, "does-not-exist").await.expect_err("sourceRunSlug と異なる slug は解決しません");
  assert_eq!(status.code(), tonic::Code::NotFound, "未知の runSlug は Not Found です: {status:?}");
}

#[tokio::test]
async fn get_run_detail_fills_goal_and_task_name_from_a_stored_task_that_only_has_a_source_run_slug() {
  // 状態ストアの完了タスクが runSlug を持たず sourceRunSlug だけを持つ場合でも、
  // 状態ストアの完了タスクからゴールとタスク名を補うことを観測します（grpc 側の完了タスク検索が
  // sourceRunSlug も照合する）。
  let harness =
    GrpcHarness::builder().single_project(project("takt-dash", project_dir()), server_meta(), json!({})).build();
  let fixture = RunFixture::new(harness.project_dir());
  fixture.write_tasks(&[RunTaskLine { run_slug: None, source_run_slug: Some("run-1") }]);
  let mut meta = running_meta("review", 1, WORKFLOW, "2026-11-13T09:00:00Z");
  meta["status"] = json!("completed");
  fixture.write_meta("run-1", &meta);
  fixture.append_records("run-1", &[workflow_start_record(WORKFLOW)]);
  let snapshot = ProjectSnapshot {
    goals: vec![],
    tasks: vec![TaskEntry {
      name:            TaskName::try_from("implement").expect("テストのタスク名は有効です"),
      goal_id:         Some(GoalId::try_from("g1").expect("テストのゴール ID は有効です")),
      status:          TaskStatus::Completed,
      started_at:      Some(time("2026-11-13T09:00:00Z")),
      run_slug:        None,
      source_run_slug: Some("run-1".to_owned()),
      current_step:    Some("review".to_owned()),
      merge_task:      false,
    }],
    inquiries: vec![],
    runs: vec![],
    notifications: vec![],
    proposals: vec![],
    ..ProjectSnapshot::empty()
  };
  harness.store.record_success("takt-dash", snapshot, time("2026-11-13T12:00:00Z"));

  let detail =
    request_detail(&harness, "run-1").await.expect("sourceRunSlug だけを持つ完了タスクからも run 詳細を取得できます");

  assert_eq!(detail.run_slug, "run-1");
  assert_eq!(detail.project, "takt-dash", "sourceRunSlug からプロジェクトを解決します");
  assert_eq!(detail.goal_id, "g1", "状態ストアの完了タスクからゴールを補います");
  assert_eq!(detail.task_name, "implement", "状態ストアの完了タスクからタスク名を補います");
  assert_eq!(detail.status, RunStatus::Completed as i32);
}

#[tokio::test]
async fn get_run_detail_fills_goal_id_from_a_listed_task_that_only_has_a_source_run_slug() {
  // takt_list_tasks の完了タスクが runSlug を持たず sourceRunSlug だけを持つ場合でも、
  // そのタスクからゴールを補うことを観測します。ローカルで読める run の GetRunDetail は takt-mcp を
  // 呼ばないため、
  // takt_list_tasks のタスクは定期読み取りで状態ストアへ入り、そこから補われます。
  let harness = GrpcHarness::builder()
    .single_project(
      project("takt-dash", project_dir()),
      server_meta(),
      json!({
        "tasks": [
          {
            "name": "implement",
            "goalId": "g1",
            "status": "completed",
            "startedAt": "2026-11-13T09:00:00Z",
            "sourceRunSlug": "run-1"
          }
        ]
      }),
    )
    .build();
  let fixture = RunFixture::new(harness.project_dir());
  fixture.write_tasks(&[RunTaskLine { run_slug: None, source_run_slug: Some("run-1") }]);
  let mut meta = running_meta("review", 1, WORKFLOW, "2026-11-13T09:00:00Z");
  meta["status"] = json!("completed");
  fixture.write_meta("run-1", &meta);
  fixture.append_records("run-1", &[workflow_start_record(WORKFLOW)]);
  let mut poller = Poller::new(
    harness.config_path().to_path_buf(),
    harness.activity.clone(),
    harness.store.clone(),
    harness.changes.clone(),
    harness.registry.clone(),
  );
  poller.read_cycle_once().await.expect("1サイクルを実行できます");
  poller.shutdown().await;

  let detail =
    request_detail(&harness, "run-1").await.expect("sourceRunSlug だけを持つ完了タスクからも run 詳細を取得できます");

  assert_eq!(detail.run_slug, "run-1");
  assert_eq!(detail.goal_id, "g1", "takt_list_tasks の完了タスクからゴールを補います");
  assert_eq!(detail.task_name, "implement", "タスク名を補います");
  assert_eq!(detail.status, RunStatus::Completed as i32);
}

#[tokio::test]
async fn unreadable_full_history_keeps_the_readable_tail_and_its_progress() {
  let mut state = mcp_state();
  state["workflows"] = json!({WORKFLOW:{"initialStep":"head","steps":[{"name":"head","rules":[{"next":"tail"}]},{"name":"tail","rules":[{"next":"COMPLETE"}]}]}});
  let harness = seeded_harness(state);
  let fixture = run_files(&harness);
  fixture.write_meta("run-1", &running_meta("tail", 2, WORKFLOW, "2026-11-13T09:00:00Z"));
  std::fs::write(fixture.body_log("run-1"), "invalid JSON before display window\n").unwrap();
  append_filler_records(&fixture, "run-1", 2 * 1024 * 1024);
  fixture.append_records("run-1", &[step_complete_record("tail", "coder", 2, "2026-11-13T09:30:00Z")]);
  let detail = request_detail(&harness, "run-1").await.unwrap();
  assert_eq!(detail.step_logs.len(), 1);
  assert_eq!(detail.step_logs[0].step_name, "tail");
  assert_eq!(detail.progress.unwrap().nodes[1].log_index, Some(0));
  assert_eq!(detail.status, RunStatus::Running as i32);
}

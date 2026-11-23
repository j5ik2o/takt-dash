//! 実 RPC による再構築と、読取周期の世代対応を検証する。

#[path = "../tests/common/mod.rs"]
mod common;
#[path = "../tests/grpc_harness/mod.rs"]
mod grpc_harness;

use std::{path::Path, sync::Arc};

use common::{project, running_meta, server_meta, RunFixture, RunTaskLine};
use grpc_harness::{client, start_server, GrpcHarness};
use serde_json::{json, Value};
use tokio::{
  sync::oneshot,
  time::{timeout, Duration},
};

use super::Poller;
use crate::{
  config::{load_config, McpServer},
  mcp::{client_config, Lane},
  model::{EventKind, ExecutionStatus, ProjectSnapshot},
  takt_dash::v1::{ApproveGoalRegistrationRequest, GetActivityRequest, GetProjectsRequest},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReadBoundary {
  BeforeAcquisition,
  AfterAcquisition,
  BeforeApply,
}

pub(super) struct ReadBarrier {
  boundary: ReadBoundary,
  reached:  oneshot::Sender<()>,
  resume:   oneshot::Receiver<()>,
}

impl Poller {
  pub(super) async fn pause_read(&mut self, boundary: ReadBoundary) {
    if self.read_barrier.as_ref().is_some_and(|barrier| barrier.boundary == boundary) {
      let barrier = self.read_barrier.take().expect("対象の同期点があります");
      barrier.reached.send(()).expect("制御側が到達通知を待っています");
      barrier.resume.await.expect("RPC 成功後の再開許可が必要です");
    }
  }
}

fn arm(poller: &mut Poller, boundary: ReadBoundary) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
  assert!(poller.read_barrier.is_none(), "未消費の同期点を上書きしません");
  let (reached, arrived) = oneshot::channel();
  let (resume, permission) = oneshot::channel();
  poller.read_barrier = Some(ReadBarrier { boundary, reached, resume: permission });
  (arrived, resume)
}

fn harness() -> GrpcHarness {
  GrpcHarness::builder()
    .single_project(project("takt-dash", Path::new("/abs/takt-dash")), server_meta(), json!({}))
    .build()
}

fn poller(harness: &GrpcHarness) -> Poller {
  Poller::new(
    harness.config_path().to_path_buf(),
    harness.activity.clone(),
    harness.store.clone(),
    harness.changes.clone(),
    harness.registry.clone(),
  )
}

const GOAL: &str = "3f9c1a54-2b41-4d1e-9d2e-2c1a5b8f7e11";
const ADDED_A: &str = "7c1f4b06-6d24-4b7f-9c1a-2b3c4d5e6f70";
const ADDED_B: &str = "8c1f4b06-6d24-4b7f-9c1a-2b3c4d5e6f70";

#[tokio::test]
async fn same_server_stdio_to_http_retires_children_and_keeps_write_order() {
  let harness = harness();
  let mut poller = poller(&harness);
  let registry = &harness.registry;
  let write_gate = registry.write_gate("mac-mini", "takt-dash");
  harness.rewrite_state(json!({"goals": [goal(GOAL, "active")]}));
  assert_eq!(poller.read_cycle_once().await.unwrap().projects_read, 1);

  let grpc = start_server(&harness).await;
  let mut client = client(grpc.address).await;
  let approval = |revision| ApproveGoalRegistrationRequest {
    server_id: "mac-mini".to_owned(),
    project: "takt-dash".to_owned(),
    revision,
  };
  client.approve_goal_registration(approval(1)).await.expect("stdio で書き込めます");
  assert_eq!(registry.write_sequence("mac-mini", "takt-dash"), 1);

  // 画面読み取りレーンも起動してから、ハンドルをレジストリだけが所有する状態にします。
  let stdio_config = load_config(harness.config_path()).unwrap();
  let McpServer::Stdio(stdio) = &stdio_config.servers["mac-mini"] else {
    panic!("切替前の接続は stdio です");
  };
  let (screen, _) =
    registry.client("mac-mini", "takt-dash", Lane::Screen, || client_config(stdio, harness.project_dir()));
  screen.lock().await.warm_up().await.unwrap();
  drop(screen);
  let old_pids = harness.recorded_processes();
  assert_eq!(old_pids.len(), 3, "定期読み取り、画面読み取り、書き込みの子プロセスが起動します");
  assert!(old_pids.iter().copied().all(common::process_exists));

  let http_state = harness.project_dir().join("http-state.json");
  std::fs::write(&http_state, json!({"goals": [goal(ADDED_A, "paused")]}).to_string()).unwrap();
  let http = common::HttpMockServer::start(&http_state, None);
  let mut config: Value = serde_json::from_str(&std::fs::read_to_string(harness.config_path()).unwrap()).unwrap();
  config["mcpServers"]["mac-mini"] = json!({
    "type": "http",
    "url": http.url(),
    "headers": { "Authorization": format!("Bearer {}", http.token()) },
  });
  std::fs::write(harness.config_path(), config.to_string()).unwrap();
  // HTTP への切替周期と次周期のそれぞれで、読み取りが完了した直後に書き込みを挟みます。
  for (revision, write_sequence) in [(2, 2), (3, 3)] {
    let (reached, resume) = arm(&mut poller, ReadBoundary::BeforeApply);
    let control = async {
      reached.await.expect("HTTP の読み取りが完了します");
      assert!(Arc::ptr_eq(&write_gate, &registry.write_gate("mac-mini", "takt-dash")));
      assert_eq!(registry.write_sequence("mac-mini", "takt-dash"), write_sequence - 1);
      client.approve_goal_registration(approval(revision)).await.expect("HTTP で書き込めます");
      assert_eq!(registry.write_sequence("mac-mini", "takt-dash"), write_sequence);
      resume.send(()).expect("読み取り結果の反映を再開できます");
    };
    let (cycle, ()) = timeout(Duration::from_secs(15), async { tokio::join!(poller.read_cycle_once(), control) })
      .await
      .expect("読み取り、書き込み、反映の判定が完了します");
    let cycle = cycle.unwrap();
    assert_eq!(cycle.projects_failed, 0);
    assert_eq!(cycle.projects_read, 0, "書き込み前に取得した結果を捨てます");
    assert_eq!(cycle.events_recorded, 0);
    let slot = harness.store.slot("takt-dash").expect("切替後も同じプロジェクトが残ります");
    assert!(slot.snapshot.is_none(), "切替時に初期化した状態に古い HTTP 結果を反映しません");

    if revision == 2 {
      assert_stdio_children_exit(&old_pids).await;
    }
  }
  std::fs::write(&http_state, json!({"goals": [goal(ADDED_B, "active")]}).to_string()).unwrap();
  assert_eq!(poller.read_cycle_once().await.unwrap().projects_read, 1);
  assert_eq!(snapshot(&harness).goals[0].goal_id.as_str(), ADDED_B, "書き込み後に取得した HTTP 結果は反映します");
  assert_eq!(harness.recorded_processes(), old_pids, "切替後に stdio の子プロセスを起動しません");
  let writes: Vec<_> = http
    .recorded_calls()
    .into_iter()
    .filter(|call| call["name"] == "takt_approve_goal_registration")
    .map(|call| call["arguments"]["revision"].clone())
    .collect();
  assert_eq!(writes, [json!(2), json!(3)], "切替後の書き込みは HTTP に届きます");
  poller.shutdown().await;
  drop(client);
  grpc.stop().await;
}

async fn assert_stdio_children_exit(old_pids: &[u32]) {
  let exited = timeout(Duration::from_secs(3), async {
    while old_pids.iter().copied().any(common::process_exists) {
      tokio::time::sleep(Duration::from_millis(25)).await;
    }
  })
  .await;
  let remaining: Vec<_> = old_pids.iter().copied().filter(|pid| common::process_exists(*pid)).collect();
  assert!(exited.is_ok(), "HTTP への切替後に旧 stdio 子プロセスが残っています: {remaining:?}");
}

fn goal(id: &str, execution: &str) -> Value {
  json!({
    "id": id, "status": "created", "executionStatus": execution,
    "objective": format!("ゴール {id}"), "integrationBranch": format!("takt/{id}")
  })
}

fn state(cwd_a: &Path, cwd_b: &Path, execution: &str) -> Value {
  json!({
    "goals": [goal(GOAL, execution)],
    "tasks": [{"name": "task-r1", "status": "running", "runSlug": "r1",
               "startedAt": "2026-11-13T08:00:00Z"}],
    "workflows": {"development-core": ["plan", "implement"]},
    "proposalsByCwd": {
      cwd_a.display().to_string(): [{"revision": 3, "repositoryDirectory": cwd_a, "summary": "A の提案"}],
      cwd_b.display().to_string(): [{"revision": 4, "repositoryDirectory": cwd_b, "summary": "B の提案"}]
    }
  })
}

fn write_run(cwd: &Path, step: &str) {
  let fixture = RunFixture::new(cwd);
  fixture.write_tasks(&[RunTaskLine { run_slug: Some("r1"), source_run_slug: None }]);
  fixture.write_meta("r1", &running_meta(step, 1, "development-core", "2026-11-13T09:00:00Z"));
  fixture.append_records("r1", &[common::workflow_start_record("development-core")]);
}

fn snapshot(harness: &GrpcHarness) -> ProjectSnapshot {
  harness.store.slot("takt-dash").expect("対象プロジェクトがあります").snapshot.expect("読取結果があります")
}

fn assert_snapshot(harness: &GrpcHarness, execution: ExecutionStatus, revision: i32, step: &str) {
  let snapshot = snapshot(harness);
  let goal = snapshot.goals.iter().find(|goal| goal.goal_id.as_str() == GOAL).expect("対象ゴールがあります");
  assert_eq!(goal.execution_status, execution, "取得した世代のゴールを読みます");
  assert_eq!(snapshot.proposals.len(), 1);
  assert_eq!(snapshot.proposals[0].revision.get(), revision, "取得した cwd の提案を保存します");
  assert_eq!(snapshot.runs.len(), 1, "ローカル run を保存します");
  assert_eq!(snapshot.runs[0].current_step.as_deref(), Some(step), "取得した cwd の run を保存します");
}

fn assert_events(harness: &GrpcHarness, created_ids: &[&str]) {
  let events = harness.activity.recent_events(50).expect("SQLite の出来事を読めます");
  assert_eq!(events.len(), created_ids.len(), "世代間の出来事を追加しません");
  for id in created_ids {
    assert_eq!(
      events.iter().filter(|event| event.kind == EventKind::GoalCreated && event.body.contains(id)).count(),
      1,
      "同じ Poller が世代内のゴール {id} の追加を1件だけ記録します"
    );
  }
}

async fn rebuild_through_rpc(boundary: ReadBoundary) {
  let harness = harness();
  let mut poller = poller(&harness);
  let alternate = tempfile::tempdir().expect("B のディレクトリを作れます");
  let cwd_a = harness.project_cwd("takt-dash");
  let cwd_b = alternate.path();
  write_run(cwd_a, "plan");
  write_run(cwd_b, "implement");
  let mut a = state(cwd_a, cwd_b, "active");
  harness.rewrite_state(a.clone());

  let initial = poller.read_cycle_once().await.expect("A の初回を読めます");
  assert_eq!(initial.projects_read, 1);
  assert_eq!(initial.events_recorded, 0);
  assert_snapshot(&harness, ExecutionStatus::Active, 3, "plan");
  assert_events(&harness, &[]);

  a["goals"].as_array_mut().expect("ゴール一覧があります").push(goal(ADDED_A, "active"));
  harness.rewrite_state(a);
  let after_acquisition = boundary == ReadBoundary::AfterAcquisition;
  let mut b = state(cwd_a, cwd_b, "paused");
  let b_state = cwd_b.join("state-b.json");
  std::fs::write(&b_state, b.to_string()).expect("B の状態を書けます");
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;
  let calls_before = harness.recorded_calls().len();
  let (reached, resume) = arm(&mut poller, boundary);

  let control = async {
    reached.await.expect("対象 Poller が取得境界へ到達します");
    let mut config: Value =
      serde_json::from_str(&std::fs::read_to_string(harness.config_path()).expect("設定を読めます"))
        .expect("設定を解析できます");
    config["projects"][0]["cwd"] = json!(cwd_b);
    let args = config["mcpServers"]["mac-mini"]["args"].as_array_mut().expect("起動引数があります");
    let index = args.iter().position(|arg| arg == "--state").expect("--state があります");
    args[index + 1] = json!(b_state);
    std::fs::write(harness.config_path(), config.to_string()).expect("B の設定を書けます");
    let proposal = &b["proposalsByCwd"][cwd_b.to_str().expect("B の cwd は文字列です")][0];
    assert_eq!(proposal["repositoryDirectory"], json!(cwd_b));
    assert_eq!(proposal["revision"], json!(4));
    let before_approval = harness.recorded_calls().len();
    let response = client
      .approve_goal_registration(ApproveGoalRegistrationRequest {
        server_id: "mac-mini".to_owned(),
        project:   "takt-dash".to_owned(),
        revision:  4,
      })
      .await
      .expect("実 RPC が B を登録して成功します")
      .into_inner();
    assert_eq!(json!(response.revision), proposal["revision"]);
    let approvals: Vec<_> = harness
      .recorded_calls()
      .into_iter()
      .skip(before_approval)
      .filter(|call| call.name == "takt_approve_goal_registration")
      .collect();
    assert_eq!(approvals.len(), 1);
    assert_eq!(approvals[0].arguments["cwd"], proposal["repositoryDirectory"]);
    assert_eq!(approvals[0].arguments["revision"], proposal["revision"]);
    resume.send(()).expect("対象周期へ再開許可を渡せます");
  };
  let (interrupted, ()) = timeout(Duration::from_secs(15), async { tokio::join!(poller.read_cycle_once(), control) })
    .await
    .expect("到達、RPC 成功、再開の順で完了します");
  let interrupted = interrupted.expect("割込み周期が成功します");
  let expected_reads = if after_acquisition { 0 } else { 1 };
  assert_eq!(interrupted.projects_read, expected_reads, "取得後に書き込みが成功した結果は採用しません");
  assert_eq!(interrupted.projects_failed, 0);

  let reads: Vec<_> = harness
    .recorded_calls()
    .into_iter()
    .skip(calls_before)
    .filter(|call| call.name == "takt_list_goals" || call.name == "takt_list_goal_registration_proposals")
    .collect();
  assert_eq!(reads.len(), 2, "割込み周期の道具呼出しだけを検査します");
  if after_acquisition {
    assert_snapshot(&harness, ExecutionStatus::Active, 3, "plan");
    assert_eq!(snapshot(&harness).goals.len(), 1);
    assert_eq!(interrupted.events_recorded, 0, "破棄した A の読み取りから出来事を記録しません");
    assert_events(&harness, &[]);
  } else {
    assert_eq!(interrupted.events_recorded, 1, "書き込みレーンの置換だけでは A の比較基準を失いません");
    assert_snapshot(&harness, ExecutionStatus::Active, 3, "plan");
    assert_eq!(snapshot(&harness).goals.len(), 2);
    assert_events(&harness, &[ADDED_A]);
  }
  for read in reads {
    assert_eq!(read.arguments["cwd"], json!(cwd_a), "{} は準備済みの poller レーンの読取先を使います", read.name);
  }

  let created_before_b = if after_acquisition { vec![] } else { vec![ADDED_A] };
  let calls_before_b = harness.recorded_calls().len();
  let first_b = poller.read_cycle_once().await.expect("同じ Poller で B 初回を読めます");
  assert_eq!(first_b.projects_read, 1);
  assert_eq!(first_b.projects_failed, 0);
  assert_eq!(first_b.events_recorded, 0, "B 初回は基準化のみです");
  assert_snapshot(&harness, ExecutionStatus::Paused, 4, "implement");
  assert_eq!(snapshot(&harness).goals.len(), 1);
  assert_events(&harness, &created_before_b);

  b["goals"].as_array_mut().expect("ゴール一覧があります").push(goal(ADDED_B, "active"));
  std::fs::write(&b_state, b.to_string()).expect("B 内の実変化を書けます");
  let next_b = poller.read_cycle_once().await.expect("同じ Poller で B の次周期を読めます");
  assert_eq!(next_b.projects_read, 1);
  assert_eq!(next_b.projects_failed, 0);
  assert_eq!(next_b.events_recorded, 1, "B 内の実変化を1件だけ記録します");
  assert_snapshot(&harness, ExecutionStatus::Paused, 4, "implement");
  assert_eq!(snapshot(&harness).goals.len(), 2);
  assert!(snapshot(&harness).goals.iter().any(|goal| goal.goal_id.as_str() == ADDED_B));
  let created = if after_acquisition { vec![ADDED_B] } else { vec![ADDED_A, ADDED_B] };
  assert_events(&harness, &created);
  let reads: Vec<_> = harness
    .recorded_calls()
    .into_iter()
    .skip(calls_before_b)
    .filter(|call| call.name == "takt_list_goals" || call.name == "takt_list_goal_registration_proposals")
    .collect();
  assert_eq!(reads.len(), 4, "B の初回と次周期の道具呼出しを検査します");
  for read in reads {
    assert_eq!(read.arguments["cwd"], json!(cwd_b), "{} は切り替え後の読取先を使います", read.name);
  }
  poller.shutdown().await;
  drop(client);
  server.stop().await;
}

#[tokio::test]
async fn rpc_rebuild_before_acquisition_keeps_the_prepared_poller_cwd() {
  rebuild_through_rpc(ReadBoundary::BeforeAcquisition).await;
}

async fn assert_assignment_events(harness: &GrpcHarness, created_ids: &[&str]) {
  assert_events(harness, created_ids);
  let server = start_server(harness).await;
  let mut client = client(server.address).await;
  let projects = client.get_projects(GetProjectsRequest {}).await.expect("一覧 API が成功します").into_inner();
  let activity = client.get_activity(GetActivityRequest {}).await.expect("活動 API が成功します").into_inner();
  assert_eq!(projects.recent_events, activity.events);
  assert_eq!(activity.events.len(), created_ids.len());
  for id in created_ids {
    let event = activity.events.iter().find(|event| event.body.contains(id)).expect("追加を API で読めます");
    assert_eq!(event.server_id, "mini-two");
    assert_eq!(event.project.as_deref(), Some("takt-dash"));
  }
  drop(client);
  server.stop().await;
}

async fn reassign_server(first_read_fails: bool, remove_first: bool) {
  let harness = harness();
  harness.rewrite_state(json!({"goals": [goal(GOAL, "active")]}));
  let mut poller = poller(&harness);
  assert_eq!(poller.read_cycle_once().await.unwrap().events_recorded, 0);
  let old = harness.store.project_views().remove(0);
  assert!(old.last_read_at.is_some());
  let b_state = harness.project_cwd("takt-dash").join("server-b.json");
  let mut b = json!({"goals": [goal(ADDED_A, "active")]});
  let initial_b = if first_read_fails { json!({"isError": ["takt_list_goals"]}) } else { b.clone() };
  std::fs::write(&b_state, initial_b.to_string()).unwrap();
  let mut config: Value = serde_json::from_str(&std::fs::read_to_string(harness.config_path()).unwrap()).unwrap();
  if remove_first {
    let mut removed = config.clone();
    removed["projects"] = json!([]);
    std::fs::write(harness.config_path(), removed.to_string()).unwrap();
    let report = poller.read_cycle_once().await.unwrap();
    assert_eq!(report.projects_read, 0);
    assert_eq!(report.projects_failed, 0);
    assert_eq!(report.events_recorded, 0);
    assert!(harness.store.project_views().is_empty(), "削除設定を1周期採用します");
    assert_assignment_events(&harness, &[]).await;
  }
  config["mcpServers"]["mini-two"] = config["mcpServers"]["mac-mini"].clone();
  let args = config["mcpServers"]["mini-two"]["args"].as_array_mut().unwrap();
  let index = args.iter().position(|arg| arg == "--state").unwrap();
  args[index + 1] = json!(b_state);
  config["servers"]["mini-two"] = config["servers"]["mac-mini"].clone();
  config["projects"][0]["server"] = json!("mini-two");
  std::fs::write(harness.config_path(), config.to_string()).unwrap();
  let (reached, resume) = arm(&mut poller, ReadBoundary::BeforeAcquisition);
  let (report, exposed) = timeout(Duration::from_secs(15), async {
    tokio::join!(poller.read_cycle_once(), async {
      reached.await.unwrap();
      let exposed = harness.store.project_views().remove(0);
      resume.send(()).unwrap();
      exposed
    })
  })
  .await
  .unwrap();
  let report = report.unwrap();
  let first = harness.store.project_views().remove(0);
  assert_eq!(exposed.server_id, "mini-two");
  assert!(exposed.snapshot.is_none(), "新所属の公開時点で旧 snapshot を持ち越しません");
  assert!(exposed.last_read_at.is_none(), "新所属へ旧成功時刻を持ち越しません");
  assert!(exposed.goals.is_empty());
  if first_read_fails {
    assert_eq!(report.projects_failed, 1);
    assert_eq!(report.projects_read, 0);
    assert!(first.snapshot.is_none());
    assert!(first.last_read_at.is_none());
    assert!(first.goals.is_empty());
    assert_assignment_events(&harness, &[]).await;
    std::fs::write(&b_state, b.to_string()).unwrap();
    let recovered = poller.read_cycle_once().await.unwrap();
    assert_eq!(recovered.events_recorded, 0);
  } else {
    assert_eq!(report.projects_read, 1);
    assert_eq!(report.projects_failed, 0);
    assert_eq!(report.events_recorded, 0);
  }
  let first_success = harness.store.project_views().remove(0);
  assert_eq!(first_success.server_id, "mini-two");
  assert_eq!(first_success.goals.len(), 1);
  assert_eq!(first_success.goals[0].goal_id.as_str(), ADDED_A);
  assert!(first_success.last_read_at.is_some());
  assert_assignment_events(&harness, &[]).await;
  b["goals"].as_array_mut().unwrap().push(goal(ADDED_B, "active"));
  std::fs::write(&b_state, b.to_string()).unwrap();
  assert_eq!(poller.read_cycle_once().await.unwrap().events_recorded, 1);
  let goals = harness.store.project_views().remove(0).goals;
  assert_eq!(goals.len(), 2);
  assert!(goals.iter().any(|goal| goal.goal_id.as_str() == ADDED_A));
  assert!(goals.iter().any(|goal| goal.goal_id.as_str() == ADDED_B));
  assert_assignment_events(&harness, &[ADDED_B]).await;
  assert_eq!(poller.read_cycle_once().await.unwrap().events_recorded, 0);
  assert_assignment_events(&harness, &[ADDED_B]).await;
  poller.shutdown().await;
}

#[tokio::test]
async fn server_reassignment_baselines_the_first_success() {
  reassign_server(false, false).await;
}

#[tokio::test]
async fn server_reassignment_clears_old_projection_before_first_failure_and_recovers() {
  reassign_server(true, false).await;
}

#[tokio::test]
async fn server_readdition_baselines_the_first_success() {
  reassign_server(false, true).await;
}

#[tokio::test]
async fn server_readdition_baselines_recovery_after_the_first_failure() {
  reassign_server(true, true).await;
}

#[tokio::test]
async fn unchanged_server_failure_keeps_the_last_success() {
  let harness = harness();
  harness.rewrite_state(json!({"goals": [goal(GOAL, "active")]}));
  let mut poller = poller(&harness);
  poller.read_cycle_once().await.unwrap();
  let before = harness.store.project_views().remove(0);
  harness.rewrite_state(json!({"isError": ["takt_list_goals"]}));
  assert_eq!(poller.read_cycle_once().await.unwrap().projects_failed, 1);
  let after = harness.store.project_views().remove(0);
  assert_eq!(after.server_id, before.server_id);
  assert_eq!(after.snapshot, before.snapshot);
  assert_eq!(after.last_read_at, before.last_read_at);
  assert_events(&harness, &[]);
  poller.shutdown().await;
}

#[tokio::test]
async fn rpc_rebuild_after_acquisition_discards_the_stale_read() {
  rebuild_through_rpc(ReadBoundary::AfterAcquisition).await;
}

#[tokio::test]
async fn same_named_pollers_cannot_consume_or_clear_each_others_barrier() {
  let target = harness();
  let other = harness();
  let mut target_poller = poller(&target);
  let mut other_poller = poller(&other);
  let (mut reached, resume) = arm(&mut target_poller, ReadBoundary::AfterAcquisition);
  other_poller.read_cycle_once().await.expect("別レジストリの同名プロジェクトを先に読めます");
  assert_eq!(reached.try_recv(), Err(oneshot::error::TryRecvError::Empty));
  let (_, cancelled) = arm(&mut other_poller, ReadBoundary::AfterAcquisition);
  drop(cancelled);
  drop(other_poller);
  assert_eq!(reached.try_recv(), Err(oneshot::error::TryRecvError::Empty));
  let (report, ()) = timeout(Duration::from_secs(15), async {
    tokio::join!(target_poller.read_cycle_once(), async {
      reached.await.expect("対象だけが到達します");
      resume.send(()).expect("対象だけを再開できます");
    })
  })
  .await
  .expect("別 Poller の破棄後も対象の同期が成立します");
  assert_eq!(report.expect("対象を読めます").projects_read, 1);
  target_poller.shutdown().await;
  other.registry.shutdown_all().await;
}

#[tokio::test]
#[should_panic(expected = "RPC 成功後の再開許可が必要です")]
async fn cancelled_resume_is_not_a_successful_interruption() {
  let harness = harness();
  let mut poller = poller(&harness);
  let (_reached, resume) = arm(&mut poller, ReadBoundary::AfterAcquisition);
  drop(resume);
  poller.read_cycle_once().await.expect("周期を実行します");
}

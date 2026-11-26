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
  domain::{RegistrationRevision, RunSlug},
  mcp::{client_config, Lane},
  model::{EventKind, ExecutionStatus, ProjectSnapshot},
  takt_dash::v1::{ApproveGoalMergeRequest, GetActivityRequest, GetPendingDecisionsRequest, GetProjectsRequest},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReadBoundary {
  BeforeAcquisition,
  AfterAcquisition,
  BeforeApply,
  AfterProjectsPublished,
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
  // 書き込みには、状態ストアの判断待ちと照合しないマージの承認を使います。接続の切替は状態
  // ストアを初期化するため、一覧と照合する書き込み（回答、ゴール登録の承認）は切替の直後に
  // 断られます。
  let merge = |pr_number| ApproveGoalMergeRequest {
    idempotency_key: None,
    server_id: "mac-mini".to_owned(),
    project: "takt-dash".to_owned(),
    goal_id: GOAL.to_owned(),
    pr_number,
    until: "merge".to_owned(),
  };
  client.approve_goal_merge(merge(1)).await.expect("stdio で書き込めます");
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
  for (pr_number, write_sequence) in [(2, 2), (3, 3)] {
    let (reached, resume) = arm(&mut poller, ReadBoundary::BeforeApply);
    let control = async {
      reached.await.expect("HTTP の読み取りが完了します");
      assert!(Arc::ptr_eq(&write_gate, &registry.write_gate("mac-mini", "takt-dash")));
      assert_eq!(registry.write_sequence("mac-mini", "takt-dash"), write_sequence - 1);
      client.approve_goal_merge(merge(pr_number)).await.expect("HTTP で書き込めます");
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

    if pr_number == 2 {
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
    .filter(|call| call["name"] == "takt_merge_goal_pr")
    .map(|call| call["arguments"]["prNumber"].clone())
    .collect();
  assert_eq!(writes, [json!(2), json!(3)], "切替後の書き込みは HTTP に届きます");

  // 切替前の設定でゲートを待っていた書き込みが、切替後にレーンを取得する順序を再現します。
  {
    let _guard = write_gate.lock().await;
    let (late_write, _) =
      registry.client("mac-mini", "takt-dash", Lane::Write, || client_config(stdio, harness.project_dir()));
    let mut late_client = late_write.lock().await;
    let revision = RegistrationRevision::try_from(4).unwrap();
    let result = common::write_tool_set(&mut late_client, harness.project_dir())
      .approve_goal_registration(revision, &common::default_conversation_id())
      .await
      .expect("切替前に解決した接続先で書き込みを完了できます");
    assert_eq!(result.revision, revision);
    assert_eq!(registry.record_write("mac-mini", "takt-dash"), 4);
  }
  let pids = harness.recorded_processes();
  assert_eq!(pids.len(), old_pids.len() + 1, "待機していた要求は解決済みの stdio 接続先を使えます");
  assert_stdio_children_exit(&pids[old_pids.len()..]).await;
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
    // 書き込みには、状態ストアの判断待ちと照合しないマージの承認を使います。この時点の状態
    // ストアは A の一覧を持つため、B にだけある登録案の承認は一覧との照合で断られます。
    let before_write = harness.recorded_calls().len();
    client
      .approve_goal_merge(ApproveGoalMergeRequest {
        idempotency_key: None,
        server_id:       "mac-mini".to_owned(),
        project:         "takt-dash".to_owned(),
        goal_id:         GOAL.to_owned(),
        pr_number:       4,
        until:           "merge".to_owned(),
      })
      .await
      .expect("実 RPC が B へ書き込んで成功します");
    let writes: Vec<_> = harness
      .recorded_calls()
      .into_iter()
      .skip(before_write)
      .filter(|call| call.name == "takt_merge_goal_pr")
      .collect();
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0].arguments["cwd"], json!(cwd_b));
    assert_eq!(writes[0].arguments["prNumber"], json!(4));
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

#[tokio::test]
async fn server_reassignment_publishes_the_new_server_without_the_old_projection() {
  // 同じ Poller と状態ストアの上で、server 割り当てを mac-mini から mini-two へ変える。
  // reset が set_projects より先に走っていれば、新 server_id の公開直後に旧 snapshot・
  // 旧接続状態・旧 run は見えない。reset を set_projects の後ろへ戻す実装では、この
  // 観測点で新 server_id と旧 projection が混在して検出できる。
  let harness = harness();
  let mut poller = poller(&harness);
  let cwd_a = harness.project_cwd("takt-dash").to_path_buf();
  write_run(&cwd_a, "plan");
  harness.rewrite_state(json!({
    "goals": [goal(GOAL, "active")],
    "tasks": [{"name": "task-r1", "status": "running", "runSlug": "r1",
               "startedAt": "2026-11-13T08:00:00Z"}],
  }));
  assert_eq!(poller.read_cycle_once().await.expect("mac-mini の初回を読めます").projects_read, 1);
  let before = harness.store.project_views().remove(0);
  assert_eq!(before.server_id, "mac-mini");
  assert!(before.goals.iter().any(|goal| goal.goal_id.as_str() == GOAL), "変更前に旧ゴールを保持します");
  let before_run = RunSlug::try_from("r1").expect("runSlug を作れます");
  assert_eq!(
    harness.store.find_run_project(&before_run).map(|view| view.server_id).as_deref(),
    Some("mac-mini"),
    "変更前は旧 run を旧 server のプロジェクトへ解決します"
  );
  // find_run_project はタスクの runSlug だけでも解決するので、run 一覧に r1 が載っていることは
  // 切り替え後と同じ gRPC 状態取得で別に確かめる。これが無いと、切り替え前から一覧が空でも通る。
  let grpc = start_server(&harness).await;
  let mut client = client(grpc.address).await;
  assert!(before.runs.iter().any(|run| run.run_slug == "r1"), "変更前は状態ストアの run 一覧に旧 run があります");
  let before_state = client
    .get_pending_decisions(GetPendingDecisionsRequest {})
    .await
    .expect("変更前の状態取得が成功します")
    .into_inner()
    .state
    .expect("変更前の状態があります");
  assert_eq!(before_state.projects.first().expect("プロジェクトがあります").server_id, "mac-mini");
  assert!(
    before_state.runs.iter().any(|run| run.run_slug == "r1"),
    "変更前は gRPC 状態取得の run 一覧に旧 run があります"
  );
  assert!(before_state.goals.iter().any(|goal| goal.goal_id == GOAL), "変更前は gRPC 状態取得に旧ゴールがあります");

  let b_state = cwd_a.join("server-b.json");
  let b = json!({"goals": [goal(ADDED_A, "active")]});
  std::fs::write(&b_state, b.to_string()).expect("mini-two の状態を書けます");
  let mut config: Value =
    serde_json::from_str(&std::fs::read_to_string(harness.config_path()).expect("設定を読めます"))
      .expect("設定を解析できます");
  config["mcpServers"]["mini-two"] = config["mcpServers"]["mac-mini"].clone();
  let args = config["mcpServers"]["mini-two"]["args"].as_array_mut().expect("起動引数があります");
  let index = args.iter().position(|arg| arg == "--state").expect("--state があります");
  args[index + 1] = json!(b_state);
  config["servers"]["mini-two"] = config["servers"]["mac-mini"].clone();
  config["projects"][0]["server"] = json!("mini-two");
  config["projects"][0]["writeServer"] = json!("mini-two");
  std::fs::write(harness.config_path(), config.to_string()).expect("mini-two の設定を書けます");

  let (reached, resume) = arm(&mut poller, ReadBoundary::AfterProjectsPublished);
  let (cycle, ()) = timeout(Duration::from_secs(15), async {
    tokio::join!(poller.read_cycle_once(), async {
      reached.await.expect("設定の公開後に到達します");
      let view = harness.store.project_views().remove(0);
      assert_eq!(view.server_id, "mini-two", "公開直後から新しい server_id を見ます");
      assert!(view.snapshot.is_none(), "公開直後に旧 snapshot が見えません");
      assert!(view.goals.is_empty(), "公開直後に旧ゴールが見えません");
      assert!(view.tasks.is_empty(), "公開直後に旧タスクが見えません");
      assert!(view.runs.is_empty(), "公開直後に旧 run が見えません");
      assert!(view.last_read_at.is_none(), "公開直後に旧成功時刻が見えません");
      assert!(view.connection_state.is_none(), "公開直後に旧接続状態が見えません");
      assert!(!view.connected, "公開直後に旧接続状態を接続中として見せません");
      assert!(
        harness.store.find_run_project(&before_run).is_none(),
        "公開直後に旧 run を新しい server_id のプロジェクトへ解決しません"
      );
      let projects = client
        .get_pending_decisions(GetPendingDecisionsRequest {})
        .await
        .expect("公開直後の状態取得が成功します")
        .into_inner();
      let state = projects.state.as_ref().expect("状態があります");
      let project = state.projects.first().expect("プロジェクトがあります");
      assert_eq!(project.server_id, "mini-two", "gRPC 状態取得でも新しい server_id を見ます");
      assert!(project.last_read_at.is_none(), "gRPC 状態取得でも旧成功時刻を見せません");
      assert!(!project.connected, "gRPC 状態取得でも旧接続状態を接続中として見せません");
      assert!(state.goals.iter().all(|goal| goal.goal_id != GOAL), "gRPC 状態取得のゴールに旧ゴールが混在しません");
      assert!(state.runs.iter().all(|run| run.run_slug != "r1"), "gRPC 状態取得の run に旧 run が混在しません");
      resume.send(()).expect("公開直後の観測後に周期を再開できます");
    })
  })
  .await
  .expect("公開直後の観測と周期の完了が 15 秒以内に終わります");
  let cycle = cycle.expect("切り替え周期が成功します");
  assert_eq!(cycle.projects_failed, 0);
  assert_eq!(cycle.projects_read, 1, "切り替え周期は新しい設定で読みます");

  let after = harness.store.project_views().remove(0);
  assert_eq!(after.server_id, "mini-two", "切り替え周期の読み取りは新しい設定を使います");
  assert_eq!(after.goals.len(), 1, "切り替え周期の読み取りは新しい server の状態です");
  assert_eq!(after.goals[0].goal_id.as_str(), ADDED_A);
  assert!(after.last_read_at.is_some());
  drop(client);
  grpc.stop().await;
  poller.shutdown().await;
}

/// 停止の合図を、サイクルが読み取りを終えて記録の直前（`BeforeApply`）で止まっているときに送る。
/// `resume_after` が `Some` なら、その時間のあとでサイクルを再開させます。`None`
/// なら再開させません。 合図から `run_until_stopped` が戻るまでの時間を返します。
async fn stop_during_a_cycle(poller: &mut Poller, resume_after: Option<Duration>) -> Duration {
  let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
  let (reached, resume) = arm(poller, ReadBoundary::BeforeApply);
  let control = async move {
    reached.await.expect("サイクルが記録の直前まで進みます");
    let stopped_at = tokio::time::Instant::now();
    stop_tx.send(true).expect("停止の合図を送れます");
    let kept = match resume_after {
      | Some(delay) => {
        tokio::time::sleep(delay).await;
        resume.send(()).expect("サイクルを再開できます");
        None
      },
      // 再開させないまま持ち続けます。サイクルが打ち切られれば、待っている側は消えます。
      | None => Some(resume),
    };
    (stopped_at, kept, stop_tx)
  };
  let ((), (stopped_at, kept, _stop_tx)) =
    timeout(Duration::from_secs(10), async { tokio::join!(super::run_until_stopped(poller, stop_rx), control) })
      .await
      .expect("run_until_stopped は停止の合図から上限までに戻ります");
  let elapsed = stopped_at.elapsed();
  drop(kept);
  elapsed
}

/// プロジェクトのゴールの追加として記録された出来事の、ゴール ID ごとの件数。
fn created_goal_counts(harness: &GrpcHarness, ids: &[&str]) -> Vec<usize> {
  let events = harness.activity.recent_events(50).expect("SQLite の出来事を読めます");
  ids
    .iter()
    .map(|id| events.iter().filter(|event| event.kind == EventKind::GoalCreated && event.body.contains(id)).count())
    .collect()
}

#[tokio::test]
async fn a_stop_signal_during_a_long_cycle_cancels_it_and_a_restart_records_each_event_once() {
  // 停止の合図のあと、サイクルが CYCLE_GRACE を過ぎても終わらなければ打ち切る。打ち切った
  // サイクルは記録の前で止まり、出来事を半端に残さない。次の起動は基準化から始まり、同じ出来事を
  // 二重に記録しない。
  let harness = harness();
  harness.rewrite_state(json!({"goals": [goal(GOAL, "active")]}));
  let mut first = poller(&harness);
  assert_eq!(first.read_cycle_once().await.unwrap().events_recorded, 0);
  harness.rewrite_state(json!({"goals": [goal(GOAL, "active"), goal(ADDED_A, "active")]}));

  let elapsed = stop_during_a_cycle(&mut first, None).await;

  assert!(elapsed >= super::CYCLE_GRACE, "合図のあと CYCLE_GRACE はサイクルを待ちます（{elapsed:?}）");
  assert!(elapsed < super::CYCLE_GRACE + Duration::from_secs(5), "サイクルの再開を待たずに戻ります（{elapsed:?}）");
  assert_eq!(created_goal_counts(&harness, &[ADDED_A]), vec![0], "打ち切ったサイクルは記録の前で止まります");

  // 次の起動。新しい Poller の最初のサイクルは基準化だけで、出来事を記録しません。
  let mut restarted = poller(&harness);
  assert_eq!(restarted.read_cycle_once().await.unwrap().events_recorded, 0, "再起動で出来事を二重に記録しません");
  harness.rewrite_state(json!({"goals": [goal(GOAL, "active"), goal(ADDED_A, "active"), goal(ADDED_B, "active")]}));
  assert_eq!(restarted.read_cycle_once().await.unwrap().events_recorded, 1);
  // 打ち切ったサイクルで見つけたはずの ADDED_A は記録されない。これは、サイクルの間に backend を
  // 止めたとき（SIGKILL
  // を含む）と同じで、再起動のあとの最初のサイクルが基準化だけであることによる。
  assert_eq!(created_goal_counts(&harness, &[ADDED_A, ADDED_B]), vec![0, 1]);
  restarted.shutdown().await;
}

#[tokio::test]
async fn a_stop_signal_during_a_short_cycle_lets_it_record_before_stopping() {
  // 停止の合図のあと CYCLE_GRACE の間に終わるサイクルは、最後まで走らせ、その出来事を 1 件だけ
  // 記録してから止まる。次の起動で同じ出来事を記録し直さない。
  let harness = harness();
  harness.rewrite_state(json!({"goals": [goal(GOAL, "active")]}));
  let mut first = poller(&harness);
  assert_eq!(first.read_cycle_once().await.unwrap().events_recorded, 0);
  harness.rewrite_state(json!({"goals": [goal(GOAL, "active"), goal(ADDED_A, "active")]}));

  stop_during_a_cycle(&mut first, Some(Duration::from_millis(200))).await;

  assert_eq!(created_goal_counts(&harness, &[ADDED_A]), vec![1], "合図のあとに終わったサイクルの出来事を記録します");
  let mut restarted = poller(&harness);
  assert_eq!(restarted.read_cycle_once().await.unwrap().events_recorded, 0);
  assert_eq!(created_goal_counts(&harness, &[ADDED_A]), vec![1], "再起動で同じ出来事を二重に記録しません");
  restarted.shutdown().await;
}

#[tokio::test]
async fn a_cancelled_cycle_stops_its_http_read_so_the_session_is_closed() {
  // 停止の合図のあと CYCLE_GRACE で周期を打ち切ると、周期が起動した HTTP の読み取りタスクも
  // 中断する。読み取りハンドルの鍵が空き、停止の段が応答するサーバーのセッションを DELETE
  // で閉じる。 タスクを中断しないと、遅い道具の呼び出しが終わるまで鍵が空かず、停止がそれを待つ。
  let harness = harness();
  let http_state = harness.project_dir().join("http-state.json");
  std::fs::write(
    &http_state,
    json!({"goals": [goal(GOAL, "active")], "delayMs": {"takt_list_goals": 10_000}}).to_string(),
  )
  .unwrap();
  let http = common::HttpMockServer::start(&http_state, None);
  let mut config: Value = serde_json::from_str(&std::fs::read_to_string(harness.config_path()).unwrap()).unwrap();
  config["mcpServers"]["mac-mini"] = json!({
    "type": "http",
    "url": http.url(),
    "headers": { "Authorization": format!("Bearer {}", http.token()) },
  });
  std::fs::write(harness.config_path(), config.to_string()).unwrap();
  let mut poller = poller(&harness);
  let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
  let control = async {
    // 読み取りが遅い道具の呼び出しに入るまで待ちます。
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !http.recorded_calls().iter().any(|call| call["name"] == "takt_list_goals") {
      assert!(tokio::time::Instant::now() < deadline, "読み取りが道具を呼びます");
      tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(http.live_session_count(), 1, "読み取りのセッションが生きています");
    let stopped_at = tokio::time::Instant::now();
    stop_tx.send(true).unwrap();
    stopped_at
  };
  let ((), stopped_at) =
    timeout(Duration::from_secs(30), async { tokio::join!(super::run_until_stopped(&mut poller, stop_rx), control) })
      .await
      .expect("run_until_stopped は戻ります");
  let elapsed = stopped_at.elapsed();

  assert!(
    elapsed < super::CYCLE_GRACE + Duration::from_secs(3),
    "遅い道具の呼び出しの終わりを待たずに止まります（{elapsed:?}）"
  );
  assert_eq!(http.live_session_count(), 0, "読み取りのセッションを閉じます: {:?}", http.session_events());
}

/// 起動設定（command、args、env、cwd）を変えずに server ID だけを変える（issue #27）。
#[tokio::test]
async fn issue27_switch_read_and_write() {
  issue27(true).await;
}

#[tokio::test]
async fn issue27_switch_read_only() {
  issue27(false).await;
}

async fn issue27(switch_write: bool) {
  let harness = harness();
  harness.rewrite_state(json!({"goals": [goal(GOAL, "active")]}));
  let mut poller = poller(&harness);
  assert_eq!(poller.read_cycle_once().await.unwrap().projects_read, 1);
  let old_read = harness.registry.take_for_read("mac-mini", "takt-dash", "mac-mini").expect("旧 poller があります");
  let old_client = old_read.client.clone();
  drop(old_read);
  let initial = load_config(harness.config_path()).unwrap();
  let McpServer::Stdio(old_server) = &initial.servers["mac-mini"] else {
    panic!("stdio です");
  };
  let old_launch = client_config(old_server, &initial.projects[0].cwd);
  let (old_write, created) = harness.registry.client("mac-mini", "takt-dash", Lane::Write, || old_launch.clone());
  assert!(!created, "切り替え前の書き込みレーンは周期で用意済みです");

  let mut config: Value = serde_json::from_str(&std::fs::read_to_string(harness.config_path()).unwrap()).unwrap();
  config["mcpServers"]["mini-two"] = config["mcpServers"]["mac-mini"].clone();
  config["servers"]["mini-two"] = config["servers"]["mac-mini"].clone();
  config["projects"][0]["server"] = json!("mini-two");
  if switch_write {
    config["projects"][0]["writeServer"] = json!("mini-two");
  }
  std::fs::write(harness.config_path(), config.to_string()).unwrap();
  let loaded = load_config(harness.config_path()).unwrap();
  let (McpServer::Stdio(a), McpServer::Stdio(b)) = (&loaded.servers["mac-mini"], &loaded.servers["mini-two"]) else {
    panic!("stdio です");
  };
  let cwd = &loaded.projects[0].cwd;
  assert_eq!(client_config(a, cwd), client_config(b, cwd), "起動設定は同一です");

  // 切り替え周期の読み取りは失敗させる。
  harness.rewrite_state(json!({"isError": ["takt_list_goals"]}));
  let report = poller.read_cycle_once().await.unwrap();
  assert_eq!(report.projects_failed, 1);
  let view = harness.store.project_views().remove(0);
  assert_eq!(view.server_id, "mini-two");
  assert!(view.snapshot.is_none(), "読み取りに失敗しても旧 snapshot を残しません");
  assert!(view.last_read_at.is_none());
  assert!(view.goals.is_empty());
  assert_eq!(
    harness.registry.take_for_read("mac-mini", "takt-dash", "mac-mini").is_none(),
    switch_write,
    "書き込み先にも使わなくなった旧 server のレーンは外れ、書き込み先に残る旧 server のレーンは残ります"
  );
  let new_read = harness.registry.take_for_read("mini-two", "takt-dash", "mini-two").expect("新 poller があります");
  assert!(!Arc::ptr_eq(&old_client, &new_read.client), "旧クライアントを再利用しません");
  drop(new_read);
  // 旧 server の書き込みレーンは、書き込み先が残るなら同じクライアントのまま残り、書き込み先も
  // 切り替えたなら外れています（外れていれば、ここで新しく作られます）。
  let (write_after, write_created) =
    harness.registry.client("mac-mini", "takt-dash", Lane::Write, || old_launch.clone());
  assert_eq!(write_created, switch_write, "旧 server の書き込みレーンは、書き込み先に使う間だけ残ります");
  assert_eq!(Arc::ptr_eq(&old_write, &write_after), !switch_write, "残した書き込みレーンは同じクライアントです");
  drop((old_write, write_after));

  // 回復した最初の成功は基準化だけで、旧基準との差分を出来事にしません。
  harness.rewrite_state(json!({"goals": [goal(GOAL, "active"), goal(ADDED_A, "active")]}));
  let recovered = poller.read_cycle_once().await.unwrap();
  assert_eq!(recovered.projects_read, 1);
  assert_eq!(recovered.events_recorded, 0, "旧 server の差分基準を使いません");
  poller.shutdown().await;
}

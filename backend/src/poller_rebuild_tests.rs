//! 実 RPC による再構築と、読取周期の世代対応を検証する。

#[path = "../tests/common/mod.rs"]
mod common;
#[path = "../tests/grpc_harness/mod.rs"]
mod grpc_harness;

use std::path::Path;

use common::{project, running_meta, server_meta, RunFixture, RunTaskLine};
use grpc_harness::{client, start_server, GrpcHarness};
use serde_json::{json, Value};
use tokio::{
  sync::oneshot,
  time::{timeout, Duration},
};

use super::Poller;
use crate::{
  model::{EventKind, ExecutionStatus, ProjectSnapshot},
  takt_dash::v1::ApproveGoalRegistrationRequest,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReadBoundary {
  BeforeAcquisition,
  AfterAcquisition,
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
  harness.rewrite_state(a.clone());
  let after_acquisition = boundary == ReadBoundary::AfterAcquisition;
  let mut b = if after_acquisition { state(cwd_a, cwd_b, "paused") } else { a };
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
    if after_acquisition {
      let args = config["mcpServers"]["mac-mini"]["args"].as_array_mut().expect("起動引数があります");
      let index = args.iter().position(|arg| arg == "--state").expect("--state があります");
      args[index + 1] = json!(b_state);
    }
    std::fs::write(harness.config_path(), config.to_string()).expect("B の設定を書けます");
    let response = client
      .approve_goal_registration(ApproveGoalRegistrationRequest {
        server_id: "mac-mini".to_owned(),
        project:   "takt-dash".to_owned(),
        revision:  3,
      })
      .await
      .expect("実 RPC が B を登録して成功します")
      .into_inner();
    assert_eq!(response.revision, 3);
    resume.send(()).expect("対象周期へ再開許可を渡せます");
  };
  let (interrupted, ()) = timeout(Duration::from_secs(15), async { tokio::join!(poller.read_cycle_once(), control) })
    .await
    .expect("到達、RPC 成功、再開の順で完了します");
  let interrupted = interrupted.expect("割込み周期が成功します");
  assert_eq!(interrupted.projects_read, 1);
  assert_eq!(interrupted.projects_failed, 0);

  let expected_cwd = if after_acquisition { cwd_a } else { cwd_b };
  let reads: Vec<_> = harness
    .recorded_calls()
    .into_iter()
    .skip(calls_before)
    .filter(|call| call.name == "takt_list_goals" || call.name == "takt_list_goal_registration_proposals")
    .collect();
  assert_eq!(reads.len(), 2, "割込み周期の道具呼出しだけを検査します");
  if after_acquisition {
    assert_snapshot(&harness, ExecutionStatus::Active, 3, "plan");
    assert_eq!(snapshot(&harness).goals.len(), 2);
    assert_eq!(interrupted.events_recorded, 1, "A の比較基準を保持しています");
    assert_events(&harness, &[ADDED_A]);
    let first_b = poller.read_cycle_once().await.expect("同じ Poller で B 初回を読めます");
    assert_eq!(first_b.projects_read, 1);
    assert_eq!(first_b.events_recorded, 0, "B 初回は基準化のみです");
    assert_snapshot(&harness, ExecutionStatus::Paused, 4, "implement");
    assert_eq!(snapshot(&harness).goals.len(), 1);
    assert_events(&harness, &[ADDED_A]);
  } else {
    assert_eq!(interrupted.events_recorded, 0, "割込み周期が B 初回なので基準化のみです");
    assert_snapshot(&harness, ExecutionStatus::Active, 4, "implement");
    assert_eq!(snapshot(&harness).goals.len(), 2);
    assert_events(&harness, &[]);
  }
  for read in reads {
    assert_eq!(read.arguments["cwd"], json!(expected_cwd), "{} の読取先が世代に対応します", read.name);
  }

  b["goals"].as_array_mut().expect("ゴール一覧があります").push(goal(ADDED_B, "active"));
  if after_acquisition {
    std::fs::write(&b_state, b.to_string()).expect("B 内の実変化を書けます");
  } else {
    harness.rewrite_state(b);
  }
  let next_b = poller.read_cycle_once().await.expect("同じ Poller で B の次周期を読めます");
  assert_eq!(next_b.projects_read, 1);
  assert_eq!(next_b.events_recorded, 1, "B 内の実変化を1件だけ記録します");
  let execution = if after_acquisition { ExecutionStatus::Paused } else { ExecutionStatus::Active };
  assert_snapshot(&harness, execution, 4, "implement");
  assert!(snapshot(&harness).goals.iter().any(|goal| goal.goal_id.as_str() == ADDED_B));
  let created = if after_acquisition { vec![ADDED_A, ADDED_B] } else { vec![ADDED_B] };
  assert_events(&harness, &created);
  poller.shutdown().await;
  drop(client);
  server.stop().await;
}

#[tokio::test]
async fn rpc_rebuild_before_acquisition_uses_the_registered_cwd() {
  rebuild_through_rpc(ReadBoundary::BeforeAcquisition).await;
}

#[tokio::test]
async fn rpc_rebuild_after_acquisition_keeps_the_same_poller_baseline() {
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

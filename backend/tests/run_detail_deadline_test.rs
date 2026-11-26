//! GetRunDetail の回り道（保存場所から読めない run を takt_get_run で読む経路）の期限を検証する
//! （issue #59 と #180、設計は `docs/design/issue-59-180.md`）。
//!
//! - 要求が打ち切られても、stdio の画面読み取りの起動・初期化・道具の呼び出しは最後まで進み、
//!   子プロセスは次の要求で使える状態で残る。
//! - GetRunDetail は backend の総上限で DEADLINE_EXCEEDED を返し、総上限の中なら遅くても成功する。

mod common;
mod grpc_harness;

use std::time::{Duration, Instant};

use common::*;
use grpc_harness::{client, start_server, start_server_with_run_detail_budget, GrpcHarness};
use serde_json::{json, Value};
use takt_dash_backend::{
  domain::GoalId,
  model::{ExecutionStatus, GoalStatus, ProjectSnapshot},
  takt_dash::v1::{GetRunDetailRequest, RunStatus},
};

fn project_dir() -> &'static std::path::Path {
  std::path::Path::new("/abs/takt-dash")
}

/// takt_get_run が run-1 を返す mock の状態。ローカルには tasks.yaml が無いので、GetRunDetail は
/// 回り道（takt_get_run）で読む。
fn remote_run_state() -> Value {
  json!({
    "tasks": [],
    "runs": {
      "run-1": {
        "runSlug": "run-1",
        "status": "running",
        "taskText": "リモートのタスク本文",
        "workflow": "remote-workflow",
        "currentStep": "remote-implement",
        "startedAt": "2026-11-13T09:00:00Z",
        "stepLogs": [
          { "step": "remote-plan", "persona": "architect", "status": "done", "timestamp": "2026-11-13T09:10:00Z", "iteration": 1 }
        ],
        "instructions": []
      }
    }
  })
}

/// 状態ストアに run-1 を持つ 1 プロジェクト分の土台を作る。
fn seeded_harness(state: Value) -> GrpcHarness {
  let harness =
    GrpcHarness::builder().single_project(project("takt-dash", project_dir()), server_meta(), state).build();
  let mut stored_run = run("run-1", "implement", time("2026-11-13T09:06:00Z"));
  stored_run.goal_id = Some(GoalId::try_from("g1").expect("テストのゴール ID は有効です"));
  let snapshot = ProjectSnapshot {
    goals: vec![goal("g1", GoalStatus::Created, ExecutionStatus::Active)],
    runs: vec![stored_run],
    ..ProjectSnapshot::empty()
  };
  harness.store.record_success("takt-dash", snapshot, time("2026-11-13T12:00:00Z"));
  harness
}

/// Web の deadline（grpc-timeout）を付けた要求。tonic のサーバーは期限で要求の future を捨てる。
fn request_with_deadline(deadline: Option<Duration>) -> tonic::Request<GetRunDetailRequest> {
  let mut request = tonic::Request::new(GetRunDetailRequest { run_slug: "run-1".to_owned() });
  if let Some(deadline) = deadline {
    request.set_timeout(deadline);
  }
  request
}

/// mock の起動の記録に、初期化を終えた（ready）子プロセスが現れるまで待つ。テストのランタイムは
/// 1 本のスレッドなので、切り離した読み取りのタスクが進むよう、非同期に待つ。
async fn wait_for_ready(spawn_log: &std::path::Path, within: Duration) -> bool {
  let started = Instant::now();
  while started.elapsed() < within {
    let text = std::fs::read_to_string(spawn_log).unwrap_or_default();
    if text.lines().any(|line| line.starts_with("ready ")) {
      return true;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
  }
  false
}

/// 道具の呼び出しの記録が `count` 件に達するまで待つ。
async fn wait_for_calls(harness: &GrpcHarness, count: usize, within: Duration) -> bool {
  let started = Instant::now();
  while started.elapsed() < within {
    if harness.recorded_calls().len() >= count {
      return true;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
  }
  false
}

#[tokio::test]
async fn a_run_detail_cancelled_during_initialize_leaves_the_child_ready_for_the_next_request() {
  // issue #180：初期化が Web の deadline
  // より長い。1回目の要求は打ち切られるが、初期化は最後まで進み、 2回目の要求は起動し直さずに読む。
  // 修正前は、打ち切りで初期化中の子プロセスを捨て、 2回目も起動からやり直す（初期化の 1.5 秒が 2
  // 回目の deadline 1 秒に間に合わない）。 mock の起動の記録の ready は、node
  // を包む起動用のスクリプトだけが止まった場合にも残るので、 子プロセスを使い回したことは、
  // 起動の回数（mock の pid の記録）で確かめる。
  let harness = seeded_harness(remote_run_state());
  let spawn_log = harness.home().join("spawn.log");
  harness.append_mock_args(&["--initialize-delay-ms", "1500", "--spawn-log", &spawn_log.display().to_string()]);
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let first = client.get_run_detail(request_with_deadline(Some(Duration::from_millis(500)))).await;
  assert!(first.is_err(), "初期化を待つ間に deadline を過ぎた要求は失敗します: {first:?}");
  assert!(wait_for_ready(&spawn_log, Duration::from_secs(5)).await, "打ち切られた要求の初期化も最後まで進みます");
  let second = client
    .get_run_detail(request_with_deadline(Some(Duration::from_secs(1))))
    .await
    .expect("初期化を終えた子プロセスで、次の要求は deadline 内に読めます")
    .into_inner();
  server.stop().await;

  assert_eq!(second.workflow, "remote-workflow", "takt_get_run の値で返します");
  assert_eq!(second.status, RunStatus::Running as i32);
  assert_eq!(harness.recorded_processes().len(), 1, "子プロセスの起動は 1 回だけです");
}

#[tokio::test]
async fn a_run_detail_cancelled_during_a_tool_call_keeps_the_child() {
  // 呼び出しの途中で打ち切られても、読み取りは最後まで進み、子プロセスを捨てない。修正前は、
  // 読みかけの子プロセスを捨てて、次の要求が起動し直す。
  let harness = seeded_harness(remote_run_state());
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;
  client.get_run_detail(request_with_deadline(None)).await.expect("最初の要求で子プロセスを起動して読めます");
  let calls_before = harness.recorded_calls().len();

  let mut slow = remote_run_state();
  slow["delays"] = json!({ "takt_get_run": 1500 });
  harness.rewrite_state(slow);
  let cancelled = client.get_run_detail(request_with_deadline(Some(Duration::from_millis(500)))).await;
  assert!(cancelled.is_err(), "呼び出しの途中で deadline を過ぎた要求は失敗します: {cancelled:?}");
  harness.rewrite_state(remote_run_state());
  // 打ち切られた要求の読み取り（takt_get_run の本体、view instructions、view task）は最後まで進む。
  assert!(
    wait_for_calls(&harness, calls_before + 3, Duration::from_secs(10)).await,
    "打ち切られた要求の読み取りも最後まで進みます: {:?}",
    harness.recorded_calls()
  );
  let detail = client
    .get_run_detail(request_with_deadline(Some(Duration::from_secs(3))))
    .await
    .expect("次の要求は同じ子プロセスで読めます")
    .into_inner();
  server.stop().await;

  assert_eq!(detail.workflow, "remote-workflow");
  assert_eq!(harness.recorded_processes().len(), 1, "子プロセスを捨てて起動し直しません");
}

#[tokio::test]
async fn get_run_detail_returns_deadline_exceeded_when_the_budget_runs_out() {
  // issue #59：backend の総上限で DEADLINE_EXCEEDED を返す。Web の deadline は付けない。
  let mut state = remote_run_state();
  state["delays"] = json!({ "takt_get_run": 1500 });
  let harness = seeded_harness(state);
  let server = start_server_with_run_detail_budget(&harness, Duration::from_millis(500)).await;
  let mut client = client(server.address).await;

  let started = Instant::now();
  let status = client.get_run_detail(request_with_deadline(None)).await.expect_err("総上限を超えた要求は失敗します");
  let elapsed = started.elapsed();
  server.stop().await;

  assert_eq!(status.code(), tonic::Code::DeadlineExceeded, "総上限の超過は DEADLINE_EXCEEDED です: {status:?}");
  assert!(status.message().contains("run-1"), "説明に run を書きます: {}", status.message());
  assert!(elapsed < Duration::from_millis(1500), "総上限で返り、遅い呼び出しを待ちません: {elapsed:?}");
}

#[tokio::test]
async fn get_run_detail_within_the_budget_succeeds_even_when_slow() {
  // issue #59：呼び出しが合わせて遅くても、総上限の中なら成功する。
  let mut state = remote_run_state();
  state["delays"] = json!({ "takt_get_run": 400 });
  let harness = seeded_harness(state);
  let server = start_server_with_run_detail_budget(&harness, Duration::from_secs(5)).await;
  let mut client = client(server.address).await;

  let detail = client.get_run_detail(request_with_deadline(None)).await.expect("総上限の中なら成功します").into_inner();
  server.stop().await;

  assert_eq!(detail.workflow, "remote-workflow");
}

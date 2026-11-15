//! gRPC の WatchState が接続時に即時に全件を返し、その後の定期読み取りで状態が変わったときに
//! 新しい DashboardState
//! を同じストリームへ送り、サーバーを止めるとストリームが終わることを検証する。

mod common;
mod grpc_harness;

use std::path::Path;

use common::*;
use grpc_harness::{client, start_server, GrpcHarness};
use serde_json::json;
use takt_dash_backend::{
  config::Thresholds,
  model::{ExecutionStatus, GoalStatus, ProjectSnapshot},
  poller::Poller,
  takt_dash::v1::WatchStateRequest,
};
use tokio::time::{timeout, Duration};

fn project_dir() -> &'static Path {
  Path::new("/abs/takt-dash")
}

fn goal_state(goal_id: &str) -> serde_json::Value {
  json!({
    "goals": [ { "id": goal_id, "status": "created", "executionStatus": "active", "objective": format!("ゴール {goal_id}"), "integrationBranch": format!("takt/{goal_id}") } ]
  })
}

/// ストアに現在の状態を入れ、poller を繋いだ土台を作る。
fn seeded_harness() -> GrpcHarness {
  let harness =
    GrpcHarness::builder().single_project(project("takt-dash", project_dir()), server_meta(), goal_state("g1")).build();
  let snapshot = ProjectSnapshot {
    goals:         vec![goal("g1", GoalStatus::Created, ExecutionStatus::Active)],
    tasks:         vec![],
    inquiries:     vec![],
    runs:          vec![],
    notifications: vec![],
    proposals:     vec![],
  };
  harness.store.record_success("takt-dash", snapshot, time("2026-11-13T12:00:00Z"));
  harness
}

#[tokio::test]
async fn watch_state_sends_the_current_state_immediately_then_follows_changes_on_the_same_stream() {
  let harness = seeded_harness();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let mut stream =
    client.watch_state(WatchStateRequest {}).await.expect("状態ストリーミングへ接続できます").into_inner();

  // 接続直後に、現在の全件を即時に返す。
  let first = timeout(Duration::from_secs(5), stream.message())
    .await
    .expect("初回の全件は即時に届きます")
    .expect("ストリームから読めます")
    .expect("初回の状態があります");
  assert_eq!(first.goals.len(), 1, "初回は現在の全件を返します");
  assert_eq!(first.goals[0].goal_id, "g1");

  // 同じストリームを開いたまま、定期読み取りで状態を変える。
  let mut poller = Poller::new(
    harness.config_path().to_path_buf(),
    harness.activity.clone(),
    harness.store.clone(),
    harness.changes.clone(),
  );
  harness.rewrite_state(goal_state("g2"));
  poller.read_cycle_once().await.expect("状態を変えるサイクルを実行できます");

  let second = timeout(Duration::from_secs(5), stream.message())
    .await
    .expect("状態が変われば新しい状態が届きます")
    .expect("ストリームから読めます")
    .expect("変化後の状態があります");
  let goal_ids: Vec<&str> = second.goals.iter().map(|goal| goal.goal_id.as_str()).collect();
  assert_eq!(goal_ids, vec!["g2"], "同じストリームで変化後の状態へ追従します");

  poller.shutdown().await;
  server.stop().await;
}

#[tokio::test]
async fn watch_state_does_not_resend_when_a_cycle_leaves_the_state_unchanged() {
  let harness = seeded_harness();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let mut stream =
    client.watch_state(WatchStateRequest {}).await.expect("状態ストリーミングへ接続できます").into_inner();
  let _first = timeout(Duration::from_secs(5), stream.message())
    .await
    .expect("初回の全件は即時に届きます")
    .expect("ストリームから読めます")
    .expect("初回の状態があります");

  // 状態を変えないサイクルを回す。
  let mut poller = Poller::new(
    harness.config_path().to_path_buf(),
    harness.activity.clone(),
    harness.store.clone(),
    harness.changes.clone(),
  );
  poller.read_cycle_once().await.expect("状態を変えないサイクルを実行できます");

  let unsent = timeout(Duration::from_millis(500), stream.message()).await;
  assert!(unsent.is_err(), "状態が変わっていないサイクルでは何も送りません: {unsent:?}");

  poller.shutdown().await;
  server.stop().await;
}

#[tokio::test]
async fn watch_state_stream_ends_when_the_server_stops() {
  let harness = seeded_harness();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let mut stream =
    client.watch_state(WatchStateRequest {}).await.expect("状態ストリーミングへ接続できます").into_inner();
  let _first = timeout(Duration::from_secs(5), stream.message())
    .await
    .expect("初回の全件は即時に届きます")
    .expect("ストリームから読めます")
    .expect("初回の状態があります");

  server.stop().await;

  // サーバーを止めるとストリームは終わり、それ以上の状態は届きません。
  let ended = timeout(Duration::from_secs(5), stream.message()).await.expect("ストリームは終わります");
  match ended {
    | Ok(None) => {},
    | Err(status) => panic!("サーバー停止でストリームは終わる必要がありますが、エラーが届きました: {status:?}"),
    | Ok(Some(state)) => panic!("サーバー停止後に状態が届きました: {state:?}"),
  }

  // クライアント側も終了したことを観測する。以降の受信は None です。
  let after = timeout(Duration::from_secs(5), stream.message()).await;
  if let Ok(Ok(Some(state))) = after {
    panic!("終端後に状態が届きました: {state:?}");
  }
}

#[tokio::test]
async fn watch_state_sends_the_current_thresholds_after_a_config_change() {
  // 同じ接続を開いたまま設定の閾値を書き換え、変化通知後の第2メッセージの thresholds が
  // 変更後の値になることを観測する。接続時にキャプチャした旧値を再利用すると失敗する。
  let harness = seeded_harness();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let mut stream =
    client.watch_state(WatchStateRequest {}).await.expect("状態ストリーミングへ接続できます").into_inner();
  let first = timeout(Duration::from_secs(5), stream.message())
    .await
    .expect("初回の全件は即時に届きます")
    .expect("ストリームから読めます")
    .expect("初回の状態があります");
  assert_eq!(
    first.thresholds.expect("閾値があります").pending_decision_stale_minutes,
    240,
    "初回は接続時の閾値を返します"
  );

  // 設定の閾値を滞留 1 分・無音 3 分へ書き換え、定期読み取りの1サイクルを回す。
  harness.rewrite_thresholds(Thresholds { pending_decision_stale_minutes: 1, run_log_silent_minutes: 3 });
  let mut poller = Poller::new(
    harness.config_path().to_path_buf(),
    harness.activity.clone(),
    harness.store.clone(),
    harness.changes.clone(),
  );
  poller.read_cycle_once().await.expect("閾値を変えるサイクルを実行できます");

  let second = timeout(Duration::from_secs(5), stream.message())
    .await
    .expect("閾値が変われば新しい状態が届きます")
    .expect("ストリームから読めます")
    .expect("変化後の状態があります");
  let thresholds = second.thresholds.expect("閾値があります");
  assert_eq!(
    thresholds.pending_decision_stale_minutes, 1,
    "変化通知後の閾値は変更後の値を返します（接続時の旧値ではありません）"
  );
  assert_eq!(thresholds.run_log_silent_minutes, 3, "無音の閾値も変更後に追随します");

  poller.shutdown().await;
  server.stop().await;
}

#[tokio::test]
async fn watch_state_task_ends_when_the_client_disconnects() {
  // クライアント側のストリームを drop すると、送信タスクは次の送信の失敗で終了し、
  // 変化通知チャネルの購読が解除される。receiver_count の復帰で観測する。
  let harness = seeded_harness();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let mut stream =
    client.watch_state(WatchStateRequest {}).await.expect("状態ストリーミングへ接続できます").into_inner();
  let _first = timeout(Duration::from_secs(5), stream.message())
    .await
    .expect("初回の全件は即時に届きます")
    .expect("ストリームから読めます")
    .expect("初回の状態があります");
  assert_eq!(harness.change_receiver_count(), 1, "接続中は購読者が 1 人います");

  // クライアントのストリームを drop して切断する。
  drop(stream);

  // 送信タスクが次の送信で終了するよう変化通知を起こし、購読解除を待つ。
  let mut ended = false;
  for _ in 0..50 {
    let _ = harness.changes.send(());
    tokio::time::sleep(Duration::from_millis(100)).await;
    if harness.change_receiver_count() == 0 {
      ended = true;
      break;
    }
  }
  assert!(ended, "クライアント切断後に WatchState の送信タスクが終了し購読が解除されます");

  server.stop().await;
}

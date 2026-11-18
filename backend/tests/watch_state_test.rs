//! gRPC の WatchState が接続時に即時に全件を返し、その後の定期読み取りで状態が変わったときに
//! 新しい DashboardState
//! を同じストリームへ送り、サーバーを止めるとストリームが終わることを検証する。

mod common;
mod grpc_harness;

use std::path::Path;

use chrono::Utc;
use common::*;
use grpc_harness::{client, start_server, GrpcHarness};
use serde_json::json;
use takt_dash_backend::{
  config::{ServerMeta, Thresholds},
  domain::{SilentMinutes, StaleMinutes},
  model::{ExecutionStatus, GoalStatus, ProjectSnapshot},
  poller::Poller,
  takt_dash::v1::WatchStateRequest,
};
use tokio::time::{timeout, Duration};
use tonic::Request;

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
    goals: vec![goal("g1", GoalStatus::Created, ExecutionStatus::Active)],
    tasks: vec![],
    inquiries: vec![],
    runs: vec![],
    notifications: vec![],
    proposals: vec![],
    ..ProjectSnapshot::empty()
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
    harness.registry.clone(),
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
    harness.registry.clone(),
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
  harness.rewrite_thresholds(Thresholds {
    pending_decision_stale_minutes: StaleMinutes::try_from(1).expect("テストの閾値は有効です"),
    run_log_silent_minutes:         SilentMinutes::try_from(3).expect("テストの閾値は有効です"),
    connection_failure_count:       1,
    connection_delay_seconds:       3,
  });
  let mut poller = Poller::new(
    harness.config_path().to_path_buf(),
    harness.activity.clone(),
    harness.store.clone(),
    harness.changes.clone(),
    harness.registry.clone(),
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
async fn watch_state_sends_the_current_server_meta_after_a_config_change() {
  // 同じ接続を開いたまま設定のサーバー表示情報を書き換え、変化通知後の第2メッセージの servers が
  // 変更後の値になることを観測する。起動時だけ server_meta を読み込む実装では、
  // 変化通知そのものが届かず、旧値のままになる。
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
  assert_eq!(first.servers[0].name, "j5ik2o-mac-mini", "初回は接続時のサーバー表示情報を返します");

  // 設定のサーバー表示情報を書き換え、定期読み取りの1サイクルを回す。
  harness
    .rewrite_server_meta("mac-mini", ServerMeta { name: "renamed-mini".to_owned(), kind: "Mac Studio".to_owned() });
  let mut poller = Poller::new(
    harness.config_path().to_path_buf(),
    harness.activity.clone(),
    harness.store.clone(),
    harness.changes.clone(),
    harness.registry.clone(),
  );
  poller.read_cycle_once().await.expect("サーバー表示情報を変えるサイクルを実行できます");

  let second = timeout(Duration::from_secs(5), stream.message())
    .await
    .expect("サーバー表示情報が変われば新しい状態が届きます")
    .expect("ストリームから読めます")
    .expect("変化後の状態があります");
  assert_eq!(second.servers.len(), 1, "設定にあるサーバーだけを返します");
  assert_eq!(second.servers[0].name, "renamed-mini", "変化通知後のサーバー名は変更後の値です");
  assert_eq!(second.servers[0].kind, "Mac Studio", "種類も変更後に追随します");

  poller.shutdown().await;
  server.stop().await;
}

#[tokio::test]
async fn watch_state_task_ends_when_the_client_disconnects() {
  // クライアント側のストリームを drop すると、送信タスクは変化通知や停止通知を待たずに
  // 購読を解除する。変化通知を送らずに receiver_count の復帰を観測する。切断を検知せず
  // 次の変化通知まで残る実装では失敗する。
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

  // クライアントのストリームを drop して切断する。変化通知は送らない。
  drop(stream);

  // 送信タスクが自分で切断を検知して終了するのを待つ。
  let mut ended = false;
  for _ in 0..50 {
    tokio::time::sleep(Duration::from_millis(100)).await;
    if harness.change_receiver_count() == 0 {
      ended = true;
      break;
    }
  }
  assert!(ended, "変化通知を送らなくても、クライアント切断で WatchState の送信タスクが終了し購読が解除されます");

  server.stop().await;
}

#[tokio::test]
async fn watch_state_task_ends_when_the_server_stops_while_a_send_is_waiting() {
  // 送信のバッファを満たし、状態の送信が相手の受信待ちで詰まっている間にサーバーを止める。
  // 送信の待機中も停止通知を監視する実装では、送信の完了を待たずに送信タスクが終了する。
  // 停止通知を送信の後でしか見ない実装では、購読者が残り続ける。
  use takt_dash_backend::{
    grpc::{ServiceDeps, TaktDashServiceImpl},
    takt_dash::v1::takt_dash_service_server::TaktDashService,
  };
  use tokio::sync::watch;

  let harness = seeded_harness();
  let (stop, stop_rx) = watch::channel(false);
  let conversation = std::sync::Arc::new(takt_dash_backend::conversation::ConversationReader::new(
    takt_dash_backend::conversation::ConversationPaths::from_home(harness.home()),
  ));
  let service = TaktDashServiceImpl::new(ServiceDeps {
    store: harness.store.clone(),
    activity: harness.activity.clone(),
    config_path: harness.config_path().to_path_buf(),
    conversation,
    changes: harness.changes.clone(),
    shutdown: stop_rx,
    registry: harness.registry.clone(),
  });

  let response = service.watch_state(Request::new(WatchStateRequest {})).await.expect("購読を開始できます");
  // クライアントをまだ読まず、送信タスクの送信をバッファで詰まらせる。変化通知を送り続けて
  // 送信が待ちに入るのを確実にする。
  let stream = response.into_inner();
  assert_eq!(harness.change_receiver_count(), 1, "接続中は購読者が 1 人います");
  let mut drained = 0;
  while drained < 64 {
    let _ = harness.changes.send(());
    drained += 1;
    tokio::time::sleep(Duration::from_millis(10)).await;
  }

  // 送信が詰まっている間に停止する。
  let _ = stop.send(true);

  let mut ended = false;
  for _ in 0..50 {
    tokio::time::sleep(Duration::from_millis(100)).await;
    if harness.change_receiver_count() == 0 {
      ended = true;
      break;
    }
  }
  drop(stream);
  assert!(ended, "送信の待機中でも停止通知で WatchState の送信タスクが終了し購読が解除されます");
}

#[tokio::test]
async fn watch_state_resends_the_current_state_when_the_deadline_elapses_without_changes() {
  // 状態が変わらなくても、送信から一定時間が経過すると同じストリームへ次の状態が届く。
  // 変化通知だけで送る実装では、期限到達の再配信を観測できないので失敗する。
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

  // 変化通知を送らずに、再配信の期限が来るのを待つ。期限の既定値より十分に長く待つ。
  let resent = timeout(Duration::from_secs(60), stream.message())
    .await
    .expect("期限到達で次の状態が届きます")
    .expect("ストリームから読めます")
    .expect("期限到達の再配信の状態があります");
  assert_eq!(resent.goals.len(), first.goals.len(), "状態が変わらなくても現在の全件を再配信します");
  assert_eq!(resent.goals[0].goal_id, "g1");

  server.stop().await;
}

#[tokio::test]
async fn watch_state_sends_added_server_metadata_after_a_config_change() {
  // サーバーの表示情報だけを設定で変えたとき、同じストリームの第2メッセージの servers が
  // 新しい内容になることを観測する。起動時にキャプチャした servers を返し続ける実装や、
  // 変化指紋が servers を見ない実装では失敗する。
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
  assert_eq!(first.servers.len(), 1, "接続時は設定の1サーバーを返します");
  assert_eq!(first.servers[0].name, "j5ik2o-mac-mini");

  harness.rewrite_servers(|servers| {
    servers["mac-mini"] = json!({ "name": "j5ik2o-mac-mini-renamed", "kind": "Mac mini", "concurrency": 8 });
  });
  let mut poller = Poller::new(
    harness.config_path().to_path_buf(),
    harness.activity.clone(),
    harness.store.clone(),
    harness.changes.clone(),
    harness.registry.clone(),
  );
  poller.read_cycle_once().await.expect("サーバー情報を変えるサイクルを実行できます");

  let second = timeout(Duration::from_secs(5), stream.message())
    .await
    .expect("サーバー情報が変われば新しい状態が届きます")
    .expect("ストリームから読めます")
    .expect("変化後の状態があります");
  assert_eq!(second.servers[0].name, "j5ik2o-mac-mini-renamed", "変化通知後の servers は新しい設定の表示名を返します");

  poller.shutdown().await;
  server.stop().await;
}

/// running なタスクを持つ mock の状態。run 自体は保存場所（`meta.json` と本体ログ）から読む。
fn running_task_state(started_at: &str) -> serde_json::Value {
  json!({
    "tasks": [ { "name": "implement", "status": "running", "runSlug": "run-1", "startedAt": started_at } ]
  })
}

#[tokio::test]
async fn watch_state_reassembles_and_returns_the_new_run_log_silent() {
  // 同じ WatchState ストリームを開いたまま、run の最終ログ（本体ログの更新時刻）を閾値外へ変えて
  // poller を1サイクル回す。変化通知後に再 assemble した状態が新しい無音を返す。
  // 通知時に以前の assemble 結果を再利用する実装では、古い無音のままになる。
  let recent = Utc::now() - chrono::Duration::minutes(5);
  let stale = Utc::now() - chrono::Duration::minutes(300);
  let harness = GrpcHarness::builder()
    .single_project(project("takt-dash", project_dir()), server_meta(), running_task_state(&stale.to_rfc3339()))
    .build();
  let fixture = RunFixture::new(harness.project_dir());
  fixture.write_tasks(&[RunTaskLine { run_slug: Some("run-1"), source_run_slug: None }]);
  fixture.write_meta("run-1", &running_meta("implement", 1, "development-core", &stale.to_rfc3339()));
  fixture.append_records("run-1", &[workflow_start_record("development-core")]);
  let snapshot = ProjectSnapshot {
    goals: vec![],
    tasks: vec![],
    inquiries: vec![],
    runs: vec![run("run-1", "implement", recent)],
    notifications: vec![],
    proposals: vec![],
    ..ProjectSnapshot::empty()
  };
  harness.store.record_success("takt-dash", snapshot, Utc::now());

  let server = start_server(&harness).await;
  let mut client = client(server.address).await;
  let mut stream =
    client.watch_state(WatchStateRequest {}).await.expect("状態ストリーミングへ接続できます").into_inner();

  let first = timeout(Duration::from_secs(5), stream.message())
    .await
    .expect("初回の全件は即時に届きます")
    .expect("ストリームから読めます")
    .expect("初回の状態があります");
  assert_eq!(first.runs.len(), 1);
  assert!(!first.runs[0].log_silent, "5分前なら10分の閾値の内側で無音ではありません");

  // 同じ run の本体ログの更新時刻を閾値外へ変え、同じストリームを開いたまま poller
  // を1サイクル回す。
  fixture.set_body_log_modified("run-1", stale);
  let mut poller = Poller::new(
    harness.config_path().to_path_buf(),
    harness.activity.clone(),
    harness.store.clone(),
    harness.changes.clone(),
    harness.registry.clone(),
  );
  poller.read_cycle_once().await.expect("無音が閾値を跨いだサイクルを実行できます");

  let second = timeout(Duration::from_secs(5), stream.message())
    .await
    .expect("変化通知後の状態が届きます")
    .expect("ストリームから読めます")
    .expect("変化後の状態があります");
  assert_eq!(second.runs.len(), 1);
  assert!(second.runs[0].log_silent, "再 assemble した状態は新しい無音を返します");

  poller.shutdown().await;
  server.stop().await;
}

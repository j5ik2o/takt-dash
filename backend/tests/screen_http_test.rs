//! 画面の要求時の読み取り 3 RPC（GetRunDetail・GetGoalDetail・GetTaskDetail）が、HTTP 接続先の
//! 設定では常駐する takt-mcp の HTTP サーバー経由で返り、stdio の子プロセスを起動しないことを
//! 観測する。既存の詳細テストと同じ状態を使い、読み取り元だけを HTTP に切り替える。

mod common;
mod grpc_harness;

use common::*;
use grpc_harness::{client, start_server, GrpcHarness};
use serde_json::json;
use takt_dash_backend::{
  model::{ExecutionStatus, GoalStatus, ProjectSnapshot},
  takt_dash::v1::{GetGoalDetailRequest, GetRunDetailRequest, GetTaskDetailRequest},
};

fn http_state() -> serde_json::Value {
  json!({
    "goals": [
      { "id": "g1", "status": "created", "executionStatus": "active", "objective": "ゴール g1", "integrationBranch": "takt/integration" },
    ],
    "goalDetails": {
      "g1": {
        "summary": { "id": "g1", "status": "created", "executionStatus": "active" },
        "objective": "ゴール g1 の目的",
        "acceptanceCriteria": ["条件1"],
      },
    },
    "tasks": [
      { "name": "implement", "goalId": "g1", "status": "running", "startedAt": "2026-11-13T09:00:00Z", "runSlug": "run-1", "currentStep": "implement" },
    ],
    "runs": {
      "run-1": { "runSlug": "run-1", "status": "running", "startedAt": "2026-11-13T09:00:00Z", "currentStep": "implement" },
    },
  })
}

fn http_harness() -> GrpcHarness {
  let harness = GrpcHarness::builder()
    .single_project(project("takt-dash", project_dir()), server_meta(), http_state())
    .http_read()
    .build();
  // GetRunDetail は状態ストアに存在する run を要求するため、run を状態ストアへ登録する。
  let snapshot = ProjectSnapshot {
    goals:          vec![goal("g1", GoalStatus::Created, ExecutionStatus::Active)],
    tasks:          vec![],
    inquiries:      vec![],
    runs:           vec![run("run-1", "implement", time("2026-11-13T09:06:00Z"))],
    notifications:  vec![],
    proposals:      vec![],
    terminal_reads: Default::default(),
  };
  harness.store.record_success("takt-dash", snapshot, time("2026-11-13T12:00:00Z"));
  harness
}

fn project_dir() -> &'static std::path::Path {
  std::path::Path::new("/abs/takt-dash")
}

#[tokio::test]
async fn screen_reads_return_over_http_without_spawning_a_child_process() {
  let harness = http_harness();
  // 状態ストアの run は解決済み cwd（実在ディレクトリ）で HTTP へ渡る必要があるため、
  // harness が割り当てた実在の cwd を状態ストアにも投影する。
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let run = client
    .get_run_detail(GetRunDetailRequest { run_slug: "run-1".to_owned() })
    .await
    .expect("GetRunDetail が HTTP 経由で返ります");
  assert_eq!(run.into_inner().run_slug, "run-1");

  let goal = client
    .get_goal_detail(GetGoalDetailRequest { goal_id: "g1".to_owned() })
    .await
    .expect("GetGoalDetail が HTTP 経由で返ります");
  let goal = goal.into_inner();
  assert_eq!(goal.goal_id, "g1");
  assert_eq!(goal.objective, "ゴール g1 の目的", "ゴールの目的が HTTP 経由の本文から返ります");
  assert_eq!(goal.acceptance_criteria, ["条件1".to_owned()], "受け入れ条件が HTTP 経由で返ります");

  let task = client
    .get_task_detail(GetTaskDetailRequest {
      project:   "takt-dash".to_owned(),
      task_name: "implement".to_owned(),
      goal_id:   Some("g1".to_owned()),
    })
    .await
    .expect("GetTaskDetail が HTTP 経由で返ります");
  assert_eq!(task.into_inner().task_name, "implement");

  // 読み取りは常駐 HTTP サーバーへ送られ、stdio の子プロセスは起動しない。
  let http_calls = harness.http_recorded_calls();
  assert!(
    http_calls.iter().any(|call| call["name"] == "takt_get_run"),
    "GetRunDetail は HTTP サーバーへ takt_get_run を送ります: {http_calls:?}"
  );
  assert!(
    http_calls.iter().any(|call| call["name"] == "takt_get_goal"),
    "GetGoalDetail は HTTP サーバーへ takt_get_goal を送ります: {http_calls:?}"
  );
  assert!(
    http_calls.iter().any(|call| call["name"] == "takt_list_tasks"),
    "GetTaskDetail は HTTP サーバーへ takt_list_tasks を送ります: {http_calls:?}"
  );
  assert!(harness.recorded_processes().is_empty(), "画面読み取りは stdio の子プロセスを起動しません");

  server.stop().await;
}

#[tokio::test]
async fn repeated_screen_reads_do_not_accumulate_sessions_on_the_http_server() {
  // 画面の読み取りを繰り返しても、takt-mcp の HTTP サーバーに生きているセッションは増えない
  // （issue #310）。takt-mcp はセッションに期限を持たないため、RPC ごとにセッションを作って
  // 閉じないと、サーバー側にたまり続ける。
  let harness = http_harness();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  for _ in 0..4 {
    client
      .get_run_detail(GetRunDetailRequest { run_slug: "run-1".to_owned() })
      .await
      .expect("GetRunDetail が HTTP 経由で返ります");
    client
      .get_goal_detail(GetGoalDetailRequest { goal_id: "g1".to_owned() })
      .await
      .expect("GetGoalDetail が HTTP 経由で返ります");
  }

  let http = harness.http_server();
  assert_eq!(
    http.created_session_count(),
    1,
    "8 回の画面読み取りは 1 つのセッションを使い回します: {:?}",
    http.session_events()
  );
  assert_eq!(http.live_session_count(), 1, "生きているセッションは 1 つです: {:?}", http.session_events());

  server.stop().await;
}

#[tokio::test]
async fn concurrent_screen_reads_share_one_session() {
  // 同時に届いた画面の読み取りも、セッションを 1 つだけ作って共有する。
  let harness = http_harness();
  let server = start_server(&harness).await;
  // 6 件の接続を先に済ませ、開始の合図で同時に最初の要求を送る。確立が必ず重なるようにして、
  // 直列化が外れたら検出できるようにする。
  let start = std::sync::Arc::new(tokio::sync::Barrier::new(6));
  let mut readers = Vec::new();
  for _ in 0..6 {
    let mut client = client(server.address).await;
    let start = start.clone();
    readers.push(tokio::spawn(async move {
      start.wait().await;
      client.get_goal_detail(GetGoalDetailRequest { goal_id: "g1".to_owned() }).await.map(|_| ())
    }));
  }
  for reader in readers {
    reader.await.expect("読み取りのタスクが終わります").expect("GetGoalDetail が HTTP 経由で返ります");
  }

  let http = harness.http_server();
  assert_eq!(
    http.created_session_count(),
    1,
    "同時の読み取りは 1 つのセッションを共有します: {:?}",
    http.session_events()
  );

  server.stop().await;
}

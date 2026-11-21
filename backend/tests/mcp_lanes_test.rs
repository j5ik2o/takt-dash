//! 接続レーン分離の回帰テスト。
//!
//! 読み取りレーン（定期読み取り）の `takt_list_tasks` が数秒遅れている間に、書き込み
//! レーンの呼び出しが待たされずに短時間で返ることを、遅延つきの mock MCP サーバで
//! 観測します。レーンが分かれていない実装では、書き込みは読み取りの遅延の間ずっと
//! 同じ子プロセスの応答を待ち、このテストの時間の上限を超えます。

mod common;

use std::{
  fs,
  path::Path,
  time::{Duration, Instant},
};

use common::*;
use serde_json::json;
use takt_dash_backend::{
  domain::{GoalId, InquiryId},
  mcp::{Lane, McpClientRegistry},
};

fn goal_id(value: &str) -> GoalId {
  GoalId::try_from(value).expect("テストのゴール ID は有効です")
}

fn inquiry_id(value: &str) -> InquiryId {
  InquiryId::try_from(value).expect("テストの inquiry ID は有効です")
}

/// `takt_list_tasks` だけを3秒遅らせる mock の状態を書く。
fn write_delayed_state(directory: &Path) -> std::path::PathBuf {
  let state = directory.join("state.json");
  let value = json!({
    "goals": [],
    "tasks": [],
    "delayMs": { "takt_list_tasks": 3000 },
  });
  fs::write(&state, value.to_string()).expect("mock の状態を書けます");
  state
}

#[tokio::test]
async fn write_lane_is_not_blocked_by_slow_poller_read() {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_delayed_state(directory.path());
  let cwd = directory.path().to_path_buf();
  let registry = McpClientRegistry::new();
  let (poller_handle, _) = registry.client("local", "proj", Lane::Poller, || mock_config(&cwd, &state, None));
  let (write_handle, _) = registry.client("local", "proj", Lane::Write, || mock_config(&cwd, &state, None));

  // 定期読み取りレーンで、遅延つきの読み取りを開始します。
  let read_cwd = cwd.clone();
  let read_task = tokio::spawn(async move {
    let logs = run_logs(&read_cwd);
    let mut client = poller_handle.lock().await;
    tool_set(&mut client, &read_cwd).read_snapshot(None, &logs).await
  });
  // 読み取りが takt_list_tasks の遅延へ入った頃合いまで待ちます。
  tokio::time::sleep(Duration::from_millis(300)).await;

  // 書き込みレーンの呼び出しは、読み取りの遅延を待たずに返らなければなりません。
  let started = Instant::now();
  let answered = {
    let mut client = write_handle.lock().await;
    write_tool_set(&mut client, &cwd)
      .answer_goal_question(
        &goal_id("3f9c1a54-2b41-4d1e-9d2e-2c1a5b8f7e11"),
        &inquiry_id("q1"),
        json!({ "text": "はい" }),
      )
      .await
  };
  let write_elapsed = started.elapsed();
  assert!(answered.is_ok(), "書き込みは成功します: {answered:?}");
  assert!(
    write_elapsed < Duration::from_secs(2),
    "書き込みは読み取りの3秒の遅延を待たずに返ります（実測 {write_elapsed:?}）"
  );

  // 読み取り自体も、遅延の後に正常に終わります。
  let snapshot = read_task.await.expect("読み取りタスクが終わります").expect("読み取りは成功します");
  assert!(snapshot.goals.is_empty(), "mock のゴールは空です");
  registry.shutdown_all().await;
}

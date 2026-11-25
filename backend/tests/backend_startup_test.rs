//! backend の実行ファイルを起動し、起動がアクティビティへ1回ずつ記録されることを観測する。

mod common;

use std::{
  path::Path,
  process::{Child, Command, Stdio},
  time::{Duration, Instant},
};

use common::*;
use serde_json::json;
use takt_dash_backend::activity::ActivityStore;

const STARTED: &str = "backend_started";
const WAIT_LIMIT: Duration = Duration::from_secs(30);

fn start_backend(config: &Path, sqlite: &Path) -> Child {
  Command::new(env!("CARGO_BIN_EXE_takt-dash-backend"))
    .arg("--config")
    .arg(config)
    .arg("--sqlite")
    .arg(sqlite)
    .args(["--listen", "127.0.0.1:0"])
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .spawn()
    .expect("backend を起動できます")
}

fn started_bodies(sqlite: &Path) -> Vec<String> {
  let Ok(store) = ActivityStore::open(sqlite) else {
    return Vec::new();
  };
  store
    .recent_events(50)
    .unwrap_or_default()
    .into_iter()
    .filter(|event| event.kind.as_str() == STARTED)
    .map(|event| event.body)
    .collect()
}

fn wait_for_started(sqlite: &Path, expected: usize) -> Vec<String> {
  let deadline = Instant::now() + WAIT_LIMIT;
  loop {
    let bodies = started_bodies(sqlite);
    if bodies.len() >= expected || Instant::now() >= deadline {
      return bodies;
    }
    std::thread::sleep(Duration::from_millis(100));
  }
}

fn stop_backend(mut child: Child) {
  let status = Command::new("kill").args(["-TERM", &child.id().to_string()]).status().expect("終了シグナルを送れます");
  assert!(status.success(), "backend へ SIGTERM を送れます");
  child.wait().expect("backend の終了を待てます");
}

#[test]
fn each_backend_start_is_recorded_once_in_the_activity() {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let project_dir = directory.path().join("project");
  std::fs::create_dir_all(&project_dir).expect("プロジェクトの cwd を作れます");
  let state_path = directory.path().join("state.json");
  std::fs::write(&state_path, json!({}).to_string()).expect("mock の状態を書けます");
  let config_path = directory.path().join("taktdash.config.json");
  let sqlite_path = directory.path().join("events.sqlite");
  let config = json!({
    "mcpServers": { "mac-mini": { "command": "node", "args": [mock_script().display().to_string(), "--state", state_path.display().to_string()], "env": {} } },
    "servers": { "mac-mini": { "name": "j5ik2o-mac-mini", "kind": "Mac mini", "concurrency": 4 } },
    "projects": [ { "name": "takt-dash", "server": "mac-mini", "about": "説明", "cwd": project_dir.display().to_string() } ],
    "intervalSeconds": 1,
    "thresholds": { "pendingDecisionStaleMinutes": 240, "runLogSilentMinutes": 10 },
    "sqlite": { "path": sqlite_path.display().to_string() }
  });
  std::fs::write(&config_path, config.to_string()).expect("設定を書けます");

  let first = start_backend(&config_path, &sqlite_path);
  let after_first = wait_for_started(&sqlite_path, 1);
  // 起動の記録は1回だけで、定期読み取りの周期のたびに増えないことを、数周期ぶん待って確かめます。
  std::thread::sleep(Duration::from_secs(3));
  let settled = started_bodies(&sqlite_path);
  stop_backend(first);
  assert_eq!(after_first.len(), 1, "起動を出来事として記録します: {after_first:?}");
  assert_eq!(settled.len(), 1, "同じ起動を周期のたびに記録しません: {settled:?}");

  let second = start_backend(&config_path, &sqlite_path);
  let after_second = wait_for_started(&sqlite_path, 2);
  stop_backend(second);
  assert_eq!(after_second.len(), 2, "再起動を新しい1件として記録します: {after_second:?}");
}

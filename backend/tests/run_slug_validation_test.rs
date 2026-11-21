//! takt_list_tasks のタスクが持つ runSlug が run ディレクトリ名として不正なとき、状態の読み取りが
//! 解析エラーになり、その値で run の保存場所を読まないことを検証する。

mod common;

use std::io::Write;

use common::*;
use serde_json::json;
use takt_dash_backend::{mcp::McpClientRegistry, poller::Poller, store::StateStore};

fn write_json(path: &std::path::Path, value: &serde_json::Value) {
  let mut file = std::fs::File::create(path).expect("JSON ファイルを作れます");
  file.write_all(value.to_string().as_bytes()).expect("JSON ファイルへ書けます");
}

#[tokio::test]
async fn a_task_run_slug_that_is_not_a_single_directory_name_fails_the_project_read() {
  // run の保存場所は runSlug から解決するため、ディレクトリ名にならない runSlug を受け付けると
  // .takt/runs の外を指し得る。この値を持つタスクの読み取りはプロジェクトの失敗になり、
  // 状態ストアには run が入らない。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let project_dir = directory.path().join("project");
  std::fs::create_dir_all(&project_dir).expect("プロジェクトの cwd を作れます");
  let fixture = RunFixture::new(&project_dir);
  fixture.write_tasks(&[RunTaskLine { run_slug: Some("r1"), source_run_slug: None }]);
  fixture.write_meta("r1", &running_meta("implement", 1, "development-core", "2026-11-13T10:00:00Z"));
  let state_path = directory.path().join("state.json");
  write_json(
    &state_path,
    &json!({ "tasks": [ { "name": "a", "status": "running", "runSlug": "../escape", "startedAt": "2026-11-13T10:00:00Z" } ] }),
  );
  let config_path = directory.path().join("taktdash.config.json");
  write_json(
    &config_path,
    &json!({
      "mcpServers": { "mac-mini": { "command": "node", "args": [mock_script().display().to_string(), "--state", state_path.display().to_string()], "env": {} } },
      "servers": { "mac-mini": { "name": "j5ik2o-mac-mini", "kind": "Mac mini", "concurrency": 4 } },
      "projects": [ { "name": "takt-dash", "server": "mac-mini", "about": "説明", "cwd": project_dir.display().to_string() } ],
      "intervalSeconds": 30,
      "thresholds": { "pendingDecisionStaleMinutes": 240, "runLogSilentMinutes": 10 },
      "sqlite": { "path": "data/taktdash.sqlite" }
    }),
  );
  let mut servers = std::collections::BTreeMap::new();
  servers.insert("mac-mini".to_owned(), server_meta());
  let store = std::sync::Arc::new(StateStore::new(servers, vec![project("takt-dash", &project_dir)], thresholds()));
  let activity = std::sync::Arc::new(
    takt_dash_backend::activity::ActivityStore::open_in_memory().expect("メモリ上の SQLite を開けます"),
  );
  let (changes, _) = tokio::sync::broadcast::channel(16);
  let mut poller = Poller::new(config_path, activity, store.clone(), changes, McpClientRegistry::new());

  let report = poller.read_cycle_once().await.expect("1周期を実行できます");
  poller.shutdown().await;

  assert_eq!(report.projects_failed, 1, "ディレクトリ名にならない runSlug を持つタスクはプロジェクトの失敗です");
  assert_eq!(report.projects_read, 0);
  let snapshot = store.slot("takt-dash").and_then(|slot| slot.snapshot);
  assert!(snapshot.is_none(), "解析に失敗した読み取りを状態ストアへ入れません: {snapshot:?}");
}

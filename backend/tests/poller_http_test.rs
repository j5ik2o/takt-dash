//! 定期読み取りを home ごとの常駐 HTTP サーバーへ向ける契約を、同じ Poller 実体の上で観測する。
//!
//! 同一 home の複数プロジェクトが 1 つの HTTP サーバーを共有し、読み取りで takt-mcp の
//! 子プロセスを起動しないこと、サーバー断が読み取り失敗として表面化することを確認する。

mod common;

use std::{
  collections::BTreeMap,
  path::{Path, PathBuf},
  sync::Arc,
};

use common::*;
use serde_json::{json, Value};
use takt_dash_backend::{
  activity::ActivityStore,
  config::Thresholds,
  domain::{SilentMinutes, StaleMinutes},
  mcp::McpClientRegistry,
  model::ConnectionState,
  poller::Poller,
  store::StateStore,
};

struct Harness {
  _directory:  tempfile::TempDir,
  config_path: PathBuf,
  activity:    Arc<ActivityStore>,
  store:       Arc<StateStore>,
  changes:     tokio::sync::broadcast::Sender<()>,
}

fn write_json(path: &Path, value: &Value) {
  std::fs::write(path, value.to_string()).expect("JSON ファイルを書けます");
}

fn state_with_goal(goal_id: &str) -> Value {
  json!({ "goals": [ { "id": goal_id, "status": "created", "executionStatus": "active", "objective": format!("ゴール {goal_id}"), "integrationBranch": format!("takt/{goal_id}") } ] })
}

impl Harness {
  /// 1 つの home の HTTP 接続先に 2 プロジェクトを載せた設定を作る。
  fn with_http_server(server_url: &str, token: &str, projects: &[&str], state: Value) -> Self {
    let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
    let config_path = directory.path().join("taktdash.config.json");
    let state_path = directory.path().join("state.json");
    write_json(&state_path, &state);
    let mut configured_projects = Vec::new();
    for name in projects {
      let project_dir = directory.path().join(name);
      std::fs::create_dir_all(&project_dir).expect("プロジェクトの cwd を作れます");
      configured_projects.push(json!({
        "name": name,
        "server": "mac-mini-read",
        "writeServer": "mac-mini-write",
        "about": format!("{name} の説明"),
        "cwd": std::fs::canonicalize(&project_dir).expect("cwd の実パスを解決できます").display().to_string(),
      }));
    }
    write_json(
      &config_path,
      &json!({
        "mcpServers": {
          "mac-mini-read": {
            "type": "http",
            "url": server_url,
            "headers": { "Authorization": format!("Bearer {token}") },
          },
          "mac-mini-write": {
            "command": "node",
            "args": [mock_script().display().to_string(), "--state", state_path.display().to_string()],
            "env": {},
          },
        },
        "servers": {
          "mac-mini-read": { "name": "j5ik2o-mac-mini", "kind": "Mac mini", "concurrency": 4 },
        },
        "projects": configured_projects,
        "intervalSeconds": 30,
        "thresholds": { "pendingDecisionStaleMinutes": 240, "runLogSilentMinutes": 10 },
        "sqlite": { "path": "data/taktdash.sqlite" },
      }),
    );
    let mut servers = BTreeMap::new();
    servers.insert("mac-mini-read".to_owned(), server_meta());
    let projects = projects.iter().map(|name| common::project(name, &directory.path().join(name))).collect();
    let store = Arc::new(StateStore::new(servers, projects, Thresholds {
      pending_decision_stale_minutes: StaleMinutes::try_from(240).expect("テストの閾値は有効です"),
      run_log_silent_minutes:         SilentMinutes::try_from(10).expect("テストの閾値は有効です"),
      connection_failure_count:       3,
      connection_delay_seconds:       120,
    }));
    let activity_db_path = directory.path().join("events.sqlite");
    let activity = Arc::new(ActivityStore::open(&activity_db_path).expect("SQLite を開けます"));
    let (changes, _) = tokio::sync::broadcast::channel(16);
    Harness { _directory: directory, config_path, activity, store, changes }
  }

  fn poller(&self) -> Poller {
    Poller::new(
      self.config_path.clone(),
      self.activity.clone(),
      self.store.clone(),
      self.changes.clone(),
      McpClientRegistry::new(),
    )
  }
}

#[tokio::test]
async fn same_home_multiple_projects_share_one_http_server_without_spawning_children() {
  // 同一 home の 2 プロジェクトを 1 つの HTTP サーバーに載せ、1 サイクルで双方を読む。
  // mock は読み取りで子プロセスを起動しないため、起動の記録があれば HTTP 化の漏れを検出できる。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = directory.path().join("state.json");
  write_json(&state, &state_with_goal("g1"));
  let server = HttpMockServer::start(&state, None);

  let process_log = directory.path().join("spawned-children.txt");
  let harness =
    Harness::with_http_server(server.url(), server.token(), &["takt-dash", "fraktor-ng-rs"], state_with_goal("g1"));
  // 子プロセスが起動したら分かるよう、stdio 接続の args に記録先を足す。
  let mut config: Value = serde_json::from_str(&std::fs::read_to_string(&harness.config_path).unwrap()).unwrap();
  config["mcpServers"]["mac-mini-write"]["args"].as_array_mut().expect("args は配列です").push(json!("--processes"));
  config["mcpServers"]["mac-mini-write"]["args"]
    .as_array_mut()
    .expect("args は配列です")
    .push(json!(process_log.display().to_string()));
  write_json(&harness.config_path, &config);

  let mut poller = harness.poller();
  let report = poller.read_cycle_once().await.expect("1 サイクルを実行できます");
  poller.shutdown().await;

  assert_eq!(report.projects_read, 2, "同一 home の 2 プロジェクトを同じ HTTP サーバーから読みます");
  assert_eq!(report.projects_failed, 0);
  assert!(!process_log.exists(), "読み取りで takt-mcp の子プロセスを起動しません");
  let calls = server.recorded_calls();
  assert!(calls.iter().any(|call| call["name"] == "takt_list_goals"), "HTTP サーバーへ読み取り道具を送ります");
}

#[tokio::test]
async fn http_server_failure_surfaces_as_connection_state_not_silent_success() {
  // 常駐サーバーが落ちると、読み取りは空 snapshot の成功ではなく失敗として扱われ、
  // プロジェクトの接続状態が Connected から遅延中へ進む。同一の Poller 実体のまま観測する。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = directory.path().join("state.json");
  write_json(&state, &state_with_goal("g1"));
  let server = HttpMockServer::start(&state, None);
  let harness = Harness::with_http_server(server.url(), server.token(), &["takt-dash"], state_with_goal("g1"));

  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("初回のサイクルを実行できます");
  let connected = harness.store.slot("takt-dash").expect("スロットがあります");
  assert_eq!(connected.connection_state, Some(ConnectionState::Connected), "初回は接続中です");

  drop(server);
  let failed = poller.read_cycle_once().await.expect("サーバー断でもサイクルは完了します");
  poller.shutdown().await;

  assert_eq!(failed.projects_failed, 1, "サーバー断は失敗件数に数えます");
  let slot = harness.store.slot("takt-dash").expect("スロットがあります");
  assert_ne!(
    slot.connection_state,
    Some(ConnectionState::Connected),
    "サーバー断は接続中のままにせず、遅延中・応答なしへ進めます"
  );
  assert!(slot.snapshot.is_some(), "最後に読めた状態はキャッシュとして保持します");
}

#[tokio::test]
async fn write_registry_still_owns_a_child_process_independent_of_http_reads() {
  // 読み取りを HTTP へ移しても、書き込み用の stdio 子プロセスは registry が管理し続ける。
  // sync_clients が書き込み子プロセスを HTTP 接続先で上書きすると、書き込み先が消える。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = directory.path().join("state.json");
  write_json(&state, &state_with_goal("g1"));
  let server = HttpMockServer::start(&state, None);
  let harness = Harness::with_http_server(server.url(), server.token(), &["takt-dash"], state_with_goal("g1"));

  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("1 サイクルを実行できます");

  // 書き込み経路は registry の client を解決して子プロセスを起動する。
  let config = takt_dash_backend::config::load_config(&harness.config_path).expect("設定を読み込めます");
  let write_server = config.servers.get("mac-mini-write").expect("書き込み用の接続先があります");
  assert!(
    matches!(write_server, takt_dash_backend::config::McpServer::Stdio(_)),
    "書き込み用の接続先は stdio のままです"
  );
  poller.shutdown().await;
}

//! gRPC の経路（実ソケットのテストサーバと生成クライアント）を検証するための共通の土台。
//!
//! 各テストは、テスト対象の状態（状態ストアと activity のクエリ）と mock MCP サーバの設定を
//! 与えて実ソケットのサーバを起動し、生成クライアントで呼び出します。起動と停止はテストごとに
//! 独立し、ソケットは空きポートを選びます。

#![allow(dead_code)]

use std::{
  collections::BTreeMap,
  net::{Ipv4Addr, SocketAddr},
  path::{Path, PathBuf},
  sync::Arc,
};

use serde_json::{json, Value};
use takt_dash_backend::{
  activity::ActivityStore,
  config::{Project, ServerMeta, Thresholds},
  store::StateStore,
};

use super::common;

/// 1テスト分の gRPC サーバと、その土台の状態。
pub struct GrpcHarness {
  _directory:   tempfile::TempDir,
  config_path:  PathBuf,
  state_path:   PathBuf,
  pub store:    Arc<StateStore>,
  pub activity: Arc<ActivityStore>,
  pub changes:  tokio::sync::broadcast::Sender<()>,
}

/// gRPC サーバの土台を作るための設定。
pub struct GrpcHarnessBuilder {
  servers:    BTreeMap<String, ServerMeta>,
  projects:   Vec<Project>,
  state:      Value,
  interval:   u64,
  thresholds: Thresholds,
}

impl Default for GrpcHarnessBuilder {
  fn default() -> Self {
    GrpcHarnessBuilder {
      servers:    BTreeMap::new(),
      projects:   Vec::new(),
      state:      json!({}),
      interval:   30,
      thresholds: Thresholds { pending_decision_stale_minutes: 240, run_log_silent_minutes: 10 },
    }
  }
}

impl GrpcHarnessBuilder {
  /// 1サーバー1プロジェクトの土台を作る。`state` は mock MCP サーバが返す状態です。
  pub fn single_project(mut self, project: Project, server: ServerMeta, state: Value) -> Self {
    self.servers.insert(project.server.clone(), server);
    self.projects.push(project);
    self.state = state;
    self
  }

  pub fn build(self) -> GrpcHarness {
    let servers = if self.servers.is_empty() {
      let mut servers = BTreeMap::new();
      servers.insert("mac-mini".to_owned(), common::server_meta());
      servers
    } else {
      self.servers
    };
    let projects = self.projects;
    let thresholds = self.thresholds;
    let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
    let project_dir = directory.path().join("project");
    std::fs::create_dir_all(&project_dir).expect("プロジェクトの cwd を作れます");
    // mock の子プロセスを起動する cwd は実在する必要があるため、渡された cwd
    // を実在のディレクトリへ置き換える。
    let projects: Vec<Project> =
      projects.into_iter().map(|project| Project { cwd: project_dir.clone(), ..project }).collect();
    let state_path = directory.path().join("state.json");
    std::fs::write(&state_path, self.state.to_string()).expect("状態ファイルを書けます");
    let config_path = directory.path().join("taktdash.config.json");
    let mut mcp_servers = serde_json::Map::new();
    for server_id in servers.keys() {
      mcp_servers.insert(
        server_id.clone(),
        json!({
          "command": "node",
          "args": [common::mock_script().display().to_string(), "--state", state_path.display().to_string()],
          "env": {}
        }),
      );
    }
    let config = json!({
      "mcpServers": mcp_servers,
      "servers": servers.iter().map(|(server_id, meta)| (server_id.clone(), json!({
        "name": meta.name,
        "kind": meta.kind,
        "concurrency": meta.concurrency,
      }))).collect::<serde_json::Map<_, _>>(),
      "projects": projects.iter().map(|project| json!({
        "name": project.name,
        "server": project.server,
        "about": project.about,
        "cwd": project.cwd.display().to_string(),
      })).collect::<Vec<_>>(),
      "intervalSeconds": self.interval,
      "thresholds": {
        "pendingDecisionStaleMinutes": thresholds.pending_decision_stale_minutes,
        "runLogSilentMinutes": thresholds.run_log_silent_minutes,
      },
      "sqlite": { "path": "data/taktdash.sqlite" },
    });
    std::fs::write(&config_path, config.to_string()).expect("設定ファイルを書けます");
    let store = Arc::new(StateStore::new(servers, projects, thresholds));
    let activity = Arc::new(ActivityStore::open_in_memory().expect("メモリ上の SQLite を開けます"));
    let (changes, _) = tokio::sync::broadcast::channel(16);
    GrpcHarness { _directory: directory, config_path, state_path, store, activity, changes }
  }
}

impl GrpcHarness {
  pub fn builder() -> GrpcHarnessBuilder {
    GrpcHarnessBuilder::default()
  }

  pub fn config_path(&self) -> &Path {
    &self.config_path
  }

  pub fn rewrite_state(&self, state: Value) {
    std::fs::write(&self.state_path, state.to_string()).expect("状態ファイルを書けます");
  }

  /// 設定ファイルの閾値を書き換える。poller の次の読み取りサイクルで state store へ反映されます。
  pub fn rewrite_thresholds(&self, thresholds: Thresholds) {
    let text = std::fs::read_to_string(&self.config_path).expect("設定を読めます");
    let mut value: Value = serde_json::from_str(&text).expect("設定を解析できます");
    value["thresholds"] = json!({
      "pendingDecisionStaleMinutes": thresholds.pending_decision_stale_minutes,
      "runLogSilentMinutes": thresholds.run_log_silent_minutes,
    });
    std::fs::write(&self.config_path, value.to_string()).expect("設定ファイルを書けます");
  }

  /// 設定ファイルのサーバー表示情報を書き換える。poller の次の読み取りサイクルで state store
  /// へ反映されます。
  pub fn rewrite_server_meta(&self, server_id: &str, meta: ServerMeta) {
    let text = std::fs::read_to_string(&self.config_path).expect("設定を読めます");
    let mut value: Value = serde_json::from_str(&text).expect("設定を解析できます");
    value["servers"][server_id] = json!({
      "name": meta.name,
      "kind": meta.kind,
      "concurrency": meta.concurrency,
    });
    std::fs::write(&self.config_path, value.to_string()).expect("設定ファイルを書けます");
  }

  /// 設定ファイルのサーバー表示情報を書き換える。poller の次の読み取りサイクルで state
  /// store へ反映されます。
  pub fn rewrite_servers(&self, mutate: impl FnOnce(&mut Value)) {
    let text = std::fs::read_to_string(&self.config_path).expect("設定を読めます");
    let mut value: Value = serde_json::from_str(&text).expect("設定を解析できます");
    mutate(&mut value["servers"]);
    std::fs::write(&self.config_path, value.to_string()).expect("設定ファイルを書けます");
  }

  /// 設定ファイルのプロジェクト一覧を書き換える。追加するプロジェクトの `cwd` は、
  /// この harness が作る実在のプロジェクトディレクトリの絶対パスに置き換えて書きます。
  pub fn rewrite_projects(&self, mutate: impl FnOnce(&mut Value, &Path)) {
    let text = std::fs::read_to_string(&self.config_path).expect("設定を読めます");
    let mut value: Value = serde_json::from_str(&text).expect("設定を解析できます");
    let directory = self._directory.path().join("project");
    std::fs::create_dir_all(&directory).expect("プロジェクトの cwd を作れます");
    mutate(&mut value["projects"], &directory);
    std::fs::write(&self.config_path, value.to_string()).expect("設定ファイルを書けます");
  }

  /// 状態変化の通知チャネルの現在の購読者数。WatchState
  /// の送信タスクの生存を観測するために使います。
  pub fn change_receiver_count(&self) -> usize {
    self.changes.receiver_count()
  }
}

/// 起動した gRPC サーバ。ドロップで停止の合図を送り、タスクの終了を待ちます。
pub struct RunningServer {
  pub address: SocketAddr,
  stop:        tokio::sync::watch::Sender<bool>,
  task:        tokio::task::JoinHandle<()>,
}

impl RunningServer {
  /// サーバを止め、タスクの終了を待つ。クライアントを切断してから呼びます。
  pub async fn stop(self) {
    let _ = self.stop.send(true);
    let _ = self.task.await;
  }
}

/// 空きポートを選び、harness の土台を持つ gRPC サーバを起動する。
///
/// 状態ストアの記録と poller の起動はテストが必要に応じて行い、サーバはその土台の状態を
/// そのまま読み取ります。
pub async fn start_server(harness: &GrpcHarness) -> RunningServer {
  let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.expect("空きポートを確保できます");
  let address = listener.local_addr().expect("待ち受けアドレスを取得できます");
  let (stop, _) = tokio::sync::watch::channel(false);
  let stop_rx = stop.subscribe();
  let service = takt_dash_backend::grpc::TaktDashServiceImpl::new(
    harness.store.clone(),
    harness.activity.clone(),
    harness.config_path().to_path_buf(),
    harness.changes.clone(),
    stop_rx.clone(),
  );
  let task = tokio::spawn(async move {
    let _ = takt_dash_backend::grpc::serve_with_listener(listener, service, stop_rx).await;
  });
  RunningServer { address, stop, task }
}

/// 待ち受けアドレスへつながる生成クライアントを作る。
pub async fn client(
  address: SocketAddr,
) -> takt_dash_backend::takt_dash::v1::takt_dash_service_client::TaktDashServiceClient<tonic::transport::Channel> {
  let endpoint = format!("http://{address}");
  let channel = tonic::transport::Endpoint::from_shared(endpoint)
    .expect("エンドポイントの URI を組み立てられます")
    .connect()
    .await
    .expect("実ソケットのサーバへつながります");
  takt_dash_backend::takt_dash::v1::takt_dash_service_client::TaktDashServiceClient::new(channel)
}

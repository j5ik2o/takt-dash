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
  conversation::{ConversationPaths, ConversationReader},
  store::StateStore,
};

use super::common;

/// 1テスト分の gRPC サーバと、その土台の状態。
pub struct GrpcHarness {
  _directory:   tempfile::TempDir,
  config_path:  PathBuf,
  state_path:   PathBuf,
  home:         PathBuf,
  project_dir:  PathBuf,
  project_cwds: BTreeMap<String, PathBuf>,
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
  /// 1プロジェクトを土台へ加える。複数回呼ぶと複数プロジェクトの土台になります。
  /// `state` は mock MCP サーバが返す状態です。
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
    let home = directory.path().join("home");
    std::fs::create_dir_all(&home).expect("テスト用のホームを作れます");
    let home = std::fs::canonicalize(&home).expect("テスト用のホームの実パスを解決できます");
    // mock の子プロセスを起動する cwd は実在する必要があるため、プロジェクトごとに
    // 実在のディレクトリを割り当てて置き換える。単一プロジェクトでは従来と同じ cwd を使う。
    let multiple = projects.len() > 1;
    let mut project_cwds = BTreeMap::new();
    for project in &projects {
      let path =
        if multiple { directory.path().join("projects").join(&project.name) } else { directory.path().join("project") };
      std::fs::create_dir_all(&path).expect("プロジェクトの cwd を作れます");
      let path = std::fs::canonicalize(&path).expect("プロジェクトの cwd の実パスを解決できます");
      project_cwds.insert(project.name.clone(), path);
    }
    let project_dir = match project_cwds.values().next() {
      | Some(cwd) => cwd.clone(),
      | None => {
        let path = directory.path().join("project");
        std::fs::create_dir_all(&path).expect("プロジェクトの cwd を作れます");
        std::fs::canonicalize(&path).expect("プロジェクトの cwd の実パスを解決できます")
      },
    };
    let projects: Vec<Project> = projects
      .into_iter()
      .map(|project| {
        let cwd = project_cwds.get(&project.name).cloned().expect("プロジェクトの cwd は build 時に割り当て済みです");
        Project { cwd, ..project }
      })
      .collect();
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
    GrpcHarness {
      _directory: directory,
      config_path,
      state_path,
      home,
      project_dir,
      project_cwds,
      store,
      activity,
      changes,
    }
  }
}

impl GrpcHarness {
  pub fn builder() -> GrpcHarnessBuilder {
    GrpcHarnessBuilder::default()
  }

  pub fn config_path(&self) -> &Path {
    &self.config_path
  }

  /// manager の記録と opencode の DB を置く、テスト用のホーム。
  pub fn home(&self) -> &Path {
    &self.home
  }

  /// 設定の全プロジェクトに割り当てた、実在する cwd。
  pub fn project_dir(&self) -> &Path {
    &self.project_dir
  }

  /// 指定したプロジェクトに割り当てた、実在する cwd。
  pub fn project_cwd(&self, name: &str) -> &Path {
    self
      .project_cwds
      .get(name)
      .map(PathBuf::as_path)
      .unwrap_or_else(|| panic!("プロジェクト {name} はこの harness の設定にありません"))
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
  let conversation = Arc::new(ConversationReader::new(ConversationPaths::from_home(harness.home())));
  let service = takt_dash_backend::grpc::TaktDashServiceImpl::new(
    harness.store.clone(),
    harness.activity.clone(),
    harness.config_path().to_path_buf(),
    conversation,
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

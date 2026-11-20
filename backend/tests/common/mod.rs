//! テストで共有する mock MCP サーバの起動と、model 型のフィクスチャ生成。

#![allow(dead_code)]

use std::{
  path::{Path, PathBuf},
  process::{Child, Command, Stdio},
  time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use takt_dash_backend::{
  config::{HttpMcpServer, HttpServerEndpoint, McpConnection, Project, ServerMeta, Thresholds},
  domain::{GoalId, InquiryId, PullRequestNumber, SilentMinutes, StaleMinutes, TaskName},
  mcp::{McpClient, McpClientConfig, ToolSet},
  model::{
    ExecutionStatus, GoalEntry, GoalStatus, InquiryEntry, InquiryKind, NotificationEntry, ProjectSnapshot, RunEntry,
    TaskEntry, TaskStatus,
  },
};

/// mock MCP サーバのスクリプトへの絶対パス。
pub fn mock_script() -> PathBuf {
  Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("mock_mcp_server.mjs")
}

/// mock サーバの起動設定を作る。`--state` と `--behavior` を渡します。
pub fn mock_config(cwd: &Path, state: &Path, behavior: Option<&str>) -> McpClientConfig {
  mock_config_with_timeout(cwd, state, behavior, 5)
}

/// タイムアウトを明示して mock サーバの起動設定を作る。
pub fn mock_config_with_timeout(
  cwd: &Path,
  state: &Path,
  behavior: Option<&str>,
  timeout_secs: u64,
) -> McpClientConfig {
  let script = mock_script();
  let mut args = vec![script.display().to_string(), "--state".to_owned(), state.display().to_string()];
  if let Some(behavior) = behavior {
    args.push("--behavior".to_owned());
    args.push(behavior.to_owned());
  }
  McpClientConfig {
    command: "node".to_owned(),
    args,
    env: Default::default(),
    cwd: cwd.to_path_buf(),
    request_timeout: std::time::Duration::from_secs(timeout_secs),
  }
}

/// mock サーバへ接続するクライアントを作る。
pub fn mock_client(config: McpClientConfig) -> McpClient {
  McpClient::new(config)
}

/// 受け取った道具呼び出しを JSONL で記録する mock サーバの起動設定を作る。
pub fn mock_config_with_calls(cwd: &Path, state: &Path, calls: &Path) -> McpClientConfig {
  let mut config = mock_config(cwd, state, None);
  config.args.push("--calls".to_owned());
  config.args.push(calls.display().to_string());
  config
}

/// プロジェクトの cwd を持つ道具のまとまりを作る。mock は実契約と同じく cwd 必須なので、
/// テストはプロジェクトの cwd（通常は mock を起動したディレクトリ）を渡します。
pub fn tool_set<'a>(client: &'a mut McpClient, cwd: &Path) -> ToolSet<'a> {
  ToolSet::new(client, cwd.to_path_buf())
}

/// takt-mcp の道具呼び出し引数へ cwd を加える。
pub fn with_cwd(cwd: &Path, extra: serde_json::Value) -> serde_json::Value {
  let mut object = serde_json::Map::new();
  object.insert("cwd".to_owned(), serde_json::Value::String(cwd.display().to_string()));
  if let serde_json::Value::Object(extra) = extra {
    object.extend(extra);
  }
  serde_json::Value::Object(object)
}

pub fn time(text: &str) -> DateTime<Utc> {
  DateTime::parse_from_rfc3339(text).expect("テストの時刻が RFC3339 ではありません").with_timezone(&Utc)
}

pub fn goal(id: &str, status: GoalStatus, execution: ExecutionStatus) -> GoalEntry {
  GoalEntry {
    goal_id: GoalId::try_from(id).expect("テストのゴール ID は有効です"),
    status,
    execution_status: execution,
    objective: format!("ゴール {id}"),
    integration_branch: format!("takt/{id}"),
    pull_request_number: None,
    pull_request_url: None,
    awaiting_merge_at: None,
    paused_reason: None,
  }
}

pub fn merge_goal(id: &str, pull_request: i64) -> GoalEntry {
  GoalEntry {
    pull_request_number: Some(PullRequestNumber::try_from(pull_request).expect("テストの PR 番号は正です")),
    pull_request_url: Some(format!("https://example.test/pr/{pull_request}")),
    ..goal(id, GoalStatus::AwaitingMerge, ExecutionStatus::Active)
  }
}

pub fn task(name: &str, status: TaskStatus) -> TaskEntry {
  TaskEntry {
    name: TaskName::try_from(name).expect("テストのタスク名は有効です"),
    goal_id: None,
    status,
    started_at: None,
    run_slug: None,
    source_run_slug: None,
    current_step: None,
    merge_task: false,
  }
}

pub fn task_for_goal(name: &str, goal_id: &str, status: TaskStatus, merge_task: bool) -> TaskEntry {
  TaskEntry {
    goal_id: Some(GoalId::try_from(goal_id).expect("テストのゴール ID は有効です")),
    merge_task,
    ..task(name, status)
  }
}

pub fn inquiry(id: &str, answered: bool, created_at: DateTime<Utc>) -> InquiryEntry {
  InquiryEntry {
    inquiry_id: InquiryId::try_from(id).expect("テストの inquiry ID は有効です"),
    goal_id: None,
    kind: InquiryKind::Question,
    body: format!("問い合わせ {id}"),
    options: vec!["はい".to_owned(), "いいえ".to_owned()],
    answered,
    created_at,
  }
}

pub fn notification(id: &str, kind: &str, recorded_at: DateTime<Utc>) -> NotificationEntry {
  NotificationEntry {
    notification_id: id.to_owned(),
    goal_id: GoalId::try_from("g1").expect("テストのゴール ID は有効です"),
    kind: kind.to_owned(),
    body: format!("通知 {id}"),
    recorded_at,
  }
}

pub fn run(run_slug: &str, task_name: &str, last_log_at: DateTime<Utc>) -> RunEntry {
  RunEntry {
    run_slug:                run_slug.to_owned(),
    session_id:              String::new(),
    task_name:               TaskName::try_from(task_name).expect("テストのタスク名は有効です"),
    goal_id:                 None,
    summary:                 String::new(),
    status:                  takt_dash_backend::model::RunStatus::Running,
    current_step:            Some("implement".to_owned()),
    current_iteration:       None,
    current_step_number:     None,
    total_steps:             None,
    started_at:              Some(last_log_at),
    current_step_started_at: None,
    last_log_at:             Some(last_log_at),
    steps:                   vec![takt_dash_backend::model::RunStepEntry {
      step_name:  "implement".to_owned(),
      model:      String::new(),
      started_at: last_log_at,
      ended_at:   None,
    }],
    reports:                 Vec::new(),
  }
}

pub fn snapshot(
  goals: Vec<GoalEntry>,
  tasks: Vec<TaskEntry>,
  inquiries: Vec<InquiryEntry>,
  runs: Vec<RunEntry>,
  notifications: Vec<NotificationEntry>,
) -> ProjectSnapshot {
  ProjectSnapshot { goals, tasks, inquiries, runs, notifications, proposals: Vec::new() }
}

pub fn server_meta() -> ServerMeta {
  ServerMeta { name: "j5ik2o-mac-mini".to_owned(), kind: "Mac mini".to_owned(), concurrency: 4 }
}

pub fn project(name: &str, cwd: &Path) -> Project {
  Project {
    name:         name.to_owned(),
    server:       "mac-mini".to_owned(),
    write_server: "mac-mini".to_owned(),
    about:        format!("{name} の説明"),
    cwd:          cwd.to_path_buf(),
  }
}

/// mock HTTP MCP サーバのスクリプトへの絶対パス。
pub fn mock_http_script() -> PathBuf {
  Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("mock_mcp_http_server.mjs")
}

/// テスト用の接続先とトークンを持つ HTTP 接続設定。
pub fn http_connection(url: &str, token: &str) -> McpConnection {
  McpConnection::Http(HttpMcpServer {
    endpoint:      HttpServerEndpoint::try_from(url).expect("テストの HTTP 接続先は 127.0.0.1 の url です"),
    authorization: format!("Bearer {token}"),
  })
}

/// mock HTTP MCP サーバの起動に必要な入力。
pub struct HttpMockServer {
  child:      Child,
  url:        String,
  token:      String,
  calls:      PathBuf,
  sessions:   PathBuf,
  _directory: tempfile::TempDir,
}

impl HttpMockServer {
  /// mock HTTP MCP サーバを空きポートで起動し、待ち受ける URL を返す。
  ///
  /// `--state` で読み取り道具の応答になる状態ファイルを渡します。起動待ちは ready ファイルが
  /// 書かれるまで行い、固定の sleep では待ち合わせません。
  pub fn start(state: &Path, behavior: Option<&str>) -> Self {
    Self::start_with_token(state, behavior, "test-token")
  }

  /// 期待する Authorization のトークンを指定して mock HTTP MCP サーバを起動する。
  pub fn start_with_token(state: &Path, behavior: Option<&str>, token: &str) -> Self {
    Self::start_on_port_with_token(state, behavior, token, 0)
  }

  /// 指定したポートで mock HTTP MCP サーバを起動する。`port` が 0 なら空きポートを選びます。
  ///
  /// サーバー再起動を再現し、同じ接続先で新しいプロセスを立ち上げるために使います。
  pub fn start_on_port(state: &Path, behavior: Option<&str>, port: u16) -> Self {
    Self::start_on_port_with_token(state, behavior, "test-token", port)
  }

  fn start_on_port_with_token(state: &Path, behavior: Option<&str>, token: &str, port: u16) -> Self {
    let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
    let ready = directory.path().join("ready.json");
    let calls = directory.path().join("calls.jsonl");
    let sessions = directory.path().join("sessions.log");
    let script = mock_http_script();
    let mut command = Command::new("node");
    command
      .arg(&script)
      .arg("--state")
      .arg(state)
      .arg("--ready-file")
      .arg(&ready)
      .arg("--calls")
      .arg(&calls)
      .arg("--session-log")
      .arg(&sessions)
      .arg("--require-auth")
      .arg(format!("Bearer {token}"))
      .arg("--port")
      .arg(port.to_string())
      .stdout(Stdio::null())
      .stderr(Stdio::null());
    if let Some(behavior) = behavior {
      command.arg("--behavior").arg(behavior);
    }
    let mut child = command.spawn().expect("mock HTTP MCP サーバを起動できます");
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
      if ready.exists() {
        let text = std::fs::read_to_string(&ready).expect("ready ファイルを読めます");
        let value: serde_json::Value = serde_json::from_str(&text).expect("ready ファイルは JSON です");
        let url = value["url"].as_str().expect("ready ファイルに url があります").to_owned();
        return HttpMockServer { child, url, token: token.to_owned(), calls, sessions, _directory: directory };
      }
      std::thread::sleep(Duration::from_millis(25));
    }
    let _ = child.kill();
    let _ = child.wait();
    panic!("mock HTTP MCP サーバが 10 秒以内に起動しませんでした");
  }

  pub fn url(&self) -> &str {
    &self.url
  }

  pub fn token(&self) -> &str {
    &self.token
  }

  /// mock サーバに対する takt-mcp の HTTP 接続設定。
  pub fn connection(&self) -> McpConnection {
    http_connection(&self.url, &self.token)
  }

  /// 受け取った tools/call を JSONL から読む。記録が無ければ空です。
  pub fn recorded_calls(&self) -> Vec<serde_json::Value> {
    let Ok(text) = std::fs::read_to_string(&self.calls) else {
      return Vec::new();
    };
    text
      .lines()
      .filter(|line| !line.trim().is_empty())
      .map(|line| serde_json::from_str(line).expect("記録した道具呼び出しを解析できます"))
      .collect()
  }

  /// セッションの採番と再利用の記録（"new ..." / "reuse ..."）を読む。
  pub fn session_events(&self) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(&self.sessions) else {
      return Vec::new();
    };
    text.lines().filter(|line| !line.trim().is_empty()).map(str::to_owned).collect()
  }

  /// 接続を確認できるまで待つために、テストから URL のポート番号を返す。
  pub fn port(&self) -> u16 {
    let without_scheme = self.url.trim_start_matches("http://");
    let authority = without_scheme.trim_end_matches('/');
    authority.rsplit(':').next().and_then(|port| port.parse().ok()).expect("url からポート番号を取れます")
  }
}

impl Drop for HttpMockServer {
  fn drop(&mut self) {
    let _ = self.child.kill();
    let _ = self.child.wait();
  }
}

pub fn thresholds() -> Thresholds {
  Thresholds {
    pending_decision_stale_minutes: StaleMinutes::try_from(240).expect("テストの閾値は有効です"),
    run_log_silent_minutes:         SilentMinutes::try_from(10).expect("テストの閾値は有効です"),
    connection_failure_count:       3,
    connection_delay_seconds:       120,
  }
}

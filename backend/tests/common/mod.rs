//! テストで共有する mock MCP サーバの起動と、model 型のフィクスチャ生成。

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use takt_dash_backend::{
  config::{McpServer, Project, ServerMeta, Thresholds},
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
    goal_id: id.to_owned(),
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
    pull_request_number: Some(pull_request),
    pull_request_url: Some(format!("https://example.test/pr/{pull_request}")),
    ..goal(id, GoalStatus::AwaitingMerge, ExecutionStatus::Active)
  }
}

pub fn task(name: &str, status: TaskStatus) -> TaskEntry {
  TaskEntry {
    name: name.to_owned(),
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
  TaskEntry { goal_id: Some(goal_id.to_owned()), merge_task, ..task(name, status) }
}

pub fn inquiry(id: &str, answered: bool, created_at: DateTime<Utc>) -> InquiryEntry {
  InquiryEntry {
    inquiry_id: id.to_owned(),
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
    goal_id: "g1".to_owned(),
    kind: kind.to_owned(),
    body: format!("通知 {id}"),
    recorded_at,
  }
}

pub fn run(run_slug: &str, task_name: &str, last_log_at: DateTime<Utc>) -> RunEntry {
  RunEntry {
    run_slug:                run_slug.to_owned(),
    session_id:              String::new(),
    task_name:               task_name.to_owned(),
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
    name:   name.to_owned(),
    server: "mac-mini".to_owned(),
    about:  format!("{name} の説明"),
    cwd:    cwd.to_path_buf(),
  }
}

pub fn mcp_server() -> McpServer {
  McpServer { command: "node".to_owned(), args: vec!["server.mjs".to_owned()], env: Default::default() }
}

pub fn thresholds() -> Thresholds {
  Thresholds {
    pending_decision_stale_minutes: 240,
    run_log_silent_minutes:         10,
    connection_failure_count:       3,
    connection_delay_seconds:       120,
  }
}

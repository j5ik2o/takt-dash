//! テストで共有する mock MCP サーバの起動と、model 型のフィクスチャ生成。

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use takt_dash_backend::{
  config::{McpServer, Project, ServerMeta, Thresholds},
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

/// 道具呼び出しの記録先を指定して mock サーバの起動設定を作る。
pub fn mock_config_with_call_log(cwd: &Path, state: &Path, call_log: &Path) -> McpClientConfig {
  let mut config = mock_config(cwd, state, None);
  config.args.push("--call-log".to_owned());
  config.args.push(call_log.display().to_string());
  config
}

/// mock サーバへ接続するクライアントを作る。
pub fn mock_client(config: McpClientConfig) -> McpClient {
  McpClient::new(config)
}

/// mock サーバが記録した道具名の一覧を読む。
pub fn recorded_calls(path: &Path) -> Vec<String> {
  match std::fs::read_to_string(path) {
    | Ok(text) => text.lines().filter(|line| !line.is_empty()).map(str::to_owned).collect(),
    | Err(_) => Vec::new(),
  }
}

/// 道具呼び出しの記録を空にする。
pub fn clear_recorded_calls(path: &Path) {
  std::fs::write(path, "").expect("道具呼び出しの記録を空にできます");
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

/// プロジェクトの cwd の run
/// の集計を作る。読み取り1回ごとに作ると、本体ログは毎回先頭から読みます。
pub fn run_logs(cwd: &Path) -> takt_dash_backend::run_progress::RunLogHandle {
  takt_dash_backend::run_progress::run_log_handle(cwd.to_path_buf())
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
    name:   name.to_owned(),
    server: "mac-mini".to_owned(),
    about:  format!("{name} の説明"),
    cwd:    cwd.to_path_buf(),
  }
}

pub fn mcp_server() -> McpServer {
  McpServer { command: "node".to_owned(), args: vec!["server.mjs".to_owned()], env: Default::default() }
}

/// takt がプロジェクトの `.takt` へ書く run の保存形式（`tasks.yaml` の対応表、`meta.json`、
/// 本体ログ、`reports/`）を一時ディレクトリへ作る。
///
/// 実ホームや実在の `takt-worktrees` は参照せず、run はすべてプロジェクトの `.takt/runs` の
/// 下に置きます。本体ログの record は実物のキー構成（type、step、persona、iteration、status、
/// timestamp、workflow、stack）に合わせた合成データです。
pub struct RunFixture {
  project_cwd: PathBuf,
}

/// `tasks.yaml` の1項目。`source_run_slug` は元 run の解決だけに使う別の値です。
pub struct RunTaskLine<'a> {
  pub run_slug:        Option<&'a str>,
  pub source_run_slug: Option<&'a str>,
}

/// worktree で動く run の `tasks.yaml` の1項目。`worktree_path` は takt が書く絶対パスです。
pub struct WorktreeTaskLine<'a> {
  pub run_slug:        Option<&'a str>,
  pub source_run_slug: Option<&'a str>,
  pub worktree_path:   &'a Path,
}

impl RunFixture {
  /// `worktree_path` を持つ項目だけで `tasks.yaml` の対応表を書く。
  pub fn write_worktree_tasks(&self, entries: &[WorktreeTaskLine<'_>]) {
    let mut yaml = String::from("tasks:\n");
    for entry in entries {
      let mut fields = Vec::new();
      if let Some(run_slug) = entry.run_slug {
        fields.push(format!("run_slug: {run_slug}"));
      }
      if let Some(source) = entry.source_run_slug {
        fields.push(format!("source_run_slug: {source}"));
      }
      fields.push(format!("worktree_path: {}", entry.worktree_path.display()));
      for (index, field) in fields.iter().enumerate() {
        let prefix = if index == 0 { "  - " } else { "    " };
        yaml.push_str(&format!("{prefix}{field}\n"));
      }
    }
    std::fs::write(self.project_cwd.join(".takt").join("tasks.yaml"), yaml).expect("tasks.yaml を書けます");
  }

  /// takt の LiveInterventionFileStore と同じく、プロジェクト側の
  /// `.takt/runs/<run_slug>/interventions.jsonl` へ指示書のイベントを1行ずつ書く。
  pub fn write_interventions(&self, run_slug: &str, events: &[serde_json::Value]) {
    let directory = self.project_cwd.join(".takt").join("runs").join(run_slug);
    std::fs::create_dir_all(&directory).expect("run のディレクトリを作れます");
    let text: String = events.iter().map(|event| format!("{event}\n")).collect();
    std::fs::write(directory.join("interventions.jsonl"), text).expect("interventions.jsonl を書けます");
  }

  pub fn new(project_cwd: &Path) -> RunFixture {
    std::fs::create_dir_all(project_cwd.join(".takt")).expect("プロジェクトの .takt を作れます");
    RunFixture { project_cwd: project_cwd.to_path_buf() }
  }

  /// `tasks.yaml` の対応表を書く。指定した run だけが対応表に載ります。
  pub fn write_tasks(&self, entries: &[RunTaskLine<'_>]) {
    let mut yaml = String::from("tasks:\n");
    for entry in entries {
      let mut fields = Vec::new();
      if let Some(run_slug) = entry.run_slug {
        fields.push(format!("run_slug: {run_slug}"));
      }
      if let Some(source) = entry.source_run_slug {
        fields.push(format!("source_run_slug: {source}"));
      }
      for (index, field) in fields.iter().enumerate() {
        let prefix = if index == 0 { "  - " } else { "    " };
        yaml.push_str(&format!("{prefix}{field}\n"));
      }
    }
    std::fs::write(self.project_cwd.join(".takt").join("tasks.yaml"), yaml).expect("tasks.yaml を書けます");
  }

  /// run のディレクトリ（`logs` を含む）を作って返す。
  pub fn run_dir(&self, run_slug: &str) -> PathBuf {
    let run_dir = self.project_cwd.join(".takt").join("runs").join(run_slug);
    std::fs::create_dir_all(run_dir.join("logs")).expect("run の logs ディレクトリを作れます");
    run_dir
  }

  pub fn write_meta(&self, run_slug: &str, meta: &serde_json::Value) {
    std::fs::write(self.run_dir(run_slug).join("meta.json"), meta.to_string()).expect("meta.json を書けます");
  }

  pub fn write_meta_raw(&self, run_slug: &str, text: &str) {
    std::fs::write(self.run_dir(run_slug).join("meta.json"), text).expect("meta.json を書けます");
  }

  pub fn body_log(&self, run_slug: &str) -> PathBuf {
    self.run_dir(run_slug).join("logs").join("session.jsonl")
  }

  /// 本体ログの末尾へ record を1行ずつ追記する。
  pub fn append_records(&self, run_slug: &str, records: &[serde_json::Value]) {
    use std::io::Write;
    let mut file =
      std::fs::OpenOptions::new().append(true).create(true).open(self.body_log(run_slug)).expect("本体ログを開けます");
    for record in records {
      let mut line = record.to_string();
      line.push('\n');
      file.write_all(line.as_bytes()).expect("本体ログへ追記できます");
    }
  }

  /// 本体ログの既読範囲にある `from` をすべて、同じバイト長の `to` へ書き換える。inode
  /// とファイルの大きさは
  /// 変えないため、読み取り位置を保持する読み取りは書き換えた範囲を読み直しません。
  pub fn overwrite_body_log_in_place(&self, run_slug: &str, from: &str, to: &str) {
    use std::io::{Seek, SeekFrom, Write};
    assert_eq!(from.len(), to.len(), "書き換えの前後でバイト長を揃えます");
    let path = self.body_log(run_slug);
    let text = std::fs::read_to_string(&path).expect("本体ログを読めます");
    let replaced = text.replace(from, to);
    assert_ne!(text, replaced, "書き換え対象 {from} が本体ログにあります");
    let mut file = std::fs::OpenOptions::new().write(true).open(&path).expect("本体ログを開けます");
    file.seek(SeekFrom::Start(0)).expect("先頭へ移動できます");
    file.write_all(replaced.as_bytes()).expect("本体ログを書き換えられます");
  }

  /// 本体ログの更新時刻を固定する。ログ群の最新更新時刻を決定的にするために使います。
  pub fn set_body_log_modified(&self, run_slug: &str, modified: DateTime<Utc>) {
    let file = std::fs::File::options().write(true).open(self.body_log(run_slug)).expect("本体ログを開けます");
    file.set_modified(std::time::SystemTime::from(modified)).expect("更新時刻を設定できます");
  }

  /// `reports/` の下へレポートを書く。`relative` は `/` 区切りの相対パスです。
  pub fn write_report(&self, run_slug: &str, relative: &str, content: &str) {
    let path = self.run_dir(run_slug).join("reports").join(relative);
    std::fs::create_dir_all(path.parent().expect("レポートの親があります")).expect("レポートの置き場を作れます");
    std::fs::write(path, content).expect("レポートを書けます");
  }
}

/// 実行中の run の `meta.json`。
pub fn running_meta(current_step: &str, current_iteration: i64, workflow: &str, start_time: &str) -> serde_json::Value {
  serde_json::json!({
    "workflow": workflow,
    "status": "running",
    "startTime": start_time,
    "currentStep": current_step,
    "currentIteration": current_iteration,
    "phase": 1,
    "updatedAt": start_time
  })
}

pub fn workflow_start_record(workflow: &str) -> serde_json::Value {
  serde_json::json!({ "type": "workflow_start", "workflow": workflow })
}

/// 最上位（stack の長さ 1）の step_complete。
pub fn step_complete_record(step: &str, persona: &str, iteration: i64, timestamp: &str) -> serde_json::Value {
  serde_json::json!({
    "type": "step_complete",
    "step": step,
    "persona": persona,
    "iteration": iteration,
    "status": "done",
    "timestamp": timestamp,
    "workflow": "development-core",
    "stack": [ { "workflow": "development-core", "step": step, "kind": "agent", "occurrence": 1 } ]
  })
}

/// step_start。`stack_steps` は外側から順の stack の step 名で、長さが stack の深さになります。
pub fn step_start_record(
  step: &str,
  workflow: &str,
  iteration: i64,
  timestamp: &str,
  stack_steps: &[&str],
) -> serde_json::Value {
  let stack: Vec<serde_json::Value> = stack_steps
    .iter()
    .map(|stack_step| serde_json::json!({ "workflow": workflow, "step": stack_step, "kind": "agent", "occurrence": 1 }))
    .collect();
  serde_json::json!({
    "type": "step_start",
    "step": step,
    "persona": "coder",
    "iteration": iteration,
    "timestamp": timestamp,
    "workflow": workflow,
    "stack": stack
  })
}

/// takt の `interventions.jsonl` の issued イベント。
pub fn issued_event(instruction_id: i64, issued_at: &str, content: &str) -> serde_json::Value {
  serde_json::json!({ "type": "issued", "instructionId": instruction_id, "issuedAt": issued_at, "content": content })
}

/// takt の `interventions.jsonl` の delivered イベント。`mode` は same_session / next_step /
/// batch_boundary です。
pub fn delivered_event(instruction_ids: &[i64], mode: &str, delivered_at: &str, step: &str) -> serde_json::Value {
  serde_json::json!({
    "type": "delivered",
    "instructionIds": instruction_ids,
    "deliveredAt": delivered_at,
    "mode": mode,
    "step": step,
    "phase": 1
  })
}

/// 集計の対象にならない種類（provider の出来事）の record。本体ログを大きくするために使います。
/// 1件の JSON はおよそ `payload_bytes` バイトです。
pub fn filler_record(index: usize, payload_bytes: usize) -> serde_json::Value {
  serde_json::json!({ "type": "provider_event", "index": index, "data": "x".repeat(payload_bytes) })
}

/// 本体ログの末尾へ、`total_bytes` を超えるまで集計対象外の record を追記する。
pub fn append_filler_records(fixture: &RunFixture, run_slug: &str, total_bytes: u64) {
  use std::io::Write;
  let mut file =
    std::fs::OpenOptions::new().append(true).create(true).open(fixture.body_log(run_slug)).expect("本体ログを開けます");
  let mut written = 0_u64;
  let mut index = 0;
  while written <= total_bytes {
    let mut line = filler_record(index, 4096).to_string();
    line.push('\n');
    file.write_all(line.as_bytes()).expect("本体ログへ追記できます");
    written += line.len() as u64;
    index += 1;
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

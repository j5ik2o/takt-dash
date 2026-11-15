//! takt-mcp の読み取り道具を型付きで呼び出し、応答テキストを model 型へ解析する。
//!
//! 応答は `{<key>:[...], nextOffset, ...}` のページ形式です。nextOffset が null になるまで
//! 追従して全件を集めます。道具が isError を返した場合は [`ToolError::Tool`] として扱い、
//! 輸送エラーとは区別します。

use std::{collections::BTreeMap, path::PathBuf, time::Duration};

use chrono::{DateTime, Utc};
use serde_json::{json, Value};

use super::client::{CallError, McpClient, McpClientConfig};
use crate::{
  config::McpServer,
  model::{
    ExecutionStatus, GoalDetail, GoalEntry, GoalQuestionEntry, GoalStatus, GoalTaskEntry, GoalWorkLogEntry,
    InquiryEntry, InquiryKind, NotificationEntry, ProjectSnapshot, ReportEntry, RunDetail, RunEntry,
    RunInstructionEntry, RunReportEntry, RunStatus, RunStepEntry, StepLogEntry, TaskDetail, TaskEntry, TaskStatus,
  },
};

/// takt-mcp 子プロセスの起動設定を作る。
///
/// poller の定期読み取りと GetRunDetail の要求時読み取りが、同じ起動条件（タイムアウトを含む）を
/// 共有するために公開します。
pub fn client_config(server: &McpServer, cwd: &std::path::Path) -> McpClientConfig {
  McpClientConfig {
    command:         server.command.clone(),
    args:            server.args.clone(),
    env:             server.env.clone(),
    cwd:             cwd.to_path_buf(),
    request_timeout: Duration::from_secs(15),
  }
}

/// 読み取り道具の呼び出しで起きる失敗。
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
  /// 子プロセスの起動失敗・タイムアウト・不正な応答・プロセス死亡を表す輸送エラー。
  #[error(transparent)]
  Transport(#[from] super::client::TransportError),
  /// 道具が isError を返した失敗。子プロセスは生存して応答しているため接続断ではありません。
  #[error(transparent)]
  Tool(#[from] super::client::ToolCallError),
  #[error("takt-mcp の応答を解析できませんでした: {0}")]
  Parse(String),
}

impl From<CallError> for ToolError {
  fn from(error: CallError) -> Self {
    match error {
      | CallError::Transport(error) => ToolError::Transport(error),
      | CallError::Tool(error) => ToolError::Tool(error),
    }
  }
}

/// 1プロジェクト分の状態を読み取るための道具のまとまり。
///
/// takt-mcp の読み取り道具は入力引数に絶対パスの `cwd` を必須とするため、この道具のまとまりは
/// 対象プロジェクトの `cwd` を保持し、すべての tools/call の引数へ渡します。
pub struct ToolSet<'a> {
  client: &'a mut McpClient,
  cwd:    PathBuf,
}

/// ページ形式の一覧を全件集めるための上限。無限追従を避けます。
const MAX_PAGES: usize = 100;

impl<'a> ToolSet<'a> {
  pub fn new(client: &'a mut McpClient, cwd: PathBuf) -> Self {
    ToolSet { client, cwd }
  }

  /// `cwd` を含む tools/call の引数を組み立てる。
  fn arguments(&self, extra: Value) -> Value {
    let mut object = serde_json::Map::new();
    object.insert("cwd".to_owned(), Value::String(self.cwd.display().to_string()));
    if let Value::Object(extra) = extra {
      object.extend(extra);
    }
    Value::Object(object)
  }

  /// ゴール・タスク・問い合わせ・run を読み取り、1プロジェクト分の snapshot を組み立てる。
  ///
  /// `prev` には前回の snapshot を渡します。補助読み取り（run・ゴール質問・通知・ゴールに
  /// 属さない問い合わせ）が道具エラーで失敗したときは、その項目だけ前回値（無ければ空）を
  /// 保持し、プロジェクト全体を失敗にしません。コア読み取り（ゴール・タスク）の失敗と、
  /// 補助読み取りの輸送エラー・解析エラーは全体の失敗として伝播します。
  ///
  /// ゴールのマージ待ちの起点時刻は takt_get_goal の通知から得るため、通知を読んだ後に
  /// ゴールへ起点時刻を反映します。
  pub async fn read_snapshot(&mut self, prev: Option<&ProjectSnapshot>) -> Result<ProjectSnapshot, ToolError> {
    let observed_at = Utc::now();
    let mut goals = self.list_goals().await?;
    let tasks = self.list_tasks().await?;
    let inquiries = self.list_all_inquiries(&goals, prev, observed_at).await?;
    let (notifications, awaiting_merge_at) = self.list_notifications(&goals, prev).await?;
    for goal in &mut goals {
      if let Some(recorded_at) = awaiting_merge_at.get(&goal.goal_id) {
        goal.awaiting_merge_at = Some(*recorded_at);
      }
    }
    let runs = self.get_running_runs(&tasks, prev).await?;
    Ok(ProjectSnapshot { goals, tasks, inquiries, runs, notifications })
  }

  async fn call_json(&mut self, name: &str, extra: Value) -> Result<Value, ToolError> {
    let arguments = self.arguments(extra);
    let text = self.client.call_tool(name, arguments).await?;
    serde_json::from_str(&text).map_err(|error| ToolError::Parse(format!("{name}: {error}")))
  }

  async fn list_goals(&mut self) -> Result<Vec<GoalEntry>, ToolError> {
    let mut entries = Vec::new();
    let mut offset = 0;
    for _ in 0..MAX_PAGES {
      let value = self.call_json("takt_list_goals", json!({ "offset": offset })).await?;
      let page = Page::from_value(&value, "goals")?;
      for item in &page.items {
        entries.push(parse_goal(item)?);
      }
      match page.next_offset {
        | Some(next) => offset = next,
        | None => return Ok(entries),
      }
    }
    Err(ToolError::Parse("takt_list_goals のページ追従が上限に達しました".to_owned()))
  }

  async fn list_tasks(&mut self) -> Result<Vec<TaskEntry>, ToolError> {
    let mut entries = Vec::new();
    let mut offset = 0;
    for _ in 0..MAX_PAGES {
      let value = self.call_json("takt_list_tasks", json!({ "offset": offset })).await?;
      let page = Page::from_value(&value, "tasks")?;
      for item in &page.items {
        entries.push(parse_task(item)?);
      }
      match page.next_offset {
        | Some(next) => offset = next,
        | None => return Ok(entries),
      }
    }
    Err(ToolError::Parse("takt_list_tasks のページ追従が上限に達しました".to_owned()))
  }

  /// ゴールに属する問い合わせと、ゴールに属さない問い合わせをまとめて集める。
  ///
  /// ゴール質問の取得は行使済みの goals を再利用します（ゴール一覧の再取得はしません）。
  /// 道具エラー（そのゴールの質問だけ取得できない、またはゴールに属さない問い合わせだけ
  /// 取得できない）は、該当項目だけ前回 snapshot の同じゴールの問い合わせを引き継ぎます
  /// （初回観測時は空）。輸送エラーと解析エラーは全体の失敗として伝播します。
  async fn list_all_inquiries(
    &mut self,
    goals: &[GoalEntry],
    prev: Option<&ProjectSnapshot>,
    observed_at: DateTime<Utc>,
  ) -> Result<Vec<InquiryEntry>, ToolError> {
    let mut inquiries = match self.list_standalone_inquiries().await {
      | Ok(entries) => entries,
      | Err(ToolError::Tool(_)) => prev_inquiries(prev, |goal_id| goal_id.is_none()),
      | Err(error) => return Err(error),
    };
    for goal in goals {
      match self.list_goal_questions(&goal.goal_id).await {
        | Ok(mut entries) => inquiries.append(&mut entries),
        | Err(ToolError::Tool(_)) => {
          inquiries.extend(prev_inquiries(prev, |goal_id| goal_id == Some(goal.goal_id.as_str())))
        },
        | Err(error) => return Err(error),
      }
    }
    Ok(inquiries.into_iter().map(|parsed| resolve_inquiry(parsed, prev, observed_at)).collect())
  }

  async fn list_goal_questions(&mut self, goal_id: &str) -> Result<Vec<ParsedInquiry>, ToolError> {
    let mut entries = Vec::new();
    let mut offset = 0;
    for _ in 0..MAX_PAGES {
      let value = self.call_json("takt_list_goal_questions", json!({ "goalId": goal_id, "offset": offset })).await?;
      let page = Page::from_value(&value, "questions")?;
      for item in &page.items {
        if let Some(entry) = parse_inquiry(item, Some(goal_id))? {
          entries.push(entry);
        }
      }
      match page.next_offset {
        | Some(next) => offset = next,
        | None => return Ok(entries),
      }
    }
    Err(ToolError::Parse("takt_list_goal_questions のページ追従が上限に達しました".to_owned()))
  }

  async fn list_standalone_inquiries(&mut self) -> Result<Vec<ParsedInquiry>, ToolError> {
    let mut entries = Vec::new();
    let mut offset = 0;
    for _ in 0..MAX_PAGES {
      let value = self.call_json("takt_list_standalone_inquiries", json!({ "offset": offset })).await?;
      let page = Page::from_value(&value, "inquiries")?;
      for item in &page.items {
        if let Some(entry) = parse_inquiry(item, None)? {
          entries.push(entry);
        }
      }
      match page.next_offset {
        | Some(next) => offset = next,
        | None => return Ok(entries),
      }
    }
    Err(ToolError::Parse("takt_list_standalone_inquiries のページ追従が上限に達しました".to_owned()))
  }

  /// running なタスクの run を集める。`takt_get_run` が道具エラーを返した run だけ、前回
  /// snapshot の同じ runSlug を引き継ぎ（無ければ欠落）、健常な run は今回の値を採ります。
  /// 輸送エラーと解析エラーは全体の失敗として伝播します。
  async fn get_running_runs(
    &mut self,
    tasks: &[TaskEntry],
    prev: Option<&ProjectSnapshot>,
  ) -> Result<Vec<RunEntry>, ToolError> {
    let mut runs = Vec::new();
    for task in tasks.iter().filter(|task| task.status == TaskStatus::Running) {
      let Some(run_slug) = task.run_slug.clone() else {
        continue;
      };
      match self.read_run(&run_slug, task).await {
        | Ok(run) => runs.push(run),
        | Err(ToolError::Tool(_)) => {
          if let Some(previous) =
            prev.and_then(|snapshot| snapshot.runs.iter().find(|entry| entry.run_slug == run_slug))
          {
            runs.push(previous.clone());
          }
        },
        | Err(error) => return Err(error),
      }
    }
    Ok(runs)
  }

  async fn read_run(&mut self, run_slug: &str, task: &TaskEntry) -> Result<RunEntry, ToolError> {
    let value = self.call_json("takt_get_run", json!({ "runSlug": run_slug })).await?;
    self.parse_run_with_pages(&value, task).await
  }

  /// takt_get_run の応答を解析する。stepLogs は先頭 50 件のウィンドウで返るため、
  /// stepLogPage.nextOffset が残っていれば追加ページを取得して末尾のログの時刻を求める。
  async fn parse_run_with_pages(&mut self, value: &Value, task: &TaskEntry) -> Result<RunEntry, ToolError> {
    let mut latest_seen = last_step_timestamp(value);
    let mut steps = step_entries(value);
    let mut next_offset = step_log_next_offset(value);
    let run_slug = string_field(value, "runSlug")?;
    let mut pages_taken = 0;
    while let Some(offset) = next_offset {
      if pages_taken >= MAX_PAGES {
        return Err(ToolError::Parse("takt_get_run の stepLogs 追従が上限に達しました".to_owned()));
      }
      pages_taken += 1;
      let page_value = self.call_json("takt_get_run", json!({ "runSlug": run_slug, "offset": offset })).await?;
      if let Some(timestamp) = last_step_timestamp(&page_value) {
        latest_seen = Some(timestamp);
      }
      steps.extend(step_entries(&page_value));
      next_offset = step_log_next_offset(&page_value);
    }
    parse_run(value, task, latest_seen, steps)
  }

  async fn list_notifications(
    &mut self,
    goals: &[GoalEntry],
    prev: Option<&ProjectSnapshot>,
  ) -> Result<(Vec<NotificationEntry>, BTreeMap<String, DateTime<Utc>>), ToolError> {
    let mut notifications = Vec::new();
    let mut awaiting_merge_at = BTreeMap::new();
    for goal in goals {
      match self.list_goal_notifications(&goal.goal_id).await {
        | Ok((goal_notifications, goal_awaiting_merge_at)) => {
          if let Some(recorded_at) = goal_awaiting_merge_at {
            awaiting_merge_at.insert(goal.goal_id.clone(), recorded_at);
          }
          notifications.extend(goal_notifications);
        },
        | Err(ToolError::Tool(_)) => {
          let goal_id = goal.goal_id.as_str();
          if let Some(prev) = prev {
            notifications.extend(prev.notifications.iter().filter(|entry| entry.goal_id == goal_id).cloned());
            if let Some(recorded_at) =
              prev.goals.iter().find(|entry| entry.goal_id == goal_id).and_then(|entry| entry.awaiting_merge_at)
            {
              awaiting_merge_at.insert(goal.goal_id.clone(), recorded_at);
            }
          }
        },
        | Err(error) => return Err(error),
      }
    }
    Ok((notifications, awaiting_merge_at))
  }

  /// 1 ゴールの通知を nextOffset が null になるまで追従して集める。あわせて、そのゴールの
  /// awaiting_merge 通知のうち最も新しい recordedAt を返す。
  async fn list_goal_notifications(
    &mut self,
    goal_id: &str,
  ) -> Result<(Vec<NotificationEntry>, Option<DateTime<Utc>>), ToolError> {
    let mut notifications = Vec::new();
    let mut awaiting_merge_at: Option<DateTime<Utc>> = None;
    let mut offset = 0;
    for _ in 0..MAX_PAGES {
      let value = self
        .call_json("takt_get_goal", json!({ "goalId": goal_id, "path": "notifications", "offset": offset }))
        .await?;
      let page = Page::from_value(&value, "items")?;
      for item in &page.items {
        let notification = parse_notification(goal_id, item)?;
        if notification.kind == "awaiting_merge"
          && awaiting_merge_at.is_none_or(|current| notification.recorded_at > current)
        {
          awaiting_merge_at = Some(notification.recorded_at);
        }
        notifications.push(notification);
      }
      match page.next_offset {
        | Some(next) => offset = next,
        | None => return Ok((notifications, awaiting_merge_at)),
      }
    }
    Err(ToolError::Parse("takt_get_goal の notices 追従が上限に達しました".to_owned()))
  }
}

/// 前回 snapshot の問い合わせのうち、`matches` に合うものを解析済みの形で取り出す。
/// 道具エラー時に該当項目だけを引き継ぐために使います。前回が無ければ空です。
fn prev_inquiries(prev: Option<&ProjectSnapshot>, matches: impl Fn(Option<&str>) -> bool) -> Vec<ParsedInquiry> {
  let Some(prev) = prev else {
    return Vec::new();
  };
  prev
    .inquiries
    .iter()
    .filter(|entry| matches(entry.goal_id.as_deref()))
    .map(|entry| ParsedInquiry {
      inquiry_id: entry.inquiry_id.clone(),
      goal_id:    entry.goal_id.clone(),
      kind:       entry.kind,
      body:       entry.body.clone(),
      options:    entry.options.clone(),
      answered:   entry.answered,
      created_at: Some(entry.created_at),
    })
    .collect()
}

struct Page {
  items:       Vec<Value>,
  next_offset: Option<usize>,
}

impl Page {
  fn from_value(value: &Value, key: &str) -> Result<Self, ToolError> {
    let items = value
      .get(key)
      .and_then(Value::as_array)
      .ok_or_else(|| ToolError::Parse(format!("応答に {key} の配列がありません")))?
      .clone();
    let next_offset = value.get("nextOffset").and_then(Value::as_u64).map(|value| value as usize);
    Ok(Page { items, next_offset })
  }
}

fn string_field(value: &Value, key: &str) -> Result<String, ToolError> {
  value
    .get(key)
    .and_then(Value::as_str)
    .map(str::to_owned)
    .ok_or_else(|| ToolError::Parse(format!("{key} が文字列ではありません")))
}

fn optional_string(value: &Value, key: &str) -> Option<String> {
  value.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn parse_time(value: Option<&Value>) -> Result<Option<DateTime<Utc>>, ToolError> {
  match value {
    | None | Some(Value::Null) => Ok(None),
    | Some(Value::String(text)) => DateTime::parse_from_rfc3339(text)
      .map(|time| Some(time.with_timezone(&Utc)))
      .map_err(|error| ToolError::Parse(format!("時刻 {text} を解析できませんでした: {error}"))),
    | Some(_) => Err(ToolError::Parse("時刻が文字列ではありません".to_owned())),
  }
}

fn parse_goal_status(value: Option<&Value>) -> Result<GoalStatus, ToolError> {
  match value.and_then(Value::as_str) {
    | Some("created") => Ok(GoalStatus::Created),
    | Some("awaiting_merge") => Ok(GoalStatus::AwaitingMerge),
    | Some("completed") => Ok(GoalStatus::Completed),
    | other => Err(ToolError::Parse(format!("未知のゴールの状態です: {other:?}"))),
  }
}

fn parse_goal_execution_status(value: Option<&Value>) -> Result<ExecutionStatus, ToolError> {
  match value.and_then(Value::as_str) {
    | Some("active") => Ok(ExecutionStatus::Active),
    | Some("paused") => Ok(ExecutionStatus::Paused),
    | Some("aborted") => Ok(ExecutionStatus::Aborted),
    | other => Err(ToolError::Parse(format!("未知のゴールの実行状態です: {other:?}"))),
  }
}

fn parse_task_status(value: Option<&Value>) -> Result<TaskStatus, ToolError> {
  match value.and_then(Value::as_str) {
    | Some("pending") => Ok(TaskStatus::Pending),
    | Some("running") => Ok(TaskStatus::Running),
    | Some("completed") => Ok(TaskStatus::Completed),
    | Some("failed") => Ok(TaskStatus::Failed),
    | Some("exceeded") => Ok(TaskStatus::Exceeded),
    | Some("pr_failed") => Ok(TaskStatus::PrFailed),
    | other => Err(ToolError::Parse(format!("未知のタスクの状態です: {other:?}"))),
  }
}

fn parse_goal(value: &Value) -> Result<GoalEntry, ToolError> {
  let goal_id = string_field(value, "id")?;
  let status = parse_goal_status(value.get("status"))?;
  let execution_status = parse_goal_execution_status(value.get("executionStatus"))?;
  let pull_request = value.get("pullRequest");
  let pull_request_number = pull_request.and_then(|value| value.get("number")).and_then(Value::as_i64);
  let pull_request_url = pull_request.and_then(|value| value.get("url")).and_then(Value::as_str).map(str::to_owned);
  Ok(GoalEntry {
    goal_id,
    status,
    execution_status,
    objective: optional_string(value, "objective").unwrap_or_default(),
    integration_branch: optional_string(value, "integrationBranch").unwrap_or_default(),
    pull_request_number,
    pull_request_url,
    awaiting_merge_at: None,
    paused_reason: optional_string(value, "pausedReason"),
  })
}

fn parse_task(value: &Value) -> Result<TaskEntry, ToolError> {
  let status = parse_task_status(value.get("status"))?;
  let started_at = parse_time(value.get("startedAt"))?;
  Ok(TaskEntry {
    name: string_field(value, "name")?,
    goal_id: optional_string(value, "goalId"),
    status,
    started_at,
    run_slug: optional_string(value, "runSlug"),
    current_step: optional_string(value, "currentStep"),
    merge_task: value.get("mergeTask").and_then(Value::as_bool).unwrap_or(false),
  })
}

fn parse_goal_work_log(value: &Value) -> Result<GoalWorkLogEntry, ToolError> {
  let recorded_at = parse_time(value.get("recordedAt"))?
    .ok_or_else(|| ToolError::Parse("作業記録の recordedAt がありません".to_owned()))?;
  Ok(GoalWorkLogEntry {
    work_log_id: string_field(value, "id")?,
    recorded_at,
    summary: optional_string(value, "summary").unwrap_or_default(),
  })
}

fn parse_goal_question(value: &Value) -> Result<GoalQuestionEntry, ToolError> {
  let answer = value.get("answer");
  let options = value
    .get("options")
    .and_then(Value::as_array)
    .map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect())
    .unwrap_or_default();
  Ok(GoalQuestionEntry {
    question_id: string_field(value, "id")?,
    kind: optional_string(value, "kind").unwrap_or_default(),
    status: optional_string(value, "status").unwrap_or_default(),
    body: optional_string(value, "body").unwrap_or_default(),
    options,
    answer_text: answer.and_then(|answer| optional_string(answer, "text")),
    answer_decision: answer.and_then(|answer| optional_string(answer, "decision")),
    answer_report: answer.and_then(|answer| optional_string(answer, "report")),
    created_at: parse_time(value.get("createdAt"))?,
    answered_at: answer.map(|answer| parse_time(answer.get("answeredAt"))).transpose()?.flatten(),
  })
}

fn parse_task_detail(project: &str, value: &Value) -> Result<TaskDetail, ToolError> {
  Ok(TaskDetail {
    project:             project.to_owned(),
    task_name:           string_field(value, "name")?,
    goal_id:             optional_string(value, "goalId"),
    status:              parse_task_status(value.get("status"))?,
    summary:             optional_string(value, "summary").unwrap_or_default(),
    started_at:          parse_time(value.get("startedAt"))?,
    run_slug:            optional_string(value, "runSlug"),
    pull_request_number: value.get("prNumber").and_then(Value::as_i64),
  })
}

/// 解析直後の問い合わせ。`created_at` は応答での有無を保持し、前回値の引き継ぎと
/// 初回観測の既定は `read_snapshot` の解像で決めます。
struct ParsedInquiry {
  inquiry_id: String,
  goal_id:    Option<String>,
  kind:       InquiryKind,
  body:       String,
  options:    Vec<String>,
  answered:   bool,
  created_at: Option<DateTime<Utc>>,
}

/// 1件の問い合わせを解析する。`approval` は判断待ちの種類（問い合わせとマージ待ち）に
/// 対応する値が無いため、判定対象外として `Ok(None)` を返して読み飛ばします。未知の種類は
/// 解析エラーにします。
fn parse_inquiry(value: &Value, goal_id: Option<&str>) -> Result<Option<ParsedInquiry>, ToolError> {
  let kind = match value.get("kind").and_then(Value::as_str) {
    | Some("question") => InquiryKind::Question,
    | Some("operation_request") => InquiryKind::OperationRequest,
    | Some("approval") => return Ok(None),
    | other => return Err(ToolError::Parse(format!("未知の問い合わせの種類です: {other:?}"))),
  };
  let answered = match value.get("status").and_then(Value::as_str) {
    | Some("pending") => false,
    | Some("answered") | Some("withdrawn") => true,
    | other => return Err(ToolError::Parse(format!("未知の問い合わせの状態です: {other:?}"))),
  };
  let options = value
    .get("options")
    .and_then(Value::as_array)
    .map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect())
    .unwrap_or_default();
  Ok(Some(ParsedInquiry {
    inquiry_id: string_field(value, "id")?,
    goal_id: goal_id.map(str::to_owned).or_else(|| optional_string(value, "goalId")),
    kind,
    body: optional_string(value, "body").unwrap_or_default(),
    options,
    answered,
    created_at: parse_time(value.get("createdAt"))?,
  }))
}

/// 解析済みの問い合わせを、`created_at` を解像して model 型へ変換する。
///
/// 応答に `createdAt` があればそれを使います。無い場合は、前回 snapshot の同一 inquiry_id の
/// created_at を引き継ぎ、初回観測なら読み取り時刻を既定にします。都度読み取り時刻へ既定化して
/// 滞留判定が毎サイクルリセットされることを避けます。
fn resolve_inquiry(parsed: ParsedInquiry, prev: Option<&ProjectSnapshot>, observed_at: DateTime<Utc>) -> InquiryEntry {
  let created_at = parsed
    .created_at
    .or_else(|| {
      prev.and_then(|snapshot| {
        snapshot.inquiries.iter().find(|entry| entry.inquiry_id == parsed.inquiry_id).map(|entry| entry.created_at)
      })
    })
    .unwrap_or(observed_at);
  InquiryEntry {
    inquiry_id: parsed.inquiry_id,
    goal_id: parsed.goal_id,
    kind: parsed.kind,
    body: parsed.body,
    options: parsed.options,
    answered: parsed.answered,
    created_at,
  }
}

fn parse_notification(goal_id: &str, value: &Value) -> Result<NotificationEntry, ToolError> {
  let recorded_at = parse_time(value.get("recordedAt"))?
    .ok_or_else(|| ToolError::Parse("通知の recordedAt がありません".to_owned()))?;
  Ok(NotificationEntry {
    notification_id: string_field(value, "id")?,
    goal_id: goal_id.to_owned(),
    kind: string_field(value, "kind")?,
    body: optional_string(value, "body").unwrap_or_default(),
    recorded_at,
  })
}

fn parse_run(
  value: &Value,
  task: &TaskEntry,
  last_step_timestamp: Option<DateTime<Utc>>,
  steps: Vec<RunStepEntry>,
) -> Result<RunEntry, ToolError> {
  let run_slug = string_field(value, "runSlug")?;
  let started_at = parse_time(value.get("startedAt"))?
    .or(task.started_at)
    .ok_or_else(|| ToolError::Parse("run の開始時刻を取得できません".to_owned()))?;
  let current_step_started_at = parse_time(value.get("currentStepStartedAt"))?;
  let last_log_at = last_step_timestamp.or(current_step_started_at).unwrap_or(started_at);
  Ok(RunEntry {
    run_slug,
    session_id: optional_string(value, "sessionId").unwrap_or_default(),
    task_name: task.name.clone(),
    goal_id: task.goal_id.clone(),
    summary: optional_string(value, "summary").unwrap_or_default(),
    status: parse_run_status(value.get("status")),
    current_step: optional_string(value, "currentStep"),
    started_at,
    current_step_started_at,
    last_log_at,
    steps,
    reports: report_summaries(value),
  })
}

/// run の状態を寛容に解析する。欠落と未知の値は継続中として扱います。
fn parse_run_status(value: Option<&Value>) -> RunStatus {
  match value.and_then(Value::as_str) {
    | Some("completed") => RunStatus::Completed,
    | Some("aborted") => RunStatus::Aborted,
    | Some("failed") => RunStatus::Failed,
    | _ => RunStatus::Running,
  }
}

/// 応答の stepLogs 配列の末尾の timestamp を取り出す。先頭ページのウィンドウ末尾の時刻です。
fn last_step_timestamp(value: &Value) -> Option<DateTime<Utc>> {
  let timestamp = value
    .get("stepLogs")
    .and_then(Value::as_array)
    .and_then(|logs| logs.last())
    .and_then(|log| log.get("timestamp"))?;
  DateTime::parse_from_rfc3339(timestamp.as_str()?).ok().map(|time| time.with_timezone(&Utc))
}

/// 応答の stepLogs 配列からステップの記録を出現順に取り出す。
///
/// 実 takt-mcp の応答で欠ける項目を解析エラーにしないため、開始時刻は `startedAt` を優先し、
/// 無ければ `timestamp` で補い、どちらも無ければ `UNIX_EPOCH` に落とします。開始時刻を run の
/// 開始時刻で補うことはしません（run の開始時刻は step の開始より前になり得るため）。
fn step_entries(value: &Value) -> Vec<RunStepEntry> {
  let Some(logs) = value.get("stepLogs").and_then(Value::as_array) else {
    return Vec::new();
  };
  let mut steps = Vec::new();
  for log in logs {
    let step_name = optional_string(log, "step").unwrap_or_default();
    let started_at =
      parse_time(log.get("startedAt")).ok().flatten().or_else(|| parse_time(log.get("timestamp")).ok().flatten());
    let ended_at = parse_time(log.get("endedAt")).ok().flatten();
    steps.push(RunStepEntry {
      step_name,
      model: optional_string(log, "model").unwrap_or_default(),
      started_at: started_at.unwrap_or(DateTime::<Utc>::UNIX_EPOCH),
      ended_at,
    });
  }
  steps
}

/// 応答の reports 配列からレポートの要約を出現順に取り出す。
fn report_summaries(value: &Value) -> Vec<ReportEntry> {
  value
    .get("reports")
    .and_then(Value::as_array)
    .map(|reports| {
      reports
        .iter()
        .map(|report| ReportEntry {
          filename:   optional_string(report, "filename").unwrap_or_default(),
          size_bytes: report.get("sizeBytes").and_then(Value::as_i64).unwrap_or_default(),
        })
        .collect()
    })
    .unwrap_or_default()
}

/// stepLogPage.nextOffset を取り出す。null または欠落なら None（追従終端）。
fn step_log_next_offset(value: &Value) -> Option<usize> {
  value.get("stepLogPage").and_then(|page| page.get("nextOffset")).and_then(Value::as_u64).map(|offset| offset as usize)
}

impl<'a> ToolSet<'a> {
  /// 指定した goalId の詳細を takt_get_goal と補助道具から読み取る。
  ///
  /// 目的・対象外・受け入れ条件・制約・仕様は takt_get_goal の path/textOffset を全ページ追従して
  /// 全文を集めます。作業記録と質問は専用の一覧道具を全ページ追従します。紐づくタスクは
  /// takt_list_tasks を全ページ取得して goal_id で絞ります。
  pub async fn read_goal_detail(&mut self, goal_id: &str) -> Result<GoalDetail, ToolError> {
    let value = self.call_json("takt_get_goal", json!({ "goalId": goal_id })).await?;
    let goal =
      value.get("goal").ok_or_else(|| ToolError::Parse("takt_get_goal の応答に goal がありません".to_owned()))?;
    let returned_id = string_field(goal, "id")?;
    if returned_id != goal_id {
      return Err(ToolError::Parse(format!("要求したゴール {goal_id} と応答のゴール {returned_id} が一致しません")));
    }
    let status = parse_goal_status(goal.get("status"))?;
    let execution_status = parse_goal_execution_status(goal.get("executionStatus"))?;
    let branch = optional_string(goal, "branch").unwrap_or_default();
    let integration_branch = optional_string(goal, "integrationBranch").unwrap_or_default();
    let objective = self.read_goal_text(goal_id, "objective").await?;
    let out_of_scope = self.read_goal_array(goal_id, "outOfScope").await?;
    let acceptance_criteria = self.read_goal_array(goal_id, "acceptanceCriteria").await?;
    let constraints = self.read_goal_array(goal_id, "constraints").await?;
    let mut specifications = BTreeMap::new();
    if let Some(notations) =
      goal.get("omissions").and_then(|value| value.get("specifications")).and_then(Value::as_object)
    {
      for notation in notations.keys() {
        let content = self.read_goal_text(goal_id, &format!("specifications.{notation}")).await?;
        specifications.insert(notation.clone(), content);
      }
    }
    let work_log = self.read_goal_work_log(goal_id).await?;
    let questions = self.read_goal_questions(goal_id).await?;
    let tasks = self
      .list_tasks()
      .await?
      .into_iter()
      .filter(|task| task.goal_id.as_deref() == Some(goal_id))
      .map(|task| GoalTaskEntry { task_name: task.name, status: task.status, run_slug: task.run_slug })
      .collect();
    Ok(GoalDetail {
      goal_id: goal_id.to_owned(),
      status,
      execution_status,
      objective,
      out_of_scope,
      acceptance_criteria,
      constraints,
      specifications,
      branch,
      integration_branch,
      work_log,
      questions,
      tasks,
    })
  }

  /// takt_get_goal の文字列フィールドを textOffset で全ページ連結する。
  async fn read_goal_text(&mut self, goal_id: &str, path: &str) -> Result<String, ToolError> {
    let mut content = String::new();
    let mut offset = 0;
    for _ in 0..MAX_PAGES {
      let value =
        self.call_json("takt_get_goal", json!({ "goalId": goal_id, "path": path, "textOffset": offset })).await?;
      let chunk = value
        .get("content")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::Parse(format!("{path} の応答に content がありません")))?;
      content.push_str(chunk);
      match value.get("nextOffset").and_then(Value::as_u64) {
        | Some(next) => offset = next,
        | None => return Ok(content),
      }
    }
    Err(ToolError::Parse(format!("takt_get_goal の {path} の追従が上限に達しました")))
  }

  /// takt_get_goal の配列フィールドを offset で全ページ集める。
  async fn read_goal_array(&mut self, goal_id: &str, path: &str) -> Result<Vec<String>, ToolError> {
    let mut entries = Vec::new();
    let mut offset = 0;
    for _ in 0..MAX_PAGES {
      let value = self.call_json("takt_get_goal", json!({ "goalId": goal_id, "path": path, "offset": offset })).await?;
      let page = Page::from_value(&value, "items")?;
      for item in &page.items {
        let text = item.as_str().ok_or_else(|| ToolError::Parse(format!("{path} の要素が文字列ではありません")))?;
        entries.push(text.to_owned());
      }
      match page.next_offset {
        | Some(next) => offset = next,
        | None => return Ok(entries),
      }
    }
    Err(ToolError::Parse(format!("takt_get_goal の {path} の追従が上限に達しました")))
  }

  /// takt_list_goal_work_log を全ページ追従して作業記録を集める。
  async fn read_goal_work_log(&mut self, goal_id: &str) -> Result<Vec<GoalWorkLogEntry>, ToolError> {
    let mut entries = Vec::new();
    let mut offset = 0;
    for _ in 0..MAX_PAGES {
      let value = self.call_json("takt_list_goal_work_log", json!({ "goalId": goal_id, "offset": offset })).await?;
      let page = Page::from_value(&value, "workLog")?;
      for item in &page.items {
        entries.push(parse_goal_work_log(item)?);
      }
      match page.next_offset {
        | Some(next) => offset = next,
        | None => return Ok(entries),
      }
    }
    Err(ToolError::Parse("takt_list_goal_work_log のページ追従が上限に達しました".to_owned()))
  }

  /// takt_list_goal_questions を全ページ追従して質問を集める。
  async fn read_goal_questions(&mut self, goal_id: &str) -> Result<Vec<GoalQuestionEntry>, ToolError> {
    let mut entries = Vec::new();
    let mut offset = 0;
    for _ in 0..MAX_PAGES {
      let value = self.call_json("takt_list_goal_questions", json!({ "goalId": goal_id, "offset": offset })).await?;
      let page = Page::from_value(&value, "questions")?;
      for item in &page.items {
        entries.push(parse_goal_question(item)?);
      }
      match page.next_offset {
        | Some(next) => offset = next,
        | None => return Ok(entries),
      }
    }
    Err(ToolError::Parse("takt_list_goal_questions のページ追従が上限に達しました".to_owned()))
  }

  /// takt_list_tasks を全ページ取得し、タスク名（と任意の
  /// goalId）が一致する最初のタスク詳細を返す。
  ///
  /// 一致するタスクが無ければ Ok(None)、ページ追従が上限に達したら Parse エラーにします。
  pub async fn read_task_detail(
    &mut self,
    project: &str,
    task_name: &str,
    goal_id: Option<&str>,
  ) -> Result<Option<TaskDetail>, ToolError> {
    let mut offset = 0;
    for _ in 0..MAX_PAGES {
      let value = self.call_json("takt_list_tasks", json!({ "offset": offset })).await?;
      let page = Page::from_value(&value, "tasks")?;
      for item in &page.items {
        if item.get("name").and_then(Value::as_str) != Some(task_name) {
          continue;
        }
        if let Some(goal_id) = goal_id {
          if item.get("goalId").and_then(Value::as_str) != Some(goal_id) {
            continue;
          }
        }
        return Ok(Some(parse_task_detail(project, item)?));
      }
      match page.next_offset {
        | Some(next) => offset = next,
        | None => return Ok(None),
      }
    }
    Err(ToolError::Parse("takt_list_tasks のページ追従が上限に達しました".to_owned()))
  }

  /// 指定した runSlug の詳細を takt_get_run から読み取る。
  ///
  /// stepLogs は先頭 50 件のウィンドウで返るため、stepLogPage.nextOffset が残っていれば追加ページを
  /// 取得して全件を集めます。レポートと指示書の本文は一覧に含まれないため、一覧の filename と
  /// instructionId を使って1件ずつ本文を取得します。
  pub async fn read_run_detail(&mut self, run_slug: &str) -> Result<RunDetail, ToolError> {
    let value = self.call_json("takt_get_run", json!({ "runSlug": run_slug })).await?;
    let task = self.list_tasks().await?.into_iter().find(|task| task.run_slug.as_deref() == Some(run_slug));
    let mut step_logs = step_log_entries(&value);
    let mut next_offset = step_log_next_offset(&value);
    let mut pages_taken = 0;
    while let Some(offset) = next_offset {
      if pages_taken >= MAX_PAGES {
        return Err(ToolError::Parse("takt_get_run の stepLogs 追従が上限に達しました".to_owned()));
      }
      pages_taken += 1;
      let page_value = self.call_json("takt_get_run", json!({ "runSlug": run_slug, "offset": offset })).await?;
      step_logs.extend(step_log_entries(&page_value));
      next_offset = step_log_next_offset(&page_value);
    }
    let mut reports = Vec::new();
    for summary in report_summaries(&value) {
      let report_value =
        self.call_json("takt_get_run", json!({ "runSlug": run_slug, "report": summary.filename })).await?;
      reports.push(RunReportEntry {
        filename:   summary.filename.clone(),
        content:    optional_string(&report_value, "content").unwrap_or_default(),
        size_bytes: report_value.get("sizeBytes").and_then(Value::as_i64).unwrap_or(summary.size_bytes),
      });
    }
    let mut instructions = Vec::new();
    for instruction in value.get("instructions").and_then(Value::as_array).cloned().unwrap_or_default() {
      let instruction_id = instruction.get("instructionId").and_then(Value::as_i64).unwrap_or_default() as i32;
      let instruction_value =
        self.call_json("takt_get_run", json!({ "runSlug": run_slug, "instructionId": instruction_id })).await?;
      instructions.push(RunInstructionEntry {
        instruction_id,
        issued_at: parse_time(instruction_value.get("issuedAt"))
          .ok()
          .flatten()
          .or_else(|| parse_time(instruction.get("issuedAt")).ok().flatten())
          .unwrap_or(DateTime::<Utc>::UNIX_EPOCH),
        state: optional_string(&instruction_value, "state")
          .or_else(|| optional_string(&instruction, "state"))
          .unwrap_or_default(),
        content: optional_string(&instruction_value, "content").unwrap_or_default(),
      });
    }
    parse_run_detail(&value, run_slug, task.as_ref(), step_logs, reports, instructions)
  }
}

fn parse_run_detail(
  value: &Value,
  run_slug: &str,
  task: Option<&TaskEntry>,
  step_logs: Vec<StepLogEntry>,
  reports: Vec<RunReportEntry>,
  instructions: Vec<RunInstructionEntry>,
) -> Result<RunDetail, ToolError> {
  let started_at =
    parse_time(value.get("startedAt"))?.ok_or_else(|| ToolError::Parse("run の開始時刻を取得できません".to_owned()))?;
  let current_step_started_at = parse_time(value.get("currentStepStartedAt"))?;
  let last_log_at = parse_time(value.get("lastLogAt"))?.or(current_step_started_at).unwrap_or(started_at);
  let task_name = optional_string(value, "taskName").or_else(|| task.map(|task| task.name.clone())).unwrap_or_default();
  let goal_id = optional_string(value, "goalId").or_else(|| task.and_then(|task| task.goal_id.clone()));
  Ok(RunDetail {
    run_slug: run_slug.to_owned(),
    session_id: optional_string(value, "sessionId").unwrap_or_default(),
    task_name,
    goal_id,
    task_text: optional_string(value, "taskText").unwrap_or_default(),
    summary: optional_string(value, "summary").unwrap_or_default(),
    workflow: optional_string(value, "workflow").unwrap_or_default(),
    status: parse_run_status(value.get("status")),
    started_at,
    current_step: optional_string(value, "currentStep"),
    current_iteration: value.get("currentIteration").and_then(Value::as_i64).map(|value| value as i32),
    current_step_started_at,
    current_step_execution_count: value
      .get("currentStepExecutionCount")
      .and_then(Value::as_i64)
      .map(|value| value as i32),
    phase: value.get("phase").and_then(Value::as_i64).map(|value| value as i32),
    step_logs,
    log_tail: optional_string(value, "logTail").unwrap_or_default(),
    reports,
    instructions,
    last_log_at,
  })
}

/// 応答の stepLogs 配列から、本文を含むステップの記録を出現順に取り出す。
fn step_log_entries(value: &Value) -> Vec<StepLogEntry> {
  value
    .get("stepLogs")
    .and_then(Value::as_array)
    .map(|logs| {
      logs
        .iter()
        .map(|log| StepLogEntry {
          step_name: optional_string(log, "step").unwrap_or_default(),
          persona:   optional_string(log, "persona").unwrap_or_default(),
          status:    optional_string(log, "status").unwrap_or_default(),
          content:   optional_string(log, "content").unwrap_or_default(),
          timestamp: parse_time(log.get("timestamp")).ok().flatten().unwrap_or(DateTime::<Utc>::UNIX_EPOCH),
          iteration: log.get("iteration").and_then(Value::as_i64).unwrap_or_default() as i32,
        })
        .collect()
    })
    .unwrap_or_default()
}

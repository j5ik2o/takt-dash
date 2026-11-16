//! takt-mcp の読み取り道具を型付きで呼び出し、応答テキストを model 型へ解析する。
//!
//! 応答は `{<key>:[...], nextOffset, ...}` のページ形式です。nextOffset が null になるまで
//! 追従して全件を集めます。道具が isError を返した場合は [`ToolError::Tool`] として扱い、
//! 輸送エラーとは区別します。

use std::{
  collections::{BTreeMap, HashMap},
  path::PathBuf,
  time::Duration,
};

use chrono::{DateTime, Utc};
use serde_json::{json, Value};

use super::client::{CallError, McpClient, McpClientConfig};
use crate::{
  config::McpServer,
  domain::{GoalId, InquiryId, MergeUntil, PullRequestNumber, RegistrationRevision, RunSlug, TaskName},
  model::{
    GoalDetail, GoalEntry, GoalQuestionEntry, GoalTaskEntry, GoalWorkLogEntry, InquiryEntry, NotificationEntry,
    ProjectSnapshot, RegistrationProposalEntry, RunDetail, RunEntry, RunInstructionEntry, RunReportEntry, TaskDetail,
    TaskEntry, TaskStatus,
  },
};

mod goal;
mod parse;
mod run;

use self::{
  goal::{
    is_missing_goal_field, parse_goal, parse_goal_execution_status, parse_goal_question, parse_goal_status,
    parse_goal_work_log, parse_inquiry, parse_notification, parse_task, parse_task_detail, prev_inquiries,
    registration_summary_text, resolve_awaiting_merge_at, resolve_inquiry, ParsedInquiry, TaskLinkFields,
  },
  parse::{checked_i32, domain_value, optional_string, parse_time, required_i32, required_string, string_field, Page},
  run::{parse_run, parse_run_detail, report_summaries, step_entries, step_log_entries, step_log_next_offset},
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
  client:    &'a mut McpClient,
  cwd:       PathBuf,
  workflows: HashMap<String, Option<Vec<String>>>,
}

/// ページ形式の一覧を全件集めるための上限。無限追従を避けます。
const MAX_PAGES: usize = 100;
/// takt のゴール仕様の記法。takt_get_goal は省略した大きい値だけを omissions に載せるため、
/// 仕様の有無を omissions で判定せず、既知の記法を順に読みます。
const SPECIFICATION_NOTATIONS: [&str; 3] = ["gherkin", "quint", "alloy"];

impl<'a> ToolSet<'a> {
  pub fn new(client: &'a mut McpClient, cwd: PathBuf) -> Self {
    ToolSet { client, cwd, workflows: HashMap::new() }
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
  /// 保持し、プロジェクト全体を失敗にしません。ゴール登録提案も道具エラーと解析エラーの
  /// ときは前回の提案（無ければ空）へ縮退し、周期を失敗させません。コア読み取り
  /// （ゴール・タスク）の失敗と、補助読み取りの輸送エラーは全体の失敗として伝播します。
  ///
  /// ゴールのマージ待ちの起点時刻は takt_get_goal の通知から得るため、通知を読んだ後に
  /// ゴールへ起点時刻を反映します。通知が無いゴールは前回 snapshot の同じゴールの起点を
  /// 引き継ぎ、前回が無い初回観測でもその読み取りの観測時刻を起点として確定します。
  pub async fn read_snapshot(&mut self, prev: Option<&ProjectSnapshot>) -> Result<ProjectSnapshot, ToolError> {
    let observed_at = Utc::now();
    let mut goals = self.list_goals().await?;
    let tasks = self.list_tasks().await?;
    let inquiries = self.list_all_inquiries(&goals, prev, observed_at).await?;
    let (notifications, awaiting_merge_at) = self.list_notifications(&goals, prev).await?;
    for goal in &mut goals {
      goal.awaiting_merge_at =
        resolve_awaiting_merge_at(goal, awaiting_merge_at.get(&goal.goal_id).copied(), prev, observed_at);
    }
    let runs = self.get_running_runs(&tasks, prev).await?;
    let proposals = match self.list_goal_registration_proposals().await {
      | Ok(entries) => entries,
      | Err(ToolError::Tool(_)) | Err(ToolError::Parse(_)) => {
        prev.map(|snapshot| snapshot.proposals.clone()).unwrap_or_default()
      },
      | Err(error) => return Err(error),
    };
    Ok(ProjectSnapshot { goals, tasks, inquiries, runs, notifications, proposals })
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
        | Err(ToolError::Tool(_)) => inquiries.extend(prev_inquiries(prev, |goal_id| goal_id == Some(&goal.goal_id))),
        | Err(error) => return Err(error),
      }
    }
    Ok(inquiries.into_iter().map(|parsed| resolve_inquiry(parsed, prev, observed_at)).collect())
  }

  async fn list_goal_questions(&mut self, goal_id: &GoalId) -> Result<Vec<ParsedInquiry>, ToolError> {
    let mut entries = Vec::new();
    let mut offset = 0;
    for _ in 0..MAX_PAGES {
      let value =
        self.call_json("takt_list_goal_questions", json!({ "goalId": goal_id.as_str(), "offset": offset })).await?;
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
            prev.and_then(|snapshot| snapshot.runs.iter().find(|entry| entry.run_slug == run_slug.as_str()))
          {
            runs.push(previous.clone());
          }
        },
        | Err(error) => return Err(error),
      }
    }
    Ok(runs)
  }

  async fn read_run(&mut self, run_slug: &RunSlug, task: &TaskEntry) -> Result<RunEntry, ToolError> {
    let value = self.call_json("takt_get_run", json!({ "runSlug": run_slug.as_str() })).await?;
    self.parse_run_with_pages(&value, task).await
  }

  /// takt_get_run の応答を解析する。stepLogs は先頭 50 件のウィンドウで返るため、
  /// stepLogPage.nextOffset が残っていれば追加ページを取得して全件を集める。
  async fn parse_run_with_pages(&mut self, value: &Value, task: &TaskEntry) -> Result<RunEntry, ToolError> {
    let mut step_logs = step_log_entries(value);
    let mut steps = step_entries(value);
    let mut next_offset = step_log_next_offset(value);
    let run_slug = domain_value("run slug", RunSlug::try_from(string_field(value, "runSlug")?.as_str()))?;
    let mut pages_taken = 0;
    while let Some(offset) = next_offset {
      if pages_taken >= MAX_PAGES {
        return Err(ToolError::Parse("takt_get_run の stepLogs 追従が上限に達しました".to_owned()));
      }
      pages_taken += 1;
      let page_value =
        self.call_json("takt_get_run", json!({ "runSlug": run_slug.as_str(), "offset": offset })).await?;
      steps.extend(step_entries(&page_value));
      step_logs.extend(step_log_entries(&page_value));
      next_offset = step_log_next_offset(&page_value);
    }
    let workflow = optional_string(value, "workflow").unwrap_or_default();
    let mut run = parse_run(value, task, step_logs, steps)?;
    if !workflow.is_empty() {
      self.enrich_step_progress(&mut run, &workflow).await?;
    }
    Ok(run)
  }

  /// workflow の step 名一覧から、run の現在ステップの位置と全ステップ数を補う。
  ///
  /// workflow を読めないときや現在ステップが一覧に無いときは、位置と総数を設定しません。
  /// 道具エラー・解析不能な応答は位置不明として扱い、run 一覧全体を失敗させません。
  async fn enrich_step_progress(&mut self, run: &mut RunEntry, workflow: &str) -> Result<(), ToolError> {
    let Some(steps) = self.workflow_steps(workflow).await? else {
      return Ok(());
    };
    // takt_get_run の currentStep は takt_get_workflow が返す workflow の step 名に対応する。
    // stepLogs の末尾は入れ子 workflow の内側の step になり得るため、位置の算出には使わない。
    // currentStep が無い応答だけ、従来どおり stepLogs の末尾で補う。
    let Some(current) = run
      .current_step
      .as_deref()
      .filter(|name| !name.is_empty())
      .or_else(|| run.steps.iter().rev().find(|step| !step.step_name.is_empty()).map(|step| step.step_name.as_str()))
    else {
      return Ok(());
    };
    let Some(index) = steps.iter().position(|name| name == current) else {
      return Ok(());
    };
    let Some(step_number) = i32::try_from(index + 1).ok() else {
      return Ok(());
    };
    let Some(total_steps) = i32::try_from(steps.len()).ok() else {
      return Ok(());
    };
    run.current_step_number = Some(step_number);
    run.total_steps = Some(total_steps);
    Ok(())
  }

  /// workflow の step 名一覧を取得する。同じ workflow の結果は道具のまとまりの中で再利用します。
  async fn workflow_steps(&mut self, workflow: &str) -> Result<Option<Vec<String>>, ToolError> {
    if let Some(cached) = self.workflows.get(workflow) {
      return Ok(cached.clone());
    }
    let steps = match self.fetch_workflow_steps(workflow).await {
      | Ok(steps) => steps,
      | Err(ToolError::Transport(error)) => return Err(ToolError::Transport(error)),
      | Err(_) => None,
    };
    self.workflows.insert(workflow.to_owned(), steps.clone());
    Ok(steps)
  }

  async fn fetch_workflow_steps(&mut self, workflow: &str) -> Result<Option<Vec<String>>, ToolError> {
    let mut steps = Vec::new();
    let mut offset = 0;
    let mut total_count = None;
    for _ in 0..MAX_PAGES {
      let value = self.call_json("takt_get_workflow", json!({ "workflow": workflow, "offset": offset })).await?;
      total_count = total_count.or_else(|| value.get("totalCount").and_then(Value::as_u64).map(|count| count as usize));
      let page = Page::from_value(&value, "steps")?;
      for item in &page.items {
        let Some(name) = item.get("name").and_then(Value::as_str) else {
          return Ok(None);
        };
        steps.push(name.to_owned());
      }
      match page.next_offset {
        | Some(next) => offset = next,
        | None => {
          if total_count.is_some_and(|total| total != steps.len()) {
            return Ok(None);
          }
          return Ok(if steps.is_empty() { None } else { Some(steps) });
        },
      }
    }
    Err(ToolError::Parse("takt_get_workflow のページ追従が上限に達しました".to_owned()))
  }

  async fn list_notifications(
    &mut self,
    goals: &[GoalEntry],
    prev: Option<&ProjectSnapshot>,
  ) -> Result<(Vec<NotificationEntry>, BTreeMap<GoalId, DateTime<Utc>>), ToolError> {
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
          if let Some(prev) = prev {
            notifications.extend(prev.notifications.iter().filter(|entry| entry.goal_id == goal.goal_id).cloned());
            if let Some(recorded_at) =
              prev.goals.iter().find(|entry| entry.goal_id == goal.goal_id).and_then(|entry| entry.awaiting_merge_at)
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
    goal_id: &GoalId,
  ) -> Result<(Vec<NotificationEntry>, Option<DateTime<Utc>>), ToolError> {
    let mut notifications = Vec::new();
    let mut awaiting_merge_at: Option<DateTime<Utc>> = None;
    let mut offset = 0;
    for _ in 0..MAX_PAGES {
      let value = self
        .call_json("takt_get_goal", json!({ "goalId": goal_id.as_str(), "path": "notifications", "offset": offset }))
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
    for notation in SPECIFICATION_NOTATIONS {
      let content = self.read_goal_text(goal_id, &format!("specifications.{notation}")).await?;
      if !content.is_empty() {
        specifications.insert(notation.to_owned(), content);
      }
    }
    let work_log = self.read_goal_work_log(goal_id).await?;
    let questions = self.read_goal_questions(goal_id).await?;
    let tasks = self
      .list_tasks()
      .await?
      .into_iter()
      .filter(|task| task.goal_id.as_ref().map(|goal_id| goal_id.as_str()) == Some(goal_id))
      .map(|task| GoalTaskEntry {
        task_name: task.name.as_str().to_owned(),
        status:    task.status,
        run_slug:  task.run_slug.as_ref().map(|run_slug| run_slug.as_str().to_owned()),
      })
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
  ///
  /// 空または省略された任意項目は空文字として扱います。`content` は必須項目ではないため、
  /// 応答に無ければ空文字です（実 takt-mcp は項目そのものが無い場合に
  /// `Selected field does not exist` の道具エラーを返します）。`content` が存在して型が
  /// 文字列でない場合は解析エラーにします。
  async fn read_goal_text(&mut self, goal_id: &str, path: &str) -> Result<String, ToolError> {
    let mut content = String::new();
    let mut offset = 0;
    for _ in 0..MAX_PAGES {
      let value =
        match self.call_json("takt_get_goal", json!({ "goalId": goal_id, "path": path, "textOffset": offset })).await {
          | Ok(value) => value,
          | Err(ToolError::Tool(error)) if is_missing_goal_field(&error) => return Ok(content),
          | Err(error) => return Err(error),
        };
      if let Some(chunk) = value.get("content") {
        let chunk =
          chunk.as_str().ok_or_else(|| ToolError::Parse(format!("{path} の content が文字列ではありません")))?;
        content.push_str(chunk);
      }
      match value.get("nextOffset").and_then(Value::as_u64) {
        | Some(next) => offset = next,
        | None => return Ok(content),
      }
    }
    Err(ToolError::Parse(format!("takt_get_goal の {path} の追従が上限に達しました")))
  }

  /// takt_get_goal の配列フィールドを offset で全ページ集める。
  ///
  /// 空または省略された任意項目は空配列として扱います。`items` は必須項目ではないため、
  /// 応答に無ければ空配列です（実 takt-mcp は項目そのものが無い場合に
  /// `Selected field does not exist` の道具エラーを返します）。`items` が存在して配列でない
  /// 場合は解析エラーにします。
  async fn read_goal_array(&mut self, goal_id: &str, path: &str) -> Result<Vec<String>, ToolError> {
    let mut entries = Vec::new();
    let mut offset = 0;
    for _ in 0..MAX_PAGES {
      let value =
        match self.call_json("takt_get_goal", json!({ "goalId": goal_id, "path": path, "offset": offset })).await {
          | Ok(value) => value,
          | Err(ToolError::Tool(error)) if is_missing_goal_field(&error) => return Ok(entries),
          | Err(error) => return Err(error),
        };
      let Some(items) = value.get("items") else {
        return Ok(entries);
      };
      let items = items.as_array().ok_or_else(|| ToolError::Parse(format!("{path} の items が配列ではありません")))?;
      for item in items {
        let text = item.as_str().ok_or_else(|| ToolError::Parse(format!("{path} の要素が文字列ではありません")))?;
        entries.push(text.to_owned());
      }
      match value.get("nextOffset").and_then(Value::as_u64) {
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

  /// takt_get_goal（path なし）の goal summary から、タスク詳細に載せる
  /// ブランチとプルリクエスト URL を読む。
  async fn read_task_link_fields(&mut self, goal_id: &str) -> Result<TaskLinkFields, ToolError> {
    let value = self.call_json("takt_get_goal", json!({ "goalId": goal_id })).await?;
    let goal =
      value.get("goal").ok_or_else(|| ToolError::Parse("takt_get_goal の応答に goal がありません".to_owned()))?;
    Ok(TaskLinkFields {
      branch:           optional_string(goal, "branch"),
      pull_request_url: goal
        .get("pullRequest")
        .and_then(|pull_request| pull_request.get("url"))
        .and_then(Value::as_str)
        .map(str::to_owned),
    })
  }

  /// takt_list_tasks を全ページ取得し、タスク名（と任意の
  /// goalId）が一致する最初のタスク詳細を返す。
  ///
  /// 一致するタスクが goalId を持つ場合は、そのゴールの summary から
  /// ブランチとプルリクエスト URL を補います。
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
        let links = match optional_string(item, "goalId") {
          | Some(task_goal_id) => self.read_task_link_fields(&task_goal_id).await?,
          | None => TaskLinkFields::default(),
        };
        return Ok(Some(parse_task_detail(project, item, links)?));
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
  pub async fn read_run_detail(&mut self, run_slug: &RunSlug) -> Result<RunDetail, ToolError> {
    let value = self.call_json("takt_get_run", json!({ "runSlug": run_slug.as_str() })).await?;
    let task = self.list_tasks().await?.into_iter().find(|task| task.run_slug.as_ref() == Some(run_slug));
    let mut step_logs = step_log_entries(&value);
    let mut next_offset = step_log_next_offset(&value);
    let mut pages_taken = 0;
    while let Some(offset) = next_offset {
      if pages_taken >= MAX_PAGES {
        return Err(ToolError::Parse("takt_get_run の stepLogs 追従が上限に達しました".to_owned()));
      }
      pages_taken += 1;
      let page_value =
        self.call_json("takt_get_run", json!({ "runSlug": run_slug.as_str(), "offset": offset })).await?;
      step_logs.extend(step_log_entries(&page_value));
      next_offset = step_log_next_offset(&page_value);
    }
    let mut reports = Vec::new();
    for summary in report_summaries(&value) {
      let report_value =
        self.call_json("takt_get_run", json!({ "runSlug": run_slug.as_str(), "report": summary.filename })).await?;
      reports.push(RunReportEntry {
        filename:   summary.filename.clone(),
        content:    optional_string(&report_value, "content").unwrap_or_default(),
        size_bytes: report_value.get("sizeBytes").and_then(Value::as_i64).unwrap_or(summary.size_bytes),
      });
    }
    let mut instructions = Vec::new();
    for instruction in value.get("instructions").and_then(Value::as_array).cloned().unwrap_or_default() {
      let instruction_id = match instruction.get("instructionId") {
        | None => 0,
        | Some(field) => checked_i32(Some(field), "instructionId")?,
      };
      let instruction_value = self
        .call_json("takt_get_run", json!({ "runSlug": run_slug.as_str(), "instructionId": instruction_id }))
        .await?;
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

  /// 問い合わせへ回答する。`answer` は takt の `answer` オブジェクト（例: `{"text": "..."}`）。
  /// 応答の `questionId` を返します。takt の入力契約では `goalId` は必須（UUID）なので、
  /// `goal_id` は必須の引数として受け取ります。
  pub async fn answer_goal_question(
    &mut self,
    goal_id: &GoalId,
    question_id: &InquiryId,
    answer: Value,
  ) -> Result<InquiryId, ToolError> {
    let value = self
      .call_json(
        "takt_answer_goal_question",
        json!({ "goalId": goal_id.as_str(), "questionId": question_id.as_str(), "answer": answer }),
      )
      .await?;
    let question_id = optional_string(&value, "questionId")
      .ok_or_else(|| ToolError::Parse("takt_answer_goal_question の応答に questionId がありません".to_owned()))?;
    domain_value("inquiry ID", InquiryId::try_from(question_id.as_str()))
  }

  /// ゴール登録の承認待ちを取得する。提案が無ければ空の並びを返します。
  pub async fn list_goal_registration_proposals(&mut self) -> Result<Vec<RegistrationProposalEntry>, ToolError> {
    let value = self.call_json("takt_list_goal_registration_proposals", json!({})).await?;
    let proposals = value.get("proposals").and_then(Value::as_array).ok_or_else(|| {
      ToolError::Parse("takt_list_goal_registration_proposals の応答に proposals の配列がありません".to_owned())
    })?;
    proposals
      .iter()
      .map(|proposal| {
        Ok(RegistrationProposalEntry {
          revision:             domain_value(
            "revision",
            RegistrationRevision::try_from(required_i32(proposal, "revision")?),
          )?,
          repository_directory: required_string(proposal, "repositoryDirectory")?,
          summary:              registration_summary_text(proposal.get("summary"))?,
        })
      })
      .collect()
  }

  /// ゴール登録の承認待ちを承認する。承認した revision と、登録されたゴールの ID を返します。
  pub async fn approve_goal_registration(
    &mut self,
    revision: RegistrationRevision,
  ) -> Result<ApproveGoalRegistrationResult, ToolError> {
    let value = self.call_json("takt_approve_goal_registration", json!({ "revision": revision.get() })).await?;
    let goal = value.get("goal").ok_or_else(|| ToolError::Parse("応答に goal がありません".to_owned()))?;
    let goal_id = domain_value("ゴール ID", GoalId::try_from(required_string(goal, "id")?.as_str()))?;
    let revision = domain_value("revision", RegistrationRevision::try_from(required_i32(&value, "revision")?))?;
    Ok(ApproveGoalRegistrationResult { revision, goal_id })
  }

  /// ゴール登録の承認待ちを却下する。却下した revision を返します。
  pub async fn reject_goal_registration(
    &mut self,
    revision: RegistrationRevision,
  ) -> Result<RegistrationRevision, ToolError> {
    let value = self.call_json("takt_reject_goal_registration", json!({ "revision": revision.get() })).await?;
    domain_value("revision", RegistrationRevision::try_from(required_i32(&value, "revision")?))
  }

  /// プルリクエストのマージを承認する。依頼したタスク名とワークフローを返します。
  pub async fn merge_goal_pr(
    &mut self,
    goal_id: &GoalId,
    pr_number: PullRequestNumber,
    until: MergeUntil,
  ) -> Result<MergeGoalPrResult, ToolError> {
    let value = self
      .call_json(
        "takt_merge_goal_pr",
        json!({ "goalId": goal_id.as_str(), "prNumber": pr_number.get(), "until": until.as_str() }),
      )
      .await?;
    Ok(MergeGoalPrResult {
      task_name: domain_value("タスク名", TaskName::try_from(required_string(&value, "taskName")?.as_str()))?,
      workflow:  required_string(&value, "workflow")?,
    })
  }
}

/// takt_approve_goal_registration の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApproveGoalRegistrationResult {
  pub revision: RegistrationRevision,
  pub goal_id:  GoalId,
}

/// takt_merge_goal_pr の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeGoalPrResult {
  pub task_name: TaskName,
  pub workflow:  String,
}

//! takt-mcp の読み取り道具を型付きで呼び出し、応答テキストを model 型へ解析する。
//!
//! 応答は `{<key>:[...], nextOffset, ...}` のページ形式です。nextOffset が null になるまで
//! 追従して全件を集めます。道具が isError を返した場合は [`ToolError::Tool`] として扱い、
//! 輸送エラーとは区別します。

use std::{
  collections::{BTreeMap, BTreeSet, HashMap},
  path::PathBuf,
  time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use serde_json::{json, Value};

use super::{
  client::{CallError, McpClient, McpClientConfig, TransportError},
  metrics::{ToolCallMetrics, ToolCallOutcome},
};
use crate::{
  config::McpServer,
  domain::{GoalId, InquiryId, MergeUntil, ProjectName, PullRequestNumber, RegistrationRevision, RunSlug, TaskName},
  model::{
    ExecutionStatus, GoalDetail, GoalEntry, GoalQuestionEntry, GoalStatus, GoalTaskEntry, GoalWorkLogEntry,
    InquiryEntry, NotificationEntry, ProjectSnapshot, RegistrationProposalEntry, RunEntry, TaskDetail, TaskEntry,
    TaskStatus, TerminalReads,
  },
  run_progress::{read_run, retain_runs, RunLogHandle, RunReadFailure},
};

mod goal;
mod parse;
mod run;

pub use self::run::RemoteRun;
use self::{
  goal::{
    is_missing_goal_field, parse_goal, parse_goal_execution_status, parse_goal_question, parse_goal_status,
    parse_goal_work_log, parse_inquiry, parse_notification, parse_task, parse_task_detail, prev_inquiries,
    registration_summary_text, resolve_awaiting_merge_at, resolve_inquiry, ParsedInquiry, TaskLinkFields,
  },
  parse::{domain_value, optional_string, required_i32, required_string, string_field, Page},
  run::{parse_remote_run, step_entries, step_log_entries, step_log_next_offset},
};

/// 一覧読み取り（ゴール・タスク）の期限。応答が大きく、2026-11-17 の本番実測では
/// 高負荷のときに takt_list_goals の応答が最大で約15秒、takt_list_tasks が15秒の
/// 期限切れになり、読み取りサイクル全体が失敗しました。初期化の期限（60秒）と
/// 同じ予算まで広げ、遅れてもサイクルを失敗させないようにします。
const LIST_READ_TIMEOUT: Duration = Duration::from_secs(60);

/// takt-mcp 子プロセスの起動設定を作る。
///
/// poller の定期読み取りと、書き込み・詳細の RPC が要求時に使う子プロセスが、同じ起動条件
/// （タイムアウトを含む）を共有するために公開します。
pub fn client_config(server: &McpServer, cwd: &std::path::Path) -> McpClientConfig {
  McpClientConfig {
    command:         server.command.clone(),
    args:            server.args.clone(),
    env:             server.env.clone(),
    cwd:             cwd.to_path_buf(),
    request_timeout: Duration::from_secs(15),
    tool_timeouts:   BTreeMap::from([
      ("takt_list_goals".to_owned(), LIST_READ_TIMEOUT),
      ("takt_list_tasks".to_owned(), LIST_READ_TIMEOUT),
    ]),
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

/// 輸送エラーのうち期限切れかどうか。
fn is_timeout(error: &ToolError) -> bool {
  matches!(error, ToolError::Transport(TransportError::Timeout(_)))
}

/// 補助読み取りの失敗のうち、前回値へ縮退してよい失敗かどうか。
///
/// 道具エラーは従来どおり縮退します。輸送の期限切れも、高負荷で1件の補助読み取りが
/// 遅れただけでプロジェクト全体を失敗させないため、縮退の対象に加えます。それ以外の
/// 輸送エラー（プロセス終了・起動失敗など）は接続自体が壊れている兆候として、
/// 従来どおり全体の失敗にします。
fn degrades_to_previous(error: &ToolError) -> bool {
  matches!(error, ToolError::Tool(_)) || is_timeout(error)
}

/// 1プロジェクト分の状態を読み取るための道具のまとまり。
///
/// takt-mcp の読み取り道具は入力引数に絶対パスの `cwd` を必須とするため、この道具のまとまりは
/// 対象プロジェクトの `cwd` を保持し、すべての tools/call の引数へ渡します。
pub struct ToolSet<'a> {
  client:    &'a mut McpClient,
  cwd:       PathBuf,
  metrics:   ToolCallMetrics,
  workflows: HashMap<String, Option<Vec<String>>>,
}

/// ページ形式の一覧を全件集めるための上限。無限追従を避けます。
const MAX_PAGES: usize = 100;
/// takt のゴール仕様の記法。takt_get_goal は省略した大きい値だけを omissions に載せるため、
/// 仕様の有無を omissions で判定せず、既知の記法を順に読みます。
const SPECIFICATION_NOTATIONS: [&str; 3] = ["gherkin", "quint", "alloy"];

impl<'a> ToolSet<'a> {
  pub fn new(client: &'a mut McpClient, cwd: PathBuf) -> Self {
    ToolSet { client, cwd, metrics: ToolCallMetrics::default(), workflows: HashMap::new() }
  }

  /// この道具のまとまりが記録した呼び出しの集計を取り出す。
  pub(crate) fn take_metrics(&mut self) -> ToolCallMetrics {
    std::mem::take(&mut self.metrics)
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
  /// `prev` には前回の snapshot を渡します。`runs` はプロジェクトの run の集計で、同じ取っ手を
  /// 周期をまたいで渡すと、run
  /// の本体ログは前回の続きだけを読みます。補助読み取り（run・ゴール質問・通知・ゴールに
  /// 属さない問い合わせ）が道具エラーまたはタイムアウトで失敗したときは、その項目だけ前回値
  /// （無ければ空）を保持し、プロジェクト全体を失敗にしません。ゴール登録提案も道具エラーと
  /// 解析エラーと期限切れのときは前回の提案（無ければ空）へ縮退し、周期を失敗させません。
  /// コア読み取り（ゴール・タスク）の失敗と、補助読み取りのその他の輸送エラー・解析エラーは
  /// 全体の失敗として伝播します。
  ///
  /// ゴールのマージ待ちの起点時刻は takt_get_goal の通知から得るため、通知を読んだ後に
  /// ゴールへ起点時刻を反映します。通知が無いゴールは前回 snapshot の同じゴールの起点を
  /// 引き継ぎ、前回が無い初回観測でもその読み取りの観測時刻を起点として確定します。
  pub async fn read_snapshot(
    &mut self,
    prev: Option<&ProjectSnapshot>,
    runs: &RunLogHandle,
  ) -> Result<ProjectSnapshot, ToolError> {
    let observed_at = Utc::now();
    let mut goals = self.list_goals().await?;
    let tasks = self.list_tasks().await?;
    let (inquiries, questions_read) = self.list_all_inquiries(&goals, prev, observed_at).await?;
    let (notifications, awaiting_merge_at, notifications_read) = self.list_notifications(&goals, prev).await?;
    for goal in &mut goals {
      goal.awaiting_merge_at =
        resolve_awaiting_merge_at(goal, awaiting_merge_at.get(&goal.goal_id).copied(), prev, observed_at);
    }
    let runs = self.get_running_runs(&tasks, prev, runs).await?;
    let proposals = match self.list_goal_registration_proposals().await {
      | Ok(entries) => entries,
      | Err(error) if matches!(error, ToolError::Parse(_)) || degrades_to_previous(&error) => {
        prev.map(|snapshot| snapshot.proposals.clone()).unwrap_or_default()
      },
      | Err(error) => return Err(error),
    };
    let terminal_reads = TerminalReads { questions: questions_read, notifications: notifications_read };
    Ok(ProjectSnapshot { goals, tasks, inquiries, runs, notifications, proposals, terminal_reads })
  }

  async fn call_json(&mut self, name: &str, extra: Value) -> Result<Value, ToolError> {
    let arguments = self.arguments(extra);
    // 子プロセスの起動と initialize は道具呼び出しではないため、計時を始める前に済ませます。
    self.client.ensure_started().await?;
    let started = Instant::now();
    let result = self.client.call_tool(name, arguments).await;
    self.metrics.record(name, started.elapsed(), call_outcome(&result));
    let text = result?;
    serde_json::from_str(&text).map_err(|error| ToolError::Parse(format!("{name}: {error}")))
  }

  /// ゴール一覧を全ページ集める。輸送の期限切れのときだけ、もう1回だけ読み直します。
  /// 期限切れで子プロセスは破棄されるため、再試行は新しい子プロセスへの読み直しに
  /// なります。読み取り専用の道具なので、再試行に副作用はありません。
  async fn list_goals(&mut self) -> Result<Vec<GoalEntry>, ToolError> {
    match self.fetch_goals().await {
      | Err(error) if is_timeout(&error) => self.fetch_goals().await,
      | result => result,
    }
  }

  async fn fetch_goals(&mut self) -> Result<Vec<GoalEntry>, ToolError> {
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

  /// タスク一覧を全ページ集める。ゴール一覧と同じく、輸送の期限切れのときだけ
  /// もう1回だけ読み直します。
  async fn list_tasks(&mut self) -> Result<Vec<TaskEntry>, ToolError> {
    match self.fetch_tasks().await {
      | Err(error) if is_timeout(&error) => self.fetch_tasks().await,
      | result => result,
    }
  }

  async fn fetch_tasks(&mut self) -> Result<Vec<TaskEntry>, ToolError> {
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
  /// 道具エラーまたはタイムアウト（そのゴールの質問だけ取得できない、またはゴールに属さない
  /// 問い合わせだけ取得できない）は、該当項目だけ前回 snapshot の同じゴールの問い合わせを
  /// 引き継ぎます（初回観測時は空）。その他の輸送エラーと解析エラーは全体の失敗として伝播します。
  ///
  /// あわせて、終端状態で質問を読めたゴール（読み取りを省いたゴールを含む）を返します。
  async fn list_all_inquiries(
    &mut self,
    goals: &[GoalEntry],
    prev: Option<&ProjectSnapshot>,
    observed_at: DateTime<Utc>,
  ) -> Result<(Vec<InquiryEntry>, BTreeSet<GoalId>), ToolError> {
    let mut inquiries = match self.list_standalone_inquiries().await {
      | Ok(entries) => entries,
      | Err(error) if degrades_to_previous(&error) => prev_inquiries(prev, |goal_id| goal_id.is_none()),
      | Err(error) => return Err(error),
    };
    let mut read_in_terminal = BTreeSet::new();
    for goal in goals {
      if is_unchanged_terminal_goal(goal, prev, |reads| &reads.questions) {
        inquiries.extend(prev_inquiries(prev, |goal_id| goal_id == Some(&goal.goal_id)));
        read_in_terminal.insert(goal.goal_id.clone());
        continue;
      }
      match self.list_goal_questions(&goal.goal_id).await {
        | Ok(mut entries) => {
          inquiries.append(&mut entries);
          if is_terminal(goal) {
            read_in_terminal.insert(goal.goal_id.clone());
          }
        },
        | Err(error) if degrades_to_previous(&error) => {
          inquiries.extend(prev_inquiries(prev, |goal_id| goal_id == Some(&goal.goal_id)))
        },
        | Err(error) => return Err(error),
      }
    }
    let inquiries = inquiries.into_iter().map(|parsed| resolve_inquiry(parsed, prev, observed_at)).collect();
    Ok((inquiries, read_in_terminal))
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

  /// running なタスクの run を、run の保存場所（`meta.json` と本体ログ）から集める。
  ///
  /// 保存場所の対応表に無い run（[`RunReadFailure::NotListed`]）と、対応表を読めない run
  /// （[`RunReadFailure::Unreadable`]。SSH 越しに扱うリモートのサーバーのプロジェクトなど）は、
  /// takt-mcp の takt_get_run から作ります。保存場所で読めた run には takt_get_run を呼びません。
  ///
  /// どちらからも読めなかった run
  /// と、ステップ位置の補完が縮退できる失敗（道具エラー・タイムアウト） になった run だけ、前回
  /// snapshot の同じ runSlug を引き継ぎ（無ければ欠落）、健常な run は
  /// 今回の値を採ります。これらの失敗はプロジェクトの失敗にしません。takt_get_run と補完の
  /// その他の輸送エラーは全体の失敗として伝播します。running でなくなった run の集計は、次の周期に
  /// 持ち越さないよう捨てます。
  async fn get_running_runs(
    &mut self,
    tasks: &[TaskEntry],
    prev: Option<&ProjectSnapshot>,
    handle: &RunLogHandle,
  ) -> Result<Vec<RunEntry>, ToolError> {
    let mut runs = Vec::new();
    let mut running = Vec::new();
    for task in tasks.iter().filter(|task| task.status == TaskStatus::Running) {
      let Some(run_slug) = task.run_slug.clone() else {
        continue;
      };
      running.push(run_slug.clone());
      let (mut run, workflow) = match read_run(handle, run_slug.clone()).await {
        | Ok(view) => (view.run_entry(&run_slug, task), view.workflow),
        | Err(failure @ (RunReadFailure::NotListed | RunReadFailure::Unreadable(_))) => {
          match self.read_remote_run(&run_slug).await {
            | Ok(remote) => (remote.run_entry(task), remote.workflow),
            | Err(error) if matches!(error, ToolError::Parse(_)) || degrades_to_previous(&error) => {
              tracing::warn!(run_slug = %run_slug.as_str(), local = %failure, error = %error, "run を保存場所からも takt_get_run からも読めませんでした。前回の値を使います。");
              runs.extend(previous_run(prev, &run_slug));
              continue;
            },
            | Err(error) => return Err(error),
          }
        },
        | Err(failure) => {
          tracing::warn!(run_slug = %run_slug.as_str(), error = %failure, "run を保存場所から読めませんでした。前回の値を使います。");
          runs.extend(previous_run(prev, &run_slug));
          continue;
        },
      };
      let enriched = match workflow.as_deref().filter(|workflow| !workflow.is_empty()) {
        | Some(workflow) => self.enrich_step_progress(&mut run, workflow).await,
        | None => Ok(()),
      };
      match enriched {
        | Ok(()) => runs.push(run),
        | Err(error) if degrades_to_previous(&error) => {
          tracing::warn!(run_slug = %run_slug.as_str(), error = %error, "run のステップ位置を補えませんでした。前回の値を使います。");
          runs.extend(previous_run(prev, &run_slug));
        },
        | Err(error) => return Err(error),
      }
    }
    retain_runs(handle, running).await;
    Ok(runs)
  }

  /// takt_get_run から run を読む。stepLogs は先頭のウィンドウだけが返るため、
  /// stepLogPage.nextOffset が残っていれば追加のページを取得して全件を集めます。
  pub async fn read_remote_run(&mut self, run_slug: &RunSlug) -> Result<RemoteRun, ToolError> {
    let value = self.call_json("takt_get_run", json!({ "runSlug": run_slug.as_str() })).await?;
    let mut steps = step_entries(&value);
    let mut step_logs = step_log_entries(&value);
    let mut next_offset = step_log_next_offset(&value);
    let mut pages_taken = 0;
    while let Some(offset) = next_offset {
      if pages_taken >= MAX_PAGES {
        return Err(ToolError::Parse("takt_get_run の stepLogs 追従が上限に達しました".to_owned()));
      }
      pages_taken += 1;
      let page = self.call_json("takt_get_run", json!({ "runSlug": run_slug.as_str(), "offset": offset })).await?;
      steps.extend(step_entries(&page));
      step_logs.extend(step_log_entries(&page));
      next_offset = step_log_next_offset(&page);
    }
    parse_remote_run(&value, steps, step_logs)
  }

  /// workflow の step 名一覧から、run の現在ステップの位置と全ステップ数を補う。
  ///
  /// workflow を読めないときや現在ステップが一覧に無いときは、位置と総数を設定しません。
  /// 道具エラー・解析不能な応答は位置不明として扱い、run 一覧全体を失敗させません。
  async fn enrich_step_progress(&mut self, run: &mut RunEntry, workflow: &str) -> Result<(), ToolError> {
    let Some(steps) = self.workflow_steps(workflow).await? else {
      return Ok(());
    };
    // meta.json の currentStep は takt_get_workflow が返す workflow の step 名に対応する。
    // ステップの記録の末尾は入れ子 workflow の内側の step になり得るため、位置の算出には使わない。
    // currentStep が無い run だけ、従来どおりステップの記録の末尾で補う。
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

  /// ゴールごとの通知と、マージ待ちの起点時刻を集める。あわせて、終端状態で通知を読めたゴール
  /// （読み取りを省いたゴールを含む）を返します。
  async fn list_notifications(
    &mut self,
    goals: &[GoalEntry],
    prev: Option<&ProjectSnapshot>,
  ) -> Result<(Vec<NotificationEntry>, BTreeMap<GoalId, DateTime<Utc>>, BTreeSet<GoalId>), ToolError> {
    let mut notifications = Vec::new();
    let mut awaiting_merge_at = BTreeMap::new();
    let mut read_in_terminal = BTreeSet::new();
    for goal in goals {
      if is_unchanged_terminal_goal(goal, prev, |reads| &reads.notifications) {
        inherit_previous_notifications(&mut notifications, &mut awaiting_merge_at, goal, prev);
        read_in_terminal.insert(goal.goal_id.clone());
        continue;
      }
      match self.list_goal_notifications(&goal.goal_id).await {
        | Ok((goal_notifications, goal_awaiting_merge_at)) => {
          if let Some(recorded_at) = goal_awaiting_merge_at {
            awaiting_merge_at.insert(goal.goal_id.clone(), recorded_at);
          }
          notifications.extend(goal_notifications);
          if is_terminal(goal) {
            read_in_terminal.insert(goal.goal_id.clone());
          }
        },
        | Err(error) if degrades_to_previous(&error) => {
          inherit_previous_notifications(&mut notifications, &mut awaiting_merge_at, goal, prev);
        },
        | Err(error) => return Err(error),
      }
    }
    Ok((notifications, awaiting_merge_at, read_in_terminal))
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

/// 道具呼び出しの結果を集計用の結果へ分類する。応答の解析エラーは道具呼び出し自体の結果では
/// ないため、ここでは扱いません。
fn call_outcome(result: &Result<String, CallError>) -> ToolCallOutcome {
  match result {
    | Ok(_) => ToolCallOutcome::Succeeded,
    | Err(CallError::Tool(_)) => ToolCallOutcome::Failed,
    | Err(CallError::Transport(TransportError::Timeout(_))) => ToolCallOutcome::TimedOut,
    | Err(CallError::Transport(_)) => ToolCallOutcome::Failed,
  }
}

/// 終端（`status == Completed` または `execution_status == Aborted`）のゴールか。
fn is_terminal(goal: &GoalEntry) -> bool {
  goal.status == GoalStatus::Completed || goal.execution_status == ExecutionStatus::Aborted
}

/// 補助読み取りの項目 `item` を省ける、前回と同じ状態の終端ゴールかを判定する。
///
/// 前回の同じゴールが同じ終端状態で、かつ前回その状態でこの項目を読めていた場合だけ省けます。
/// 初回観測、状態変化、`AwaitingMerge` などの非終端、前回の終端状態で読み取りに失敗した項目は
/// 読み取り対象のままです。
fn is_unchanged_terminal_goal(
  goal: &GoalEntry,
  prev: Option<&ProjectSnapshot>,
  item: impl Fn(&TerminalReads) -> &BTreeSet<GoalId>,
) -> bool {
  let Some(prev) = prev else {
    return false;
  };
  let Some(previous) = prev.goals.iter().find(|entry| entry.goal_id == goal.goal_id) else {
    return false;
  };
  previous.status == goal.status
    && previous.execution_status == goal.execution_status
    && is_terminal(goal)
    && item(&prev.terminal_reads).contains(&goal.goal_id)
}

/// 前回 snapshot の同じ runSlug の run を返す。前回が無ければ `None` です。
fn previous_run(prev: Option<&ProjectSnapshot>, run_slug: &RunSlug) -> Option<RunEntry> {
  prev.and_then(|snapshot| snapshot.runs.iter().find(|entry| entry.run_slug == run_slug.as_str())).cloned()
}

/// 前回 snapshot の同じゴールの通知と、マージ待ちの起点時刻を引き継ぐ。前回が無ければ何もしません。
fn inherit_previous_notifications(
  notifications: &mut Vec<NotificationEntry>,
  awaiting_merge_at: &mut BTreeMap<GoalId, DateTime<Utc>>,
  goal: &GoalEntry,
  prev: Option<&ProjectSnapshot>,
) {
  let Some(prev) = prev else {
    return;
  };
  notifications.extend(prev.notifications.iter().filter(|entry| entry.goal_id == goal.goal_id).cloned());
  if let Some(recorded_at) =
    prev.goals.iter().find(|entry| entry.goal_id == goal.goal_id).and_then(|entry| entry.awaiting_merge_at)
  {
    awaiting_merge_at.insert(goal.goal_id.clone(), recorded_at);
  }
}

impl<'a> ToolSet<'a> {
  /// 指定した goalId の詳細を takt_get_goal と補助道具から読み取る。
  ///
  /// 目的・対象外・受け入れ条件・制約・仕様は takt_get_goal の path/textOffset を全ページ追従して
  /// 全文を集めます。作業記録と質問は専用の一覧道具を全ページ追従します。紐づくタスクは
  /// takt_list_tasks を全ページ取得して goal_id で絞ります。
  pub async fn read_goal_detail(&mut self, goal_id: &GoalId) -> Result<GoalDetail, ToolError> {
    let value = self.call_json("takt_get_goal", json!({ "goalId": goal_id.as_str() })).await?;
    let goal =
      value.get("goal").ok_or_else(|| ToolError::Parse("takt_get_goal の応答に goal がありません".to_owned()))?;
    let returned_id = string_field(goal, "id")?;
    if returned_id != goal_id.as_str() {
      return Err(ToolError::Parse(format!("要求したゴール {goal_id} と応答のゴール {returned_id} が一致しません")));
    }
    let status = parse_goal_status(goal.get("status"))?;
    let execution_status = parse_goal_execution_status(goal.get("executionStatus"))?;
    let branch = optional_string(goal, "branch").unwrap_or_default();
    let integration_branch = optional_string(goal, "integrationBranch").unwrap_or_default();
    let objective = self.read_goal_text(goal_id.as_str(), "objective").await?;
    let out_of_scope = self.read_goal_array(goal_id.as_str(), "outOfScope").await?;
    let acceptance_criteria = self.read_goal_array(goal_id.as_str(), "acceptanceCriteria").await?;
    let constraints = self.read_goal_array(goal_id.as_str(), "constraints").await?;
    let mut specifications = BTreeMap::new();
    for notation in SPECIFICATION_NOTATIONS {
      let content = self.read_goal_text(goal_id.as_str(), &format!("specifications.{notation}")).await?;
      if !content.is_empty() {
        specifications.insert(notation.to_owned(), content);
      }
    }
    let work_log = self.read_goal_work_log(goal_id.as_str()).await?;
    let questions = self.read_goal_questions(goal_id.as_str()).await?;
    let tasks = self
      .list_tasks()
      .await?
      .into_iter()
      .filter(|task| task.goal_id.as_ref() == Some(goal_id))
      .map(|task| GoalTaskEntry { task_name: task.name, status: task.status, run_slug: task.run_slug })
      .collect();
    Ok(GoalDetail {
      goal_id: goal_id.clone(),
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
    project: &ProjectName,
    task_name: &TaskName,
    goal_id: Option<&GoalId>,
  ) -> Result<Option<TaskDetail>, ToolError> {
    let mut offset = 0;
    for _ in 0..MAX_PAGES {
      let value = self.call_json("takt_list_tasks", json!({ "offset": offset })).await?;
      let page = Page::from_value(&value, "tasks")?;
      for item in &page.items {
        if item.get("name").and_then(Value::as_str) != Some(task_name.as_str()) {
          continue;
        }
        if let Some(goal_id) = goal_id {
          if item.get("goalId").and_then(Value::as_str) != Some(goal_id.as_str()) {
            continue;
          }
        }
        // goalId が空文字のタスクはゴール未所属（parse_task と同じ扱い）とし、
        // takt_get_goal を呼ばずに、ID が欠落する場合と同じ既定値を使う。
        let links = match optional_string(item, "goalId") {
          | Some(task_goal_id) if !task_goal_id.is_empty() => self.read_task_link_fields(&task_goal_id).await?,
          | _ => TaskLinkFields::default(),
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

#[cfg(test)]
mod tests {
  //! 道具呼び出しの計時境界と、結果分類を固定する。
  //!
  //! 計時境界は mock MCP サーバの起動と initialize を遅らせ、読み取り全体の経過時間と
  //! 道具別の所要時間を同時に観測して確かめます。

  use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
  };

  use super::{call_outcome, ToolError, ToolSet};
  use crate::{
    mcp::{
      client::{CallError, McpClient, McpClientConfig, ToolCallError, TransportError},
      metrics::{ToolCallMetrics, ToolCallOutcome},
    },
    run_progress::run_log_handle,
  };

  fn mock_script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("mock_mcp_server.mjs")
  }

  fn write_state(directory: &Path) -> PathBuf {
    let path = directory.join("state.json");
    std::fs::write(&path, "{}").expect("状態ファイルを書けます");
    path
  }

  fn config(directory: &Path, command: &str, args: Vec<String>, timeout_secs: u64) -> McpClientConfig {
    McpClientConfig {
      command: command.to_owned(),
      args,
      env: Default::default(),
      cwd: directory.to_path_buf(),
      request_timeout: Duration::from_secs(timeout_secs),
      tool_timeouts: Default::default(),
    }
  }

  fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
  }

  /// mock サーバの起動と initialize を `delay` 秒遅らせる設定を作る。
  fn delayed_config(directory: &Path, delay: &str) -> McpClientConfig {
    let state = write_state(directory);
    let script = mock_script();
    let command = format!(
      "sleep {delay}; exec node {} --state {}",
      shell_quote(&script.display().to_string()),
      shell_quote(&state.display().to_string())
    );
    config(directory, "sh", vec!["-c".to_owned(), command], 5)
  }

  #[tokio::test]
  async fn initial_call_duration_excludes_process_start_and_initialize() {
    let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
    let mut client = McpClient::new(delayed_config(directory.path(), "1.5"));
    let mut tools = ToolSet::new(&mut client, directory.path().to_path_buf());

    let started = Instant::now();
    tools
      .read_snapshot(None, &run_log_handle(directory.path().to_path_buf()))
      .await
      .expect("起動の遅延後も読み取れます");
    let elapsed = started.elapsed();
    let metrics = tools.take_metrics();
    drop(tools);

    let goals = metrics.stats("takt_list_goals").expect("takt_list_goals の集計があります");
    assert_eq!(goals.calls, 1);
    assert!(elapsed >= Duration::from_millis(1500), "起動と initialize の遅延を含む経過時間です: {elapsed:?}");
    assert!(goals.max < Duration::from_millis(1000), "道具呼び出しの計時に起動と initialize を含めません: {goals:?}");
    client.shutdown().await;
  }

  #[tokio::test]
  async fn start_failure_is_not_recorded_as_a_tool_call() {
    let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
    let config = config(directory.path(), "this-command-does-not-exist", Vec::new(), 1);
    let mut client = McpClient::new(config);
    let mut tools = ToolSet::new(&mut client, directory.path().to_path_buf());

    let error = tools
      .read_snapshot(None, &run_log_handle(directory.path().to_path_buf()))
      .await
      .expect_err("起動できない client は読み取れません");
    let metrics = tools.take_metrics();
    drop(tools);

    assert!(
      matches!(error, ToolError::Transport(TransportError::Spawn(_) | TransportError::Io(_))),
      "起動失敗は輸送エラーです: {error}"
    );
    assert_eq!(metrics.total_calls(), 0, "起動失敗を道具呼び出しとして記録しません");
    assert_eq!(metrics.total_failures(), 0);
  }

  #[tokio::test]
  async fn exit_before_initialize_reply_is_not_recorded_as_a_tool_call() {
    let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
    // initialize の要求を読んだ後、応答せずに終了する子プロセスです。
    let config = config(directory.path(), "sh", vec!["-c".to_owned(), "read -r _line; exit 0".to_owned()], 1);
    let mut client = McpClient::new(config);
    let mut tools = ToolSet::new(&mut client, directory.path().to_path_buf());

    let error = tools
      .read_snapshot(None, &run_log_handle(directory.path().to_path_buf()))
      .await
      .expect_err("initialize に応答がなければ読み取れません");
    let metrics = tools.take_metrics();
    drop(tools);

    assert!(
      matches!(error, ToolError::Transport(TransportError::ProcessExited)),
      "initialize の前の終了は輸送エラーです: {error}"
    );
    assert_eq!(metrics.total_calls(), 0, "起動中の終了を道具呼び出しとして記録しません");
    assert_eq!(metrics.total_failures(), 0, "起動中の終了を道具の失敗として記録しません");
    assert_eq!(metrics.total_timeouts(), 0, "起動中の終了を道具のタイムアウトとして記録しません");
  }

  #[test]
  fn classifies_tool_call_results_into_success_failure_and_timeout() {
    assert_eq!(call_outcome(&Ok("{}".to_owned())), ToolCallOutcome::Succeeded);
    assert_eq!(
      call_outcome(&Err(CallError::Tool(ToolCallError {
        tool:    "takt_list_goals".to_owned(),
        message: "失敗しました".to_owned(),
      }))),
      ToolCallOutcome::Failed
    );
    assert_eq!(call_outcome(&Err(CallError::Transport(TransportError::Timeout(15)))), ToolCallOutcome::TimedOut);
    assert_eq!(
      call_outcome(&Err(CallError::Transport(TransportError::Io("切断".to_owned())))),
      ToolCallOutcome::Failed
    );

    let mut metrics = ToolCallMetrics::default();
    let timeout = Err(CallError::Transport(TransportError::Timeout(15)));
    metrics.record("takt_list_goals", Duration::from_millis(4), call_outcome(&timeout));
    assert_eq!(metrics.total_timeouts(), 1);
    assert_eq!(metrics.total_failures(), 0, "タイムアウトを失敗に混ぜません");
  }
}

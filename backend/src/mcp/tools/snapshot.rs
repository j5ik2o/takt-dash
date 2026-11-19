//! プロジェクト snapshot の読み取り（ゴール・タスク・問い合わせ・通知・run の集約）。

use std::{
  collections::{BTreeMap, BTreeSet},
  time::Instant,
};

use chrono::{DateTime, Utc};
use serde_json::{json, Value};

use super::{
  call_outcome, degrades_to_previous, inherit_previous_notifications, is_terminal, is_timeout,
  is_unchanged_terminal_goal, parse_goal, parse_inquiry, parse_notification, parse_task, prev_inquiries, previous_run,
  resolve_awaiting_merge_at, resolve_inquiry, Page, ParsedInquiry, ToolError, ToolSet, MAX_PAGES,
};
use crate::{
  domain::GoalId,
  model::{
    GoalEntry, InquiryEntry, NotificationEntry, ProjectSnapshot, RunEntry, TaskEntry, TaskStatus, TerminalReads,
  },
  run_progress::{read_run, retain_runs, RunLogHandle, RunReadFailure},
};

impl ToolSet<'_> {
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

  pub(super) async fn call_json(&mut self, name: &str, extra: Value) -> Result<Value, ToolError> {
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
  pub(super) async fn list_tasks(&mut self) -> Result<Vec<TaskEntry>, ToolError> {
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
  /// 問い合わせへの回答の後に poller が前回の記録から外したゴールは、終端でも読み直します。
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
  /// SSH 接続の run、およびローカルの対応表に無い
  /// run（[`RunReadFailure::NotListed`]）と、対応表を読めない run
  /// （[`RunReadFailure::Unreadable`]）は、
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
      let local = if self.client.config().is_ssh() {
        Err(RunReadFailure::NotListed)
      } else {
        read_run(handle, run_slug.clone()).await
      };
      let (mut run, workflow, evidence) = match local {
        | Ok(view) => (view.run_entry(&run_slug, task), view.workflow, view.evidence),
        | Err(failure @ (RunReadFailure::NotListed | RunReadFailure::Unreadable(_))) => {
          match self.read_remote_run(&run_slug).await {
            | Ok(mut remote) => {
              self.enrich_remote_progress(&run_slug, &mut remote).await;
              (remote.run_entry(task), remote.workflow, remote.evidence)
            },
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
        | Some(workflow) => self.enrich_step_progress(&mut run, workflow, &evidence).await,
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
}
impl ToolSet<'_> {
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

//! 前回の状態と今回の状態の差分から出来事を検出する。
//!
//! 出来事は「前回の状態との差分」であり、前回が無い初回の読み取りでは何も出しません
//! （基準化のみ）。変化がなければ何も出しません。接続断は接続から非接続への遷移で1回だけ出します。
//! タスクと run の失敗も差分から出すため、backend の起動の直後の読み取りは、すでに失敗していた
//! ものを新しい失敗として出しません。

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

use crate::{
  domain::{GoalId, RunSlug},
  model::{
    ActivityEvent, ConnectionState, ConnectionTransition, EventKind, ProjectSnapshot, RunStatus, TaskEntry, TaskStatus,
  },
  store::{is_human_merge_wait, GoalTermination},
};

/// 1プロジェクトの前回状態と今回状態から、検出した出来事を返す。
///
/// `prev` が None のときは基準化のみで、ゴール・タスクなどの差分は出しません。
/// `transition` は今回の読み取りで生じた接続状態の遷移です。接続断は応答なしへ移ったときだけ、
/// 復帰は応答なしから接続中へ戻ったときだけ出します。
pub fn detect(
  server_id: &str,
  project: &str,
  prev: Option<&ProjectSnapshot>,
  next: &ProjectSnapshot,
  transition: Option<ConnectionTransition>,
  now: DateTime<Utc>,
) -> Vec<ActivityEvent> {
  detect_with_terminations(server_id, project, prev, next, transition, None, now)
}

/// 自動終了の記録を踏まえて出来事を検出する。
///
/// 判断待ちの射影と同じ規則（[`GoalTermination::suppresses_display`]）で除外されるゴールは、
/// 人のマージ待ちの出来事も記録しません。判断待ちに出ないゴールを「マージ待ちになった」と
/// 記録しないため、射影と出来事が同じ入力で判定します。
pub fn detect_with_terminations(
  server_id: &str,
  project: &str,
  prev: Option<&ProjectSnapshot>,
  next: &ProjectSnapshot,
  transition: Option<ConnectionTransition>,
  terminations: Option<&BTreeMap<GoalId, GoalTermination>>,
  now: DateTime<Utc>,
) -> Vec<ActivityEvent> {
  let mut events = Vec::new();
  if let Some(transition) = transition {
    if transition.next == Some(ConnectionState::Unresponsive)
      && transition.previous != Some(ConnectionState::Unresponsive)
    {
      events.push(event(
        now,
        server_id,
        Some(project),
        EventKind::ConnectionLost,
        format!("{project} の takt-mcp から応答が返らなくなりました。"),
        None,
      ));
    }
    if transition.previous == Some(ConnectionState::Unresponsive) && transition.next == Some(ConnectionState::Connected)
    {
      let outage = transition.outage.unwrap_or_else(chrono::Duration::zero).num_seconds().max(0);
      events.push(event(
        now,
        server_id,
        Some(project),
        EventKind::ConnectionRestored,
        format!("{project} の takt-mcp との接続が復帰しました。切断していた時間は {outage} 秒です。"),
        None,
      ));
    }
  }
  let Some(prev) = prev else {
    return events;
  };
  if transition.is_some_and(|transition| {
    !matches!(transition.next, Some(ConnectionState::Connected) | Some(ConnectionState::Delayed))
  }) {
    return events;
  }
  detect_goals(server_id, project, prev, next, terminations, now, &mut events);
  detect_tasks(server_id, project, prev, next, now, &mut events);
  detect_runs(server_id, project, prev, next, now, &mut events);
  detect_inquiries(server_id, project, prev, next, now, &mut events);
  detect_notifications(server_id, project, prev, next, now, &mut events);
  events
}

fn detect_goals(
  server_id: &str,
  project: &str,
  prev: &ProjectSnapshot,
  next: &ProjectSnapshot,
  terminations: Option<&BTreeMap<GoalId, GoalTermination>>,
  now: DateTime<Utc>,
  events: &mut Vec<ActivityEvent>,
) {
  for goal in &next.goals {
    let previous = prev.goals.iter().find(|entry| entry.goal_id == goal.goal_id);
    let name = goal.objective.trim().to_string();
    match previous {
      | None => events.push(event(
        now,
        server_id,
        Some(project),
        EventKind::GoalCreated,
        format!("ゴール「{name}」が登録されました。"),
        None,
      )),
      | Some(previous) => {
        // 人のマージ待ちは、判断待ち一覧と同じ規則（仕上げタスクを見る）を前回と今回の両方へ
        // 適用し、人待ちでない状態から人待ちへ変わった回だけ記録します。判断待ち一覧と同じく、
        // 対象を仕上げタスクに絞ってから判定します。
        let prev_tasks: Vec<&crate::model::TaskEntry> = prev.tasks.iter().filter(|task| task.merge_task).collect();
        let next_tasks: Vec<&crate::model::TaskEntry> = next.tasks.iter().filter(|task| task.merge_task).collect();
        let suppressed =
          terminations.and_then(|records| records.get(&goal.goal_id)).is_some_and(GoalTermination::suppresses_display);
        let previous_wait = is_human_merge_wait(previous.status, &goal.goal_id, &prev_tasks);
        let current_wait = !suppressed && is_human_merge_wait(goal.status, &goal.goal_id, &next_tasks);
        if !previous_wait && current_wait {
          events.push(event(
            now,
            server_id,
            Some(project),
            EventKind::AwaitingMerge,
            format!("ゴール「{name}」が人のマージ待ちになりました。"),
            None,
          ));
        }
        if previous.execution_status != crate::model::ExecutionStatus::Paused
          && goal.execution_status == crate::model::ExecutionStatus::Paused
        {
          events.push(event(
            now,
            server_id,
            Some(project),
            EventKind::Paused,
            format!("ゴール「{name}」が一時停止されました。"),
            None,
          ));
        }
      },
    }
  }
}

fn detect_tasks(
  server_id: &str,
  project: &str,
  prev: &ProjectSnapshot,
  next: &ProjectSnapshot,
  now: DateTime<Utc>,
  events: &mut Vec<ActivityEvent>,
) {
  for task in &next.tasks {
    let previous = previous_task(prev, task);
    if task.status == TaskStatus::Running && previous.map(|entry| entry.status) != Some(TaskStatus::Running) {
      events.push(event(
        now,
        server_id,
        Some(project),
        EventKind::RunStarted,
        format!("タスク「{}」の run が始まりました。", task.name),
        task.run_slug.clone(),
      ));
    }
    if task.status == TaskStatus::Completed && previous.map(|entry| entry.status) != Some(TaskStatus::Completed) {
      events.push(event(
        now,
        server_id,
        Some(project),
        EventKind::TaskCompleted,
        format!("タスク「{}」が完了しました。", task.name),
        task.run_slug.clone(),
      ));
    }
    if let Some((kind, outcome)) = task_failure(task.status) {
      if previous.map(|entry| entry.status) != Some(task.status) {
        events.push(event(
          now,
          server_id,
          Some(project),
          kind,
          format!("タスク「{}」が{outcome}。", task.name),
          task.run_slug.clone(),
        ));
      }
    }
  }
}

/// 前回の読み取りで同じタスクを探す。
///
/// タスク名の一意性はゴール内に限られるため（`docs/domain-primitives.md`）、名前だけで探すと
/// 別のゴールにある同名タスクの状態を前回の状態として使ってしまいます。ゴール ID とタスク名の
/// 組で照合します。
fn previous_task<'a>(prev: &'a ProjectSnapshot, task: &TaskEntry) -> Option<&'a TaskEntry> {
  prev.tasks.iter().find(|entry| entry.name == task.name && entry.goal_id == task.goal_id)
}

/// タスクの状態が失敗の一種なら、出来事の種類と本文の述部を返す。
fn task_failure(status: TaskStatus) -> Option<(EventKind, &'static str)> {
  match status {
    | TaskStatus::Failed => Some((EventKind::TaskFailed, "失敗しました")),
    | TaskStatus::Exceeded => Some((EventKind::TaskExceeded, "上限を超えて止まりました")),
    | TaskStatus::PrFailed => Some((EventKind::TaskPrFailed, "プルリクエストの作成に失敗しました")),
    | TaskStatus::Pending | TaskStatus::Running | TaskStatus::Completed => None,
  }
}

/// run の失敗と中断を検出する。
///
/// snapshot が持つ run は、タスクが running のあいだの run だけです。タスクが running のまま
/// run だけが失敗か中断で終わった場合を、ここで出来事にします。タスクが失敗へ移った場合は
/// [`detect_tasks`] が出来事にします。
fn detect_runs(
  server_id: &str,
  project: &str,
  prev: &ProjectSnapshot,
  next: &ProjectSnapshot,
  now: DateTime<Utc>,
  events: &mut Vec<ActivityEvent>,
) {
  for run in &next.runs {
    let (kind, outcome) = match run.status {
      | RunStatus::Failed => (EventKind::RunFailed, "失敗で終わりました"),
      | RunStatus::Aborted => (EventKind::RunAborted, "中断されました"),
      | RunStatus::Running | RunStatus::Completed => continue,
    };
    let previous = prev.runs.iter().find(|entry| entry.run_slug == run.run_slug);
    if previous.map(|entry| entry.status) != Some(run.status) {
      events.push(event(
        now,
        server_id,
        Some(project),
        kind,
        format!("タスク「{}」の run が{outcome}。", run.task_name),
        RunSlug::try_from(run.run_slug.as_str()).ok(),
      ));
    }
  }
}

fn detect_inquiries(
  server_id: &str,
  project: &str,
  prev: &ProjectSnapshot,
  next: &ProjectSnapshot,
  now: DateTime<Utc>,
  events: &mut Vec<ActivityEvent>,
) {
  for inquiry in &next.inquiries {
    if inquiry.answered {
      continue;
    }
    let known = prev.inquiries.iter().any(|entry| entry.inquiry_id == inquiry.inquiry_id);
    if !known {
      events.push(event(
        now,
        server_id,
        Some(project),
        EventKind::Inquiry,
        format!("問い合わせ「{}」を受け付けました。", inquiry.body),
        None,
      ));
    }
  }
}

fn detect_notifications(
  server_id: &str,
  project: &str,
  prev: &ProjectSnapshot,
  next: &ProjectSnapshot,
  now: DateTime<Utc>,
  events: &mut Vec<ActivityEvent>,
) {
  for notification in &next.notifications {
    if notification.kind != "blocked" {
      continue;
    }
    let known = prev.notifications.iter().any(|entry| entry.notification_id == notification.notification_id);
    if !known {
      events.push(event(
        now,
        server_id,
        Some(project),
        EventKind::Blocked,
        format!("manager が blocked を通知しました。{}", notification.body),
        None,
      ));
    }
  }
}

/// backend の起動の出来事を作る。
///
/// 起動はどのサーバーにもプロジェクトにも属さないため、サーバーは空、プロジェクトは無しにします。
pub fn backend_started(occurred_at: DateTime<Utc>) -> ActivityEvent {
  event(occurred_at, "", None, EventKind::BackendStarted, "taktDash の backend が起動しました。".to_owned(), None)
}

fn event(
  occurred_at: DateTime<Utc>,
  server_id: &str,
  project: Option<&str>,
  kind: EventKind,
  body: String,
  run_slug: Option<RunSlug>,
) -> ActivityEvent {
  ActivityEvent {
    occurred_at,
    server_id: server_id.to_owned(),
    project: project.map(str::to_owned),
    kind,
    body,
    run_slug,
  }
}

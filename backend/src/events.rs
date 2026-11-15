//! 前回の状態と今回の状態の差分から出来事を検出する。
//!
//! 出来事は「前回の状態との差分」であり、前回が無い初回の読み取りでは何も出しません
//! （基準化のみ）。変化がなければ何も出しません。接続断は接続から非接続への遷移で1回だけ出します。

use chrono::{DateTime, Utc};

use crate::{
  model::{ActivityEvent, EventKind, ProjectSnapshot, TaskStatus},
  store::is_human_merge_wait,
};

/// 1プロジェクトの前回状態と今回状態から、検出した出来事を返す。
///
/// `prev` が None のときは基準化のみで、出来事を返しません。
/// `connected` は今回の読み取りで応答が得られたかどうか、`prev_connected` は前回の応答の有無です。
pub fn detect(
  server_id: &str,
  project: &str,
  prev: Option<&ProjectSnapshot>,
  prev_connected: bool,
  connected: bool,
  next: &ProjectSnapshot,
  now: DateTime<Utc>,
) -> Vec<ActivityEvent> {
  let mut events = Vec::new();
  if prev_connected && !connected {
    events.push(event(
      now,
      server_id,
      Some(project),
      EventKind::ConnectionLost,
      format!("{project} の takt-mcp から応答が返らなくなりました。"),
      None,
    ));
  }
  let Some(prev) = prev else {
    return events;
  };
  if !connected {
    return events;
  }
  detect_goals(server_id, project, prev, next, now, &mut events);
  detect_tasks(server_id, project, prev, next, now, &mut events);
  detect_inquiries(server_id, project, prev, next, now, &mut events);
  detect_notifications(server_id, project, prev, next, now, &mut events);
  events
}

fn detect_goals(
  server_id: &str,
  project: &str,
  prev: &ProjectSnapshot,
  next: &ProjectSnapshot,
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
        let previous_wait = is_human_merge_wait(previous.status, &goal.goal_id, &prev_tasks);
        let current_wait = is_human_merge_wait(goal.status, &goal.goal_id, &next_tasks);
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
    let previous = prev.tasks.iter().find(|entry| entry.name == task.name);
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

fn event(
  occurred_at: DateTime<Utc>,
  server_id: &str,
  project: Option<&str>,
  kind: EventKind,
  body: String,
  run_slug: Option<String>,
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

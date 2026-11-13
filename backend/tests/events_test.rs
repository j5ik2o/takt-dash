//! 出来事検出の純粋関数を検証する。8種の検出、初回の基準化、変化なし、接続断の1回だけを確かめる。

mod common;

use common::*;
use takt_dash_backend::{
  events::detect,
  model::{EventKind, ExecutionStatus, GoalStatus, ProjectSnapshot, TaskEntry, TaskStatus},
};

const SERVER: &str = "mac-mini";
const PROJECT: &str = "takt-dash";

fn ids(events: &[takt_dash_backend::model::ActivityEvent]) -> Vec<EventKind> {
  events.iter().map(|event| event.kind).collect()
}

#[test]
fn first_read_only_establishes_baseline_without_events() {
  let now = time("2026-11-13T10:00:00Z");
  let next = snapshot(vec![goal("g1", GoalStatus::Created, ExecutionStatus::Active)], vec![], vec![], vec![], vec![]);

  let events = detect(SERVER, PROJECT, None, false, true, &next, now);

  assert!(events.is_empty(), "初回は差分が定義されないため出来事を出しません: {:?}", ids(&events));
}

#[test]
fn unchanged_state_emits_no_events() {
  let now = time("2026-11-13T10:00:00Z");
  let prev = snapshot(
    vec![goal("g1", GoalStatus::Created, ExecutionStatus::Active)],
    vec![task("a", TaskStatus::Completed)],
    vec![inquiry("q1", false, now)],
    vec![],
    vec![],
  );

  let events = detect(SERVER, PROJECT, Some(&prev), true, true, &prev, now);

  assert!(events.is_empty(), "変化がないので出来事を出しません: {:?}", ids(&events));
}

#[test]
fn new_goal_is_detected_as_goal_created() {
  let now = time("2026-11-13T10:00:00Z");
  let prev = snapshot(vec![], vec![], vec![], vec![], vec![]);
  let next = snapshot(vec![goal("g1", GoalStatus::Created, ExecutionStatus::Active)], vec![], vec![], vec![], vec![]);

  let events = detect(SERVER, PROJECT, Some(&prev), true, true, &next, now);

  assert_eq!(ids(&events), vec![EventKind::GoalCreated]);
  assert_eq!(events[0].server_id, SERVER);
  assert_eq!(events[0].project.as_deref(), Some(PROJECT));
  assert_eq!(events[0].occurred_at, now);
}

#[test]
fn task_becoming_running_is_detected_as_run_started_with_run_slug() {
  let now = time("2026-11-13T10:00:00Z");
  let prev = snapshot(vec![], vec![task("a", TaskStatus::Pending)], vec![], vec![], vec![]);
  let running = TaskEntry { run_slug: Some("20261113-abc".to_owned()), ..task("a", TaskStatus::Running) };
  let next = snapshot(vec![], vec![running], vec![], vec![], vec![]);

  let events = detect(SERVER, PROJECT, Some(&prev), true, true, &next, now);

  assert_eq!(ids(&events), vec![EventKind::RunStarted]);
  assert_eq!(events[0].run_slug.as_deref(), Some("20261113-abc"));
}

#[test]
fn task_becoming_completed_is_detected_as_task_completed() {
  let now = time("2026-11-13T10:00:00Z");
  let prev = snapshot(vec![], vec![task("a", TaskStatus::Running)], vec![], vec![], vec![]);
  let next = snapshot(vec![], vec![task("a", TaskStatus::Completed)], vec![], vec![], vec![]);

  let events = detect(SERVER, PROJECT, Some(&prev), true, true, &next, now);

  assert_eq!(ids(&events), vec![EventKind::TaskCompleted]);
}

#[test]
fn newly_unanswered_inquiry_is_detected_but_answered_one_is_not() {
  let now = time("2026-11-13T10:00:00Z");
  let prev = snapshot(vec![], vec![], vec![], vec![], vec![]);
  let next =
    snapshot(vec![], vec![], vec![inquiry("q-open", false, now), inquiry("q-done", true, now)], vec![], vec![]);

  let events = detect(SERVER, PROJECT, Some(&prev), true, true, &next, now);

  assert_eq!(ids(&events), vec![EventKind::Inquiry], "未回答の問い合わせだけを検出します");
}

#[test]
fn goal_transition_to_awaiting_merge_is_detected() {
  let now = time("2026-11-13T10:00:00Z");
  let prev = snapshot(vec![goal("g1", GoalStatus::Created, ExecutionStatus::Active)], vec![], vec![], vec![], vec![]);
  let next = snapshot(vec![merge_goal("g1", 42)], vec![], vec![], vec![], vec![]);

  let events = detect(SERVER, PROJECT, Some(&prev), true, true, &next, now);

  assert_eq!(ids(&events), vec![EventKind::AwaitingMerge]);
}

#[test]
fn already_awaiting_merge_goal_does_not_repeat_the_event() {
  let now = time("2026-11-13T10:00:00Z");
  let prev = snapshot(vec![merge_goal("g1", 42)], vec![], vec![], vec![], vec![]);

  let events = detect(SERVER, PROJECT, Some(&prev), true, true, &prev, now);

  assert!(events.is_empty(), "状態が同じなので繰り返しません: {:?}", ids(&events));
}

#[test]
fn paused_execution_is_detected_once() {
  let now = time("2026-11-13T10:00:00Z");
  let prev = snapshot(vec![goal("g1", GoalStatus::Created, ExecutionStatus::Active)], vec![], vec![], vec![], vec![]);
  let next = snapshot(vec![goal("g1", GoalStatus::Created, ExecutionStatus::Paused)], vec![], vec![], vec![], vec![]);

  let events = detect(SERVER, PROJECT, Some(&prev), true, true, &next, now);

  assert_eq!(ids(&events), vec![EventKind::Paused]);
}

#[test]
fn new_blocked_notification_is_detected_but_other_kinds_are_ignored() {
  let now = time("2026-11-13T10:00:00Z");
  let prev = snapshot(vec![], vec![], vec![], vec![], vec![]);
  let next = snapshot(vec![], vec![], vec![], vec![], vec![
    notification("n-blocked", "blocked", now),
    notification("n-progress", "progress", now),
    notification("n-merge", "awaiting_merge", now),
  ]);

  let events = detect(SERVER, PROJECT, Some(&prev), true, true, &next, now);

  assert_eq!(ids(&events), vec![EventKind::Blocked], "blocked の通知の種類だけを検出します");
}

#[test]
fn connection_lost_is_detected_once_on_transition_from_connected() {
  let now = time("2026-11-13T10:00:00Z");
  let prev = snapshot(vec![], vec![], vec![], vec![], vec![]);

  let transition = detect(SERVER, PROJECT, Some(&prev), true, false, &prev, now);
  assert_eq!(ids(&transition), vec![EventKind::ConnectionLost]);

  let still_down = detect(SERVER, PROJECT, Some(&prev), false, false, &prev, now);
  assert!(still_down.is_empty(), "応答なしが続いても接続断を重ねません: {:?}", ids(&still_down));
}

#[test]
fn notifications_are_compared_by_id_not_by_count() {
  let now = time("2026-11-13T10:00:00Z");
  let prev = snapshot(vec![], vec![], vec![], vec![], vec![
    notification("n-1", "blocked", now),
    notification("n-2", "blocked", now),
  ]);
  let next = snapshot(vec![], vec![], vec![], vec![], vec![
    notification("n-1", "blocked", now),
    notification("n-2", "blocked", now),
    notification("n-3", "blocked", now),
  ]);

  let events = detect(SERVER, PROJECT, Some(&prev), true, true, &next, now);

  assert_eq!(ids(&events), vec![EventKind::Blocked]);
  assert_eq!(events.len(), 1, "新しい通知の ID だけを1件検出します");
}

#[test]
fn baseline_of_empty_snapshot_with_existing_goal_emits_nothing() {
  let now = time("2026-11-13T10:00:00Z");
  let empty: ProjectSnapshot = ProjectSnapshot::empty();
  let next = snapshot(vec![goal("g1", GoalStatus::Created, ExecutionStatus::Active)], vec![], vec![], vec![], vec![]);

  let events = detect(SERVER, PROJECT, Some(&empty), true, true, &next, now);

  assert_eq!(ids(&events), vec![EventKind::GoalCreated]);
}

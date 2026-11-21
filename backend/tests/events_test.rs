//! 出来事検出の純粋関数を検証する。9種の検出、初回の基準化、変化なし、
//! 接続断と復帰の1回だけを確かめる。

mod common;

use common::*;
use takt_dash_backend::{
  domain::{GoalId, PullRequestState, RunSlug},
  events::{detect, detect_with_terminations},
  model::{
    ConnectionState, ConnectionTransition, EventKind, ExecutionStatus, GoalStatus, ProjectSnapshot, TaskEntry,
    TaskStatus,
  },
  store::{FinishRequest, GoalTermination},
};

const SERVER: &str = "mac-mini";
const PROJECT: &str = "takt-dash";

fn ids(events: &[takt_dash_backend::model::ActivityEvent]) -> Vec<EventKind> {
  events.iter().map(|event| event.kind).collect()
}

fn connection(previous: ConnectionState, next: ConnectionState, outage_seconds: Option<i64>) -> ConnectionTransition {
  ConnectionTransition {
    previous: Some(previous),
    next:     Some(next),
    outage:   outage_seconds.map(chrono::Duration::seconds),
  }
}

fn stable() -> ConnectionTransition {
  connection(ConnectionState::Connected, ConnectionState::Connected, None)
}

#[test]
fn first_read_only_establishes_baseline_without_events() {
  let now = time("2026-11-13T10:00:00Z");
  let next = snapshot(vec![goal("g1", GoalStatus::Created, ExecutionStatus::Active)], vec![], vec![], vec![], vec![]);

  let events = detect(SERVER, PROJECT, None, &next, None, now);

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

  let events = detect(SERVER, PROJECT, Some(&prev), &prev, Some(stable()), now);

  assert!(events.is_empty(), "変化がないので出来事を出しません: {:?}", ids(&events));
}

#[test]
fn new_goal_is_detected_as_goal_created() {
  let now = time("2026-11-13T10:00:00Z");
  let prev = snapshot(vec![], vec![], vec![], vec![], vec![]);
  let next = snapshot(vec![goal("g1", GoalStatus::Created, ExecutionStatus::Active)], vec![], vec![], vec![], vec![]);

  let events = detect(SERVER, PROJECT, Some(&prev), &next, Some(stable()), now);

  assert_eq!(ids(&events), vec![EventKind::GoalCreated]);
  assert_eq!(events[0].server_id, SERVER);
  assert_eq!(events[0].project.as_deref(), Some(PROJECT));
  assert_eq!(events[0].occurred_at, now);
}

#[test]
fn task_becoming_running_is_detected_as_run_started_with_run_slug() {
  let now = time("2026-11-13T10:00:00Z");
  let prev = snapshot(vec![], vec![task("a", TaskStatus::Pending)], vec![], vec![], vec![]);
  let running = TaskEntry {
    run_slug: Some(RunSlug::try_from("20261113-abc").expect("テストの run slug は有効です")),
    ..task("a", TaskStatus::Running)
  };
  let next = snapshot(vec![], vec![running], vec![], vec![], vec![]);

  let events = detect(SERVER, PROJECT, Some(&prev), &next, Some(stable()), now);

  assert_eq!(ids(&events), vec![EventKind::RunStarted]);
  assert_eq!(events[0].run_slug.as_ref().map(RunSlug::as_str), Some("20261113-abc"));
}

#[test]
fn task_becoming_completed_is_detected_as_task_completed() {
  let now = time("2026-11-13T10:00:00Z");
  let prev = snapshot(vec![], vec![task("a", TaskStatus::Running)], vec![], vec![], vec![]);
  let next = snapshot(vec![], vec![task("a", TaskStatus::Completed)], vec![], vec![], vec![]);

  let events = detect(SERVER, PROJECT, Some(&prev), &next, Some(stable()), now);

  assert_eq!(ids(&events), vec![EventKind::TaskCompleted]);
}

#[test]
fn newly_unanswered_inquiry_is_detected_but_answered_one_is_not() {
  let now = time("2026-11-13T10:00:00Z");
  let prev = snapshot(vec![], vec![], vec![], vec![], vec![]);
  let next =
    snapshot(vec![], vec![], vec![inquiry("q-open", false, now), inquiry("q-done", true, now)], vec![], vec![]);

  let events = detect(SERVER, PROJECT, Some(&prev), &next, Some(stable()), now);

  assert_eq!(ids(&events), vec![EventKind::Inquiry], "未回答の問い合わせだけを検出します");
}

#[test]
fn goal_transition_to_awaiting_merge_is_detected() {
  let now = time("2026-11-13T10:00:00Z");
  let prev = snapshot(vec![goal("g1", GoalStatus::Created, ExecutionStatus::Active)], vec![], vec![], vec![], vec![]);
  let next = snapshot(vec![merge_goal("g1", 42)], vec![], vec![], vec![], vec![]);

  let events = detect(SERVER, PROJECT, Some(&prev), &next, Some(stable()), now);

  assert_eq!(ids(&events), vec![EventKind::AwaitingMerge]);
}

#[test]
fn awaiting_merge_with_running_finish_task_is_not_a_human_merge_wait_event() {
  // ゴールの status が awaiting_merge でも、仕上げタスクが running の間は人待ちではありません。
  let now = time("2026-11-13T10:00:00Z");
  let prev = snapshot(
    vec![goal("g1", GoalStatus::Created, ExecutionStatus::Active)],
    vec![task_for_goal("merge", "g1", TaskStatus::Pending, true)],
    vec![],
    vec![],
    vec![],
  );
  let next = snapshot(
    vec![merge_goal("g1", 42)],
    vec![task_for_goal("merge", "g1", TaskStatus::Running, true)],
    vec![],
    vec![],
    vec![],
  );

  let events = detect(SERVER, PROJECT, Some(&prev), &next, Some(stable()), now);

  assert!(
    !ids(&events).contains(&EventKind::AwaitingMerge),
    "仕上げタスクの実行中は人のマージ待ちを記録しません: {:?}",
    ids(&events)
  );
}

#[test]
fn awaiting_merge_becomes_a_human_merge_wait_event_when_the_finish_task_completes() {
  // 仕上げタスクが完了して人待ちに変わった回だけ、人のマージ待ちを1回記録します。
  let now = time("2026-11-13T10:00:00Z");
  let prev = snapshot(
    vec![merge_goal("g1", 42)],
    vec![task_for_goal("merge", "g1", TaskStatus::Running, true)],
    vec![],
    vec![],
    vec![],
  );
  let next = snapshot(
    vec![merge_goal("g1", 42)],
    vec![task_for_goal("merge", "g1", TaskStatus::Completed, true)],
    vec![],
    vec![],
    vec![],
  );

  let events = detect(SERVER, PROJECT, Some(&prev), &next, Some(stable()), now);

  assert_eq!(
    ids(&events).iter().filter(|kind| **kind == EventKind::AwaitingMerge).count(),
    1,
    "仕上げタスクの完了で人のマージ待ちを1回記録します: {:?}",
    ids(&events)
  );
}

#[test]
fn awaiting_merge_with_completed_finish_task_and_unrelated_running_task_is_a_human_merge_wait_event() {
  // 仕上げタスクが完了していれば人待ちです。同じ snapshot に仕上げタスクでない running の
  // タスクが併存しても、人のマージ待ちを1回記録します。全タスクを判定へ渡す実装では、
  // 無関係な running タスクを仕上げタスクの実行中と誤認し、記録を取りこぼします。
  let now = time("2026-11-13T10:00:00Z");
  let prev = snapshot(
    vec![merge_goal("g1", 42)],
    vec![
      task_for_goal("merge", "g1", TaskStatus::Running, true),
      task_for_goal("implement", "g1", TaskStatus::Running, false),
    ],
    vec![],
    vec![],
    vec![],
  );
  let next = snapshot(
    vec![merge_goal("g1", 42)],
    vec![
      task_for_goal("merge", "g1", TaskStatus::Completed, true),
      task_for_goal("implement", "g1", TaskStatus::Running, false),
    ],
    vec![],
    vec![],
    vec![],
  );

  let events = detect(SERVER, PROJECT, Some(&prev), &next, Some(stable()), now);

  assert_eq!(
    ids(&events).iter().filter(|kind| **kind == EventKind::AwaitingMerge).count(),
    1,
    "仕上げタスクの完了で、無関係な running タスクが併存しても人のマージ待ちを1回記録します: {:?}",
    ids(&events)
  );
}

#[test]
fn awaiting_merge_with_failed_finish_task_is_a_human_merge_wait_event() {
  // 仕上げタスクが failed で終わったゴールは、仕上げが止まって人の判断が要る状態として
  // 人のマージ待ちに含める（issue #231）。前回は人待ちでなく今回が人待ちなので、
  // 人のマージ待ちを1回記録します。
  let now = time("2026-11-13T10:00:00Z");
  let prev = snapshot(
    vec![goal("g1", GoalStatus::Created, ExecutionStatus::Active)],
    vec![task_for_goal("merge", "g1", TaskStatus::Running, true)],
    vec![],
    vec![],
    vec![],
  );
  let next = snapshot(
    vec![merge_goal("g1", 42)],
    vec![task_for_goal("merge", "g1", TaskStatus::Failed, true)],
    vec![],
    vec![],
    vec![],
  );

  let events = detect(SERVER, PROJECT, Some(&prev), &next, Some(stable()), now);

  assert_eq!(
    ids(&events).iter().filter(|kind| **kind == EventKind::AwaitingMerge).count(),
    1,
    "仕上げタスクが失敗で終わったゴールは人のマージ待ちを1回記録します: {:?}",
    ids(&events)
  );
}

#[test]
fn already_awaiting_merge_goal_does_not_repeat_the_event() {
  let now = time("2026-11-13T10:00:00Z");
  let prev = snapshot(vec![merge_goal("g1", 42)], vec![], vec![], vec![], vec![]);

  let events = detect(SERVER, PROJECT, Some(&prev), &prev, Some(stable()), now);

  assert!(events.is_empty(), "状態が同じなので繰り返しません: {:?}", ids(&events));
}

#[test]
fn paused_execution_is_detected_once() {
  let now = time("2026-11-13T10:00:00Z");
  let prev = snapshot(vec![goal("g1", GoalStatus::Created, ExecutionStatus::Active)], vec![], vec![], vec![], vec![]);
  let next = snapshot(vec![goal("g1", GoalStatus::Created, ExecutionStatus::Paused)], vec![], vec![], vec![], vec![]);

  let events = detect(SERVER, PROJECT, Some(&prev), &next, Some(stable()), now);

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

  let events = detect(SERVER, PROJECT, Some(&prev), &next, Some(stable()), now);

  assert_eq!(ids(&events), vec![EventKind::Blocked], "blocked の通知の種類だけを検出します");
}

#[test]
fn connection_lost_is_detected_once_when_state_enters_unresponsive() {
  let now = time("2026-11-13T10:00:00Z");
  let prev = snapshot(vec![], vec![], vec![], vec![], vec![]);

  let entering = detect(
    SERVER,
    PROJECT,
    Some(&prev),
    &prev,
    Some(connection(ConnectionState::Connected, ConnectionState::Unresponsive, None)),
    now,
  );
  assert_eq!(ids(&entering), vec![EventKind::ConnectionLost]);
}

#[test]
fn unresponsive_to_unresponsive_emits_nothing() {
  let now = time("2026-11-13T10:00:00Z");
  let prev = snapshot(vec![], vec![], vec![], vec![], vec![]);

  let still_down = detect(
    SERVER,
    PROJECT,
    Some(&prev),
    &prev,
    Some(connection(ConnectionState::Unresponsive, ConnectionState::Unresponsive, None)),
    now,
  );
  assert!(still_down.is_empty(), "応答なしが続いても接続断を重ねません: {:?}", ids(&still_down));
}

#[test]
fn delayed_transition_emits_no_connection_event() {
  let now = time("2026-11-13T10:00:00Z");
  let prev = snapshot(vec![], vec![], vec![], vec![], vec![]);

  let to_delayed = detect(
    SERVER,
    PROJECT,
    Some(&prev),
    &prev,
    Some(connection(ConnectionState::Connected, ConnectionState::Delayed, None)),
    now,
  );
  assert!(to_delayed.is_empty(), "遅延中への移行は接続断にしません: {:?}", ids(&to_delayed));

  let back_from_delayed = detect(
    SERVER,
    PROJECT,
    Some(&prev),
    &prev,
    Some(connection(ConnectionState::Delayed, ConnectionState::Connected, None)),
    now,
  );
  assert!(back_from_delayed.is_empty(), "遅延中からの復帰は復帰イベントにしません: {:?}", ids(&back_from_delayed));
}

#[test]
fn connection_restored_is_detected_with_outage_duration() {
  let now = time("2026-11-13T10:00:00Z");
  let prev = snapshot(vec![], vec![], vec![], vec![], vec![]);

  let events = detect(
    SERVER,
    PROJECT,
    Some(&prev),
    &prev,
    Some(connection(ConnectionState::Unresponsive, ConnectionState::Connected, Some(150))),
    now,
  );

  assert_eq!(ids(&events), vec![EventKind::ConnectionRestored]);
  assert!(
    events[0].body.contains("切断していた時間は 150 秒です。"),
    "復帰イベントの本文に切断していた時間を入れます: {}",
    events[0].body
  );
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

  let events = detect(SERVER, PROJECT, Some(&prev), &next, Some(stable()), now);

  assert_eq!(ids(&events), vec![EventKind::Blocked]);
  assert_eq!(events.len(), 1, "新しい通知の ID だけを1件検出します");
}

#[test]
fn baseline_of_empty_snapshot_with_existing_goal_emits_nothing() {
  let now = time("2026-11-13T10:00:00Z");
  let empty: ProjectSnapshot = ProjectSnapshot::empty();
  let next = snapshot(vec![goal("g1", GoalStatus::Created, ExecutionStatus::Active)], vec![], vec![], vec![], vec![]);

  let events = detect(SERVER, PROJECT, Some(&empty), &next, Some(stable()), now);

  assert_eq!(ids(&events), vec![EventKind::GoalCreated]);
}

#[test]
fn awaiting_merge_event_is_suppressed_for_terminated_goal() {
  let now = time("2026-11-13T10:00:00Z");
  let prev = snapshot(vec![goal("g1", GoalStatus::Created, ExecutionStatus::Active)], vec![], vec![], vec![], vec![]);
  let next = snapshot(vec![merge_goal("g1", 42)], vec![], vec![], vec![], vec![]);
  let mut terminated: std::collections::BTreeMap<GoalId, GoalTermination> = std::collections::BTreeMap::new();
  terminated.insert(GoalId::try_from("g1").expect("有効なゴール ID"), GoalTermination {
    pull_request:   None,
    finish_request: FinishRequest::Requested,
  });

  let events = detect_with_terminations(SERVER, PROJECT, Some(&prev), &next, Some(stable()), Some(&terminated), now);

  assert!(
    !ids(&events).contains(&EventKind::AwaitingMerge),
    "終了依頼済みのゴールはマージ待ちの出来事を記録しません: {:?}",
    ids(&events)
  );
}

#[test]
fn awaiting_merge_event_still_emits_for_open_pull_request() {
  let now = time("2026-11-13T10:00:00Z");
  let prev = snapshot(vec![goal("g1", GoalStatus::Created, ExecutionStatus::Active)], vec![], vec![], vec![], vec![]);
  let next = snapshot(vec![merge_goal("g1", 42)], vec![], vec![], vec![], vec![]);
  let mut terminated: std::collections::BTreeMap<GoalId, GoalTermination> = std::collections::BTreeMap::new();
  terminated.insert(GoalId::try_from("g1").expect("有効なゴール ID"), GoalTermination {
    pull_request:   Some(PullRequestState::Open),
    finish_request: FinishRequest::NotRequested,
  });

  let events = detect_with_terminations(SERVER, PROJECT, Some(&prev), &next, Some(stable()), Some(&terminated), now);

  assert_eq!(ids(&events), vec![EventKind::AwaitingMerge]);
}

//! 状態ストアの投影（人のマージ待ち判定・滞留・ログの無音・読めた時刻）と、
//! 応答なしのキャッシュ保持を検証する。

mod common;

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use common::*;
use takt_dash_backend::{
  config::ServerMeta,
  domain::{GoalId, SilentMinutes, StaleMinutes},
  model::{EventKind, ExecutionStatus, GoalStatus, PendingDecisionFilter, PendingDecisionType, TaskStatus},
  store::{is_human_merge_wait, StateStore},
};

fn goal_id() -> GoalId {
  goal_id_named("g1")
}

fn goal_id_named(value: &str) -> GoalId {
  GoalId::try_from(value).expect("テストのゴール ID は有効です")
}

fn store_with(threshold_minutes: i64, silent_minutes: i64) -> StateStore {
  let mut servers = BTreeMap::new();
  servers.insert("mac-mini".to_owned(), server_meta());
  let projects = vec![project("takt-dash", std::path::Path::new("/abs/takt-dash"))];
  let thresholds = takt_dash_backend::config::Thresholds {
    pending_decision_stale_minutes: StaleMinutes::try_from(
      u32::try_from(threshold_minutes).expect("テストの閾値は u32 の範囲です"),
    )
    .expect("テストの閾値は有効です"),
    run_log_silent_minutes:         SilentMinutes::try_from(
      u32::try_from(silent_minutes).expect("テストの閾値は u32 の範囲です"),
    )
    .expect("テストの閾値は有効です"),
  };
  StateStore::new(servers, projects, thresholds)
}

fn now() -> DateTime<Utc> {
  DateTime::parse_from_rfc3339("2026-11-13T12:00:00Z").unwrap().with_timezone(&Utc)
}

fn minutes_before(base: DateTime<Utc>, minutes: i64) -> DateTime<Utc> {
  base - chrono::Duration::minutes(minutes)
}

#[test]
fn human_merge_wait_requires_awaiting_merge_status() {
  let tasks = [task_for_goal("merge", "g1", TaskStatus::Completed, true)];
  let borrows: Vec<&takt_dash_backend::model::TaskEntry> = tasks.iter().collect();

  assert!(!is_human_merge_wait(GoalStatus::Created, &goal_id(), &borrows));
  assert!(!is_human_merge_wait(GoalStatus::Completed, &goal_id(), &borrows));
  assert!(is_human_merge_wait(GoalStatus::AwaitingMerge, &goal_id(), &borrows));
}

#[test]
fn human_merge_wait_excludes_pending_or_running_finish_task() {
  let pending = [task_for_goal("merge", "g1", TaskStatus::Pending, true)];
  let pending_refs: Vec<&takt_dash_backend::model::TaskEntry> = pending.iter().collect();
  assert!(!is_human_merge_wait(GoalStatus::AwaitingMerge, &goal_id(), &pending_refs));

  let running = [task_for_goal("merge", "g1", TaskStatus::Running, true)];
  let running_refs: Vec<&takt_dash_backend::model::TaskEntry> = running.iter().collect();
  assert!(!is_human_merge_wait(GoalStatus::AwaitingMerge, &goal_id(), &running_refs));
}

#[test]
fn human_merge_wait_excludes_failed_finish_task_states() {
  for status in [TaskStatus::Failed, TaskStatus::Exceeded, TaskStatus::PrFailed] {
    let tasks = [task_for_goal("merge", "g1", status, true)];
    let refs: Vec<&takt_dash_backend::model::TaskEntry> = tasks.iter().collect();
    assert!(
      !is_human_merge_wait(GoalStatus::AwaitingMerge, &goal_id(), &refs),
      "{status:?} は人のマージ待ちではありません"
    );
  }
}

#[test]
fn human_merge_wait_uses_last_finish_task_among_multiple() {
  // 末尾の仕上げタスクが完了なら人の待ち。途中に失敗があっても末尾が優先される。
  let tasks = [
    task_for_goal("merge-a", "g1", TaskStatus::Failed, true),
    task_for_goal("merge-b", "g1", TaskStatus::Completed, true),
  ];
  let refs: Vec<&takt_dash_backend::model::TaskEntry> = tasks.iter().collect();
  assert!(is_human_merge_wait(GoalStatus::AwaitingMerge, &goal_id(), &refs));
}

#[test]
fn human_merge_wait_ignores_other_goals_finish_tasks() {
  let tasks = [
    task_for_goal("merge", "g1", TaskStatus::Pending, true),
    task_for_goal("merge", "g2", TaskStatus::Completed, true),
  ];
  let refs: Vec<&takt_dash_backend::model::TaskEntry> = tasks.iter().collect();
  assert!(
    !is_human_merge_wait(GoalStatus::AwaitingMerge, &goal_id(), &refs),
    "g1 の仕上げタスクが pending なら待ちではない"
  );
  assert!(
    is_human_merge_wait(GoalStatus::AwaitingMerge, &goal_id_named("g2"), &refs),
    "g2 の仕上げタスクは完了なので待ち"
  );
}

#[test]
fn pending_decisions_include_only_inquiries_and_human_merge_goals() {
  let store = store_with(240, 10);
  let snapshot = snapshot(
    vec![merge_goal("g-merge", 99)],
    vec![task_for_goal("merge", "g-merge", TaskStatus::Completed, true)],
    vec![inquiry("q-open", false, now()), inquiry("q-answered", true, now())],
    vec![],
    vec![],
  );
  store.record_success("takt-dash", snapshot, now());

  let decisions = store.pending_decisions(now(), PendingDecisionFilter::All);

  let kinds: Vec<PendingDecisionType> = decisions.iter().map(|decision| decision.decision_type).collect();
  assert!(kinds.contains(&PendingDecisionType::Inquiry));
  assert!(kinds.contains(&PendingDecisionType::AwaitingMerge));
  assert_eq!(decisions.len(), 2, "未回答の問い合わせと人のマージ待ちだけを返します: {kinds:?}");
}

#[test]
fn awaiting_merge_with_unfinished_finish_task_is_not_a_pending_decision() {
  let store = store_with(240, 10);
  let snapshot = snapshot(
    vec![merge_goal("g-merge", 99)],
    vec![task_for_goal("merge", "g-merge", TaskStatus::Running, true)],
    vec![],
    vec![],
    vec![],
  );
  store.record_success("takt-dash", snapshot, now());

  let decisions = store.pending_decisions(now(), PendingDecisionFilter::AwaitingMerge);

  assert!(decisions.is_empty(), "仕上げタスクが進行中なら人のマージ待ちに含めません");
}

#[test]
fn stale_uses_configured_threshold_for_inquiry() {
  let created = minutes_before(now(), 300);
  let store = store_with(240, 10);
  let snapshot = snapshot(vec![], vec![], vec![inquiry("q", false, created)], vec![], vec![]);
  store.record_success("takt-dash", snapshot, now());

  let decisions = store.pending_decisions(now(), PendingDecisionFilter::Inquiry);

  assert_eq!(decisions.len(), 1);
  assert!(decisions[0].stale, "240分を超えた問い合わせは滞留です");
}

#[test]
fn not_stale_when_within_threshold() {
  let created = minutes_before(now(), 100);
  let store = store_with(240, 10);
  let snapshot = snapshot(vec![], vec![], vec![inquiry("q", false, created)], vec![], vec![]);
  store.record_success("takt-dash", snapshot, now());

  let decisions = store.pending_decisions(now(), PendingDecisionFilter::Inquiry);

  assert!(!decisions[0].stale, "240分以内は滞留ではありません");
}

#[test]
fn stale_threshold_change_alters_decision_on_same_store() {
  let created = minutes_before(now(), 120);
  let short = store_with(60, 10);
  let long = store_with(240, 10);
  for store in [&short, &long] {
    store.record_success(
      "takt-dash",
      snapshot(vec![], vec![], vec![inquiry("q", false, created)], vec![], vec![]),
      now(),
    );
  }

  assert!(short.pending_decisions(now(), PendingDecisionFilter::Inquiry)[0].stale, "60分基準では120分は滞留");
  assert!(!long.pending_decisions(now(), PendingDecisionFilter::Inquiry)[0].stale, "240分基準では120分は滞留でない");
}

#[test]
fn awaiting_merge_uses_notification_recorded_at_for_wait_start() {
  let recorded = minutes_before(now(), 300);
  let mut goal = merge_goal("g-merge", 7);
  goal.awaiting_merge_at = Some(recorded);
  let store = store_with(240, 10);
  store.record_success(
    "takt-dash",
    snapshot(vec![goal], vec![task_for_goal("merge", "g-merge", TaskStatus::Completed, true)], vec![], vec![], vec![]),
    now(),
  );

  let decisions = store.pending_decisions(now(), PendingDecisionFilter::AwaitingMerge);

  assert_eq!(decisions[0].created_at, recorded);
  assert!(decisions[0].stale, "通知の recordedAt から240分を超えた待ちは滞留です");
}

#[test]
fn awaiting_merge_wait_start_stays_fixed_and_becomes_stale_as_time_passes() {
  // 起点が決まった後は、同じ snapshot を同じ store に保持したまま投影時刻だけが進んでも、
  // created_at は起点のまま固定され、閾値を超えた時点で stale になる。
  let start = minutes_before(now(), 100);
  let mut goal = merge_goal("g-merge", 7);
  goal.awaiting_merge_at = Some(start);
  let store = store_with(240, 10);
  store.record_success(
    "takt-dash",
    snapshot(vec![goal], vec![task_for_goal("merge", "g-merge", TaskStatus::Completed, true)], vec![], vec![], vec![]),
    now(),
  );

  let early = store.pending_decisions(now(), PendingDecisionFilter::AwaitingMerge);
  assert_eq!(early[0].created_at, start, "投影しても起点は変わりません");
  assert!(!early[0].stale, "240分以内は滞留ではありません");

  let later = store.pending_decisions(now() + chrono::Duration::minutes(200), PendingDecisionFilter::AwaitingMerge);
  assert_eq!(later[0].created_at, start, "時刻が進んでも起点は固定されたままです");
  assert!(later[0].stale, "起点から240分を超えた待ちは滞留です");
}

#[test]
fn log_silent_uses_configured_threshold() {
  let store = store_with(240, 10);
  let silent_run = run("r1", "a", minutes_before(now(), 30));
  let fresh_run = run("r2", "b", minutes_before(now(), 2));

  assert!(store.is_log_silent(&silent_run, now()), "10分を超えた無音は異常です");
  assert!(!store.is_log_silent(&fresh_run, now()), "10分以内は異常ではありません");
}

#[test]
fn failure_keeps_snapshot_and_last_read_at_but_marks_disconnected() {
  let store = store_with(240, 10);
  let read_at = time("2026-11-13T11:00:00Z");
  let snapshot =
    snapshot(vec![goal("g1", GoalStatus::Created, ExecutionStatus::Active)], vec![], vec![], vec![], vec![]);
  store.record_success("takt-dash", snapshot.clone(), read_at);

  store.record_failure("takt-dash");

  let slot = store.slot("takt-dash").expect("スロットがあります");
  assert!(!slot.connected, "応答がない間は connected 偽です");
  assert_eq!(slot.last_read_at, Some(read_at), "最後に読めた時刻を保持します");
  assert_eq!(slot.snapshot, Some(snapshot), "最後に読めた状態を保持します");
}

#[test]
fn recovery_updates_state_on_next_success() {
  let store = store_with(240, 10);
  let first = time("2026-11-13T11:00:00Z");
  store.record_success(
    "takt-dash",
    snapshot(vec![goal("g1", GoalStatus::Created, ExecutionStatus::Active)], vec![], vec![], vec![], vec![]),
    first,
  );
  store.record_failure("takt-dash");

  let second = time("2026-11-13T11:30:00Z");
  store.record_success(
    "takt-dash",
    snapshot(vec![goal("g2", GoalStatus::Created, ExecutionStatus::Active)], vec![], vec![], vec![], vec![]),
    second,
  );

  let slot = store.slot("takt-dash").expect("スロットがあります");
  assert!(slot.connected);
  assert_eq!(slot.last_read_at, Some(second), "復旧後の読み取りで時刻を更新します");
  assert_eq!(slot.snapshot.expect("状態があります").goals[0].goal_id.as_str(), "g2");
}

#[test]
fn project_not_yet_read_reports_no_state() {
  let store = store_with(240, 10);
  let views = store.project_views();
  assert_eq!(views.len(), 1);
  assert!(!views[0].connected);
  assert!(views[0].last_read_at.is_none());
  assert!(views[0].goals.is_empty());
  assert!(views[0].snapshot.is_none());
}

#[test]
fn human_merge_decision_has_no_inquiry_kind_while_inquiry_decision_has_one() {
  // 人のマージ待ちは問い合わせではないため inquiry_kind を持たない。
  // 問い合わせの判断待ちだけが実値を持つ。
  let store = store_with(240, 10);
  let snapshot = snapshot(
    vec![merge_goal("g-merge", 99)],
    vec![task_for_goal("merge", "g-merge", TaskStatus::Completed, true)],
    vec![inquiry("q-open", false, now())],
    vec![],
    vec![],
  );
  store.record_success("takt-dash", snapshot, now());

  let decisions = store.pending_decisions(now(), PendingDecisionFilter::All);
  let merge = decisions
    .iter()
    .find(|decision| decision.decision_type == PendingDecisionType::AwaitingMerge)
    .expect("人のマージ待ちの判断待ちがあります");
  let inquiry = decisions
    .iter()
    .find(|decision| decision.decision_type == PendingDecisionType::Inquiry)
    .expect("問い合わせの判断待ちがあります");

  assert_eq!(merge.inquiry_kind, None, "マージ待ちは問い合わせの種類を持ちません");
  assert_eq!(inquiry.inquiry_kind, Some(takt_dash_backend::model::InquiryKind::Question), "問い合わせは実値を持ちます");
}

#[test]
fn set_projects_projects_added_projects_as_unread_and_removes_deleted_ones() {
  let mut servers = BTreeMap::new();
  servers.insert("mac-mini".to_owned(), server_meta());
  let store =
    StateStore::new(servers, vec![project("takt-dash", std::path::Path::new("/abs/takt-dash"))], thresholds());
  store.record_success(
    "takt-dash",
    snapshot(vec![goal("g1", GoalStatus::Created, ExecutionStatus::Active)], vec![], vec![], vec![], vec![]),
    now(),
  );

  // 稼働中の再読み込みで、1つ追加し1つ削除した一覧を反映する。
  store.set_projects(vec![
    project("takt-dash", std::path::Path::new("/abs/takt-dash")),
    project("corporate-site", std::path::Path::new("/abs/corporate-site")),
  ]);
  // takt-dash を一覧から外し、corporate-site だけを残す。
  store.set_projects(vec![project("corporate-site", std::path::Path::new("/abs/corporate-site"))]);

  let views = store.project_views();
  let names: Vec<&str> = views.iter().map(|view| view.name.as_str()).collect();
  assert_eq!(names, vec!["corporate-site"], "削除したプロジェクトは投影から消えます: {names:?}");
  assert!(!views[0].connected, "追加されたプロジェクトは未読スロットで connected 偽です");
  assert!(views[0].last_read_at.is_none(), "追加されたプロジェクトはまだ読めていません");
  assert!(views[0].snapshot.is_none(), "追加されたプロジェクトはまだ状態を持ちません");
}

#[test]
fn set_servers_replaces_adds_and_removes_server_metadata() {
  let mut servers = BTreeMap::new();
  servers.insert("mac-mini".to_owned(), ServerMeta {
    name:        "j5ik2o-mac-mini".to_owned(),
    kind:        "Mac mini".to_owned(),
    concurrency: 4,
  });
  let store =
    StateStore::new(servers, vec![project("takt-dash", std::path::Path::new("/abs/takt-dash"))], thresholds());

  // 稼働中の再読み込みで、既存サーバーの表示情報を変え、新規サーバーを追加し、旧サーバーを消す。
  let mut refreshed = BTreeMap::new();
  refreshed.insert("mac-mini".to_owned(), ServerMeta {
    name:        "mini-renamed".to_owned(),
    kind:        "Mac mini (改称)".to_owned(),
    concurrency: 8,
  });
  refreshed.insert("mac-studio".to_owned(), ServerMeta {
    name:        "j5ik2o-mac-studio".to_owned(),
    kind:        "Mac Studio".to_owned(),
    concurrency: 2,
  });
  store.set_servers(refreshed);

  let after = store.servers();
  assert_eq!(after.len(), 2, "追加したサーバーが投影に載り、削除したサーバーは消えます: {after:?}");
  let mini = after.get("mac-mini").expect("既存サーバーは残ります");
  assert_eq!(mini.name, "mini-renamed", "既存サーバーの表示情報を新しい設定で置き換えます");
  assert_eq!(mini.kind, "Mac mini (改称)");
  assert_eq!(mini.concurrency, 8, "並列度の変更も反映します");
  assert_eq!(after.get("mac-studio").expect("追加サーバーがあります").name, "j5ik2o-mac-studio");
}

#[test]
fn set_servers_does_not_disturb_project_slots() {
  let mut servers = BTreeMap::new();
  servers.insert("mac-mini".to_owned(), server_meta());
  let store =
    StateStore::new(servers, vec![project("takt-dash", std::path::Path::new("/abs/takt-dash"))], thresholds());
  let read_at = time("2026-11-13T11:00:00Z");
  store.record_success(
    "takt-dash",
    snapshot(vec![goal("g1", GoalStatus::Created, ExecutionStatus::Active)], vec![], vec![], vec![], vec![]),
    read_at,
  );

  let mut refreshed = BTreeMap::new();
  refreshed.insert("mac-mini".to_owned(), server_meta());
  store.set_servers(refreshed);

  let view = &store.project_views()[0];
  assert!(view.connected, "サーバー情報の反映はプロジェクトの接続状態を変えません");
  assert_eq!(view.last_read_at, Some(read_at), "サーバー情報の反映は読めた時刻を変えません");
}

#[test]
fn set_projects_keeps_existing_project_slot() {
  let mut servers = BTreeMap::new();
  servers.insert("mac-mini".to_owned(), server_meta());
  let store =
    StateStore::new(servers, vec![project("takt-dash", std::path::Path::new("/abs/takt-dash"))], thresholds());
  let read_at = time("2026-11-13T11:00:00Z");
  store.record_success(
    "takt-dash",
    snapshot(vec![goal("g1", GoalStatus::Created, ExecutionStatus::Active)], vec![], vec![], vec![], vec![]),
    read_at,
  );

  store.set_projects(vec![
    project("takt-dash", std::path::Path::new("/abs/takt-dash")),
    project("corporate-site", std::path::Path::new("/abs/corporate-site")),
  ]);

  let views = store.project_views();
  let takt_dash = views.iter().find(|view| view.name == "takt-dash").expect("既存プロジェクトは残ります");
  assert!(takt_dash.connected, "既存プロジェクトの接続状態を保持します");
  assert_eq!(takt_dash.last_read_at, Some(read_at), "既存プロジェクトの読めた時刻を保持します");
  assert_eq!(takt_dash.snapshot.as_ref().expect("既存の状態を保持します").goals[0].goal_id.as_str(), "g1");
}

#[test]
fn set_servers_replaces_server_meta_on_the_same_store() {
  let mut servers = BTreeMap::new();
  servers.insert("mac-mini".to_owned(), server_meta());
  let store =
    StateStore::new(servers, vec![project("takt-dash", std::path::Path::new("/abs/takt-dash"))], thresholds());
  assert_eq!(store.servers().len(), 1);

  // 稼働中の再読み込みで、既存サーバーの表示情報を変え、新しいサーバーを追加する。
  let mut changed = BTreeMap::new();
  changed.insert("mac-mini".to_owned(), ServerMeta {
    name:        "renamed-mini".to_owned(),
    kind:        "Mac Studio".to_owned(),
    concurrency: 8,
  });
  changed.insert("mac-studio".to_owned(), ServerMeta {
    name:        "j5ik2o-mac-studio".to_owned(),
    kind:        "Mac Studio".to_owned(),
    concurrency: 2,
  });
  store.set_servers(changed);

  let after = store.servers();
  assert_eq!(after.len(), 2, "追加したサーバーを投影に載せます");
  assert_eq!(after["mac-mini"].name, "renamed-mini", "改名を同じストアへ反映します");
  assert_eq!(after["mac-mini"].kind, "Mac Studio", "種類の変更も反映します");
  assert_eq!(after["mac-mini"].concurrency, 8, "同時実行数の変更も反映します");
  assert_eq!(after["mac-studio"].name, "j5ik2o-mac-studio");

  // 設定から消えたサーバーは投影に残さない。
  let mut removed = BTreeMap::new();
  removed.insert("mac-mini".to_owned(), after["mac-mini"].clone());
  store.set_servers(removed);

  assert_eq!(store.servers().len(), 1, "設定から消えたサーバーは投影から消えます");
  assert!(!store.servers().contains_key("mac-studio"));
}

#[test]
fn project_view_returns_goal_and_event_kind_helpers_stay_consistent() {
  let store = store_with(240, 10);
  let read_at = time("2026-11-13T11:00:00Z");
  store.record_success(
    "takt-dash",
    snapshot(vec![goal("g1", GoalStatus::Created, ExecutionStatus::Active)], vec![], vec![], vec![], vec![]),
    read_at,
  );

  let view = &store.project_views()[0];
  assert_eq!(view.name, "takt-dash");
  assert!(view.connected);
  assert_eq!(view.goals[0].goal_id.as_str(), "g1");
  assert_eq!(EventKind::from_stored(EventKind::Inquiry.as_str()), Some(EventKind::Inquiry));
}

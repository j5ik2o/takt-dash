//! mock MCP サーバを使い、定期読み取りの1サイクル・出来事の記録・応答なしのキャッシュ保持・
//! 復旧・稼働中の設定変更の反映を、同じ Poller 実体の上で観測する。

mod common;

use std::{
  collections::BTreeMap,
  path::{Path, PathBuf},
  sync::Arc,
};

use common::*;
use serde_json::{json, Value};
use takt_dash_backend::{
  activity::ActivityStore, config::Thresholds, model::EventKind, poller::Poller, store::StateStore,
};

struct Harness {
  _directory:  tempfile::TempDir,
  config_path: PathBuf,
  state_path:  PathBuf,
  activity:    Arc<ActivityStore>,
  store:       Arc<StateStore>,
}

fn write_json(path: &Path, value: &Value) {
  std::fs::write(path, value.to_string()).expect("JSON ファイルを書けます");
}

fn state_with_goal(goal_id: &str) -> Value {
  json!({ "goals": [ { "id": goal_id, "status": "created", "executionStatus": "active", "objective": format!("ゴール {goal_id}"), "integrationBranch": format!("takt/{goal_id}") } ] })
}

impl Harness {
  fn new(state: Value) -> Self {
    let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
    let config_path = directory.path().join("taktdash.config.json");
    let state_path = directory.path().join("state.json");
    let project_dir = directory.path().join("project");
    std::fs::create_dir_all(&project_dir).expect("プロジェクトの cwd を作れます");
    write_json(&state_path, &state);
    write_json(
      &config_path,
      &json!({
        "mcpServers": { "mac-mini": { "command": "node", "args": [mock_script().display().to_string(), "--state", state_path.display().to_string()], "env": {} } },
        "servers": { "mac-mini": { "name": "j5ik2o-mac-mini", "kind": "Mac mini", "concurrency": 4 } },
        "projects": [ { "name": "takt-dash", "server": "mac-mini", "about": "説明", "cwd": project_dir.display().to_string() } ],
        "intervalSeconds": 30,
        "thresholds": { "pendingDecisionStaleMinutes": 240, "runLogSilentMinutes": 10 },
        "sqlite": { "path": "data/taktdash.sqlite" }
      }),
    );
    let mut servers = BTreeMap::new();
    servers.insert("mac-mini".to_owned(), server_meta());
    let store = Arc::new(StateStore::new(servers, vec![project("takt-dash", &project_dir)], Thresholds {
      pending_decision_stale_minutes: 240,
      run_log_silent_minutes:         10,
    }));
    let activity = Arc::new(ActivityStore::open_in_memory().expect("メモリ上の SQLite を開けます"));
    Harness { _directory: directory, config_path, state_path, activity, store }
  }

  fn poller(&self) -> Poller {
    Poller::new(self.config_path.clone(), self.activity.clone(), self.store.clone())
  }

  fn rewrite_state(&self, state: Value) {
    write_json(&self.state_path, &state);
  }
}

#[tokio::test]
async fn first_cycle_reads_state_without_events_and_sets_read_time() {
  let harness = Harness::new(state_with_goal("g1"));
  let mut poller = harness.poller();

  let report = poller.read_cycle_once().await.expect("1サイクルを実行できます");

  assert_eq!(report.projects_read, 1);
  assert_eq!(report.events_recorded, 0, "初回は基準化のみで出来事を出しません");
  let slot = harness.store.slot("takt-dash").expect("スロットがあります");
  assert!(slot.connected);
  assert!(slot.last_read_at.is_some(), "読めた時刻を更新します");
  poller.shutdown().await;
}

#[tokio::test]
async fn state_change_between_cycles_emits_and_records_events() {
  let harness = Harness::new(state_with_goal("g1"));
  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("初回のサイクルを実行できます");

  harness.rewrite_state(state_with_goal("g2"));
  let report = poller.read_cycle_once().await.expect("2回目のサイクルを実行できます");

  assert_eq!(report.events_recorded, 1);
  let events = harness.activity.recent_events(10).expect("記録した出来事を読めます");
  assert_eq!(events[0].kind, EventKind::GoalCreated);
  assert_eq!(events[0].project.as_deref(), Some("takt-dash"));
  assert_eq!(events[0].server_id, "mac-mini");
  poller.shutdown().await;
}

#[tokio::test]
async fn unreachable_project_keeps_last_state_and_read_time_then_recovers() {
  let harness = Harness::new(state_with_goal("g1"));
  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("初回のサイクルを実行できます");
  let first_read_at = harness.store.slot("takt-dash").and_then(|slot| slot.last_read_at).expect("読めた時刻があります");

  // 状態ファイルを壊して、子プロセスを不正な応答に変える。
  std::fs::write(&harness.state_path, "{ this is not json").expect("壊れた状態ファイルを書けます");
  let failed = poller.read_cycle_once().await.expect("失敗してもサイクルは完了します");

  assert_eq!(failed.projects_failed, 1);
  let down = harness.store.slot("takt-dash").expect("スロットがあります");
  assert!(!down.connected, "応答がない間は connected 偽です");
  assert_eq!(down.last_read_at, Some(first_read_at), "最後に読めた時刻を保持します");
  assert_eq!(down.snapshot.expect("状態を保持します").goals[0].goal_id, "g1", "最後に読めた状態を保持します");
  let lost: Vec<EventKind> = harness.activity.recent_events(10).unwrap().into_iter().map(|event| event.kind).collect();
  assert_eq!(lost, vec![EventKind::ConnectionLost], "接続断を1回だけ記録します");

  harness.rewrite_state(state_with_goal("g1"));
  poller.read_cycle_once().await.expect("復旧後のサイクルを実行できます");
  let recovered = harness.store.slot("takt-dash").expect("スロットがあります");
  assert!(recovered.connected, "復旧すれば connected 真です");
  poller.shutdown().await;
}

#[tokio::test]
async fn threshold_change_is_applied_on_the_same_running_poller() {
  // 「今」から120分前の問い合わせを作る。初期の240分基準では滞留せず、1分基準では滞留する。
  let created_at = (chrono::Utc::now() - chrono::Duration::minutes(120)).to_rfc3339();
  let harness = Harness::new(json!({
    "goals": [],
    "standaloneInquiries": [ { "id": "q1", "status": "pending", "kind": "question", "body": "どうしますか", "createdAt": created_at } ]
  }));
  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("初回のサイクルを実行できます");

  let before =
    harness.store.pending_decisions(chrono::Utc::now(), takt_dash_backend::model::PendingDecisionFilter::Inquiry);
  assert_eq!(before.len(), 1);
  assert!(!before[0].stale, "240分基準では120分前の問い合わせは滞留しません");

  // 同じ Poller 実体と状態ストアのまま、設定の閾値を1分に変える。
  let current = std::fs::read_to_string(&harness.config_path).expect("設定を読めます");
  let mut value: Value = serde_json::from_str(&current).expect("設定を解析できます");
  value["thresholds"]["pendingDecisionStaleMinutes"] = json!(1);
  write_json(&harness.config_path, &value);

  poller.read_cycle_once().await.expect("2回目のサイクルを実行できます");

  let after =
    harness.store.pending_decisions(chrono::Utc::now(), takt_dash_backend::model::PendingDecisionFilter::Inquiry);
  poller.shutdown().await;
  assert_eq!(after.len(), 1);
  assert!(after[0].stale, "1分基準に変わったので同じ問い合わせが滞留になります");
}

#[tokio::test]
async fn interval_follows_config_file_changes_on_same_poller() {
  let harness = Harness::new(state_with_goal("g1"));
  let mut poller = harness.poller();
  let first = poller.read_cycle_once().await.expect("1サイクルを実行できます");
  assert_eq!(first.interval, std::time::Duration::from_secs(30));

  let current = std::fs::read_to_string(&harness.config_path).expect("設定を読めます");
  let mut value: Value = serde_json::from_str(&current).expect("設定を解析できます");
  value["intervalSeconds"] = json!(5);
  write_json(&harness.config_path, &value);

  let second = poller.read_cycle_once().await.expect("2サイクルを実行できます");
  poller.shutdown().await;
  assert_eq!(second.interval, std::time::Duration::from_secs(5), "稼働中でも設定変更に従います");
}

#[tokio::test]
async fn two_projects_read_independently_and_one_failure_does_not_stop_the_other() {
  let harness = Harness::new(state_with_goal("g1"));
  let directory = harness._directory.path();
  // 2つ目のプロジェクトは専用の state ファイルを持つ別サーバーに割り当てる。
  let project_two_dir = directory.join("project-two");
  std::fs::create_dir_all(&project_two_dir).expect("2つ目の cwd を作れます");
  let state_two = directory.join("state-two.json");
  write_json(&state_two, &state_with_goal("g2"));
  let mut config: Value = serde_json::from_str(&std::fs::read_to_string(&harness.config_path).unwrap()).unwrap();
  config["mcpServers"]["mini-two"] = json!({
    "command": "node",
    "args": [mock_script().display().to_string(), "--state", state_two.display().to_string()],
    "env": {}
  });
  config["servers"]["mini-two"] = json!({ "name": "j5ik2o-mac-mini-2", "kind": "Mac mini", "concurrency": 4 });
  config["projects"].as_array_mut().unwrap().push(json!({
    "name": "other", "server": "mini-two", "about": "説明2", "cwd": project_two_dir.display().to_string()
  }));
  write_json(&harness.config_path, &config);

  // 2つ目のプロジェクトの状態ファイルを壊し、その子プロセスだけを死亡させる。
  std::fs::write(&state_two, "{ this is not json").expect("壊れた状態ファイルを書けます");

  let mut poller = harness.poller();
  let report = poller.read_cycle_once().await.expect("1サイクルを実行できます");

  assert_eq!(report.projects_read, 1, "健常なプロジェクトだけを読めた件数に数えます");
  assert_eq!(report.projects_failed, 1, "失敗したプロジェクトを失敗件数に数えます");
  let healthy = harness.store.slot("takt-dash").expect("健常側のスロットがあります");
  assert!(healthy.connected, "健常なプロジェクトは connected 真のままです");
  let failed = harness.store.slot("other").expect("失敗側のスロットがあります");
  assert!(!failed.connected, "失敗したプロジェクトは connected 偽になります");
  poller.shutdown().await;
}

#[tokio::test]
async fn goal_pause_transition_emits_paused_event_through_the_cycle() {
  let harness = Harness::new(state_with_goal("g1"));
  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("初回のサイクルを実行できます");

  harness.rewrite_state(json!({
    "goals": [ { "id": "g1", "status": "created", "executionStatus": "paused", "objective": "ゴール g1", "integrationBranch": "takt/g1" } ]
  }));
  let report = poller.read_cycle_once().await.expect("2回目のサイクルを実行できます");

  let events: Vec<EventKind> =
    harness.activity.recent_events(10).unwrap().into_iter().map(|event| event.kind).collect();
  poller.shutdown().await;
  assert!(report.events_recorded >= 1);
  assert!(events.contains(&EventKind::Paused), "一時停止への遷移を出来事として記録します: {events:?}");
}

#[tokio::test]
async fn goal_read_failure_is_not_recorded_as_a_successful_partial_snapshot() {
  // ゴールの読み取りが isError で失敗したサイクルは全体の失敗として扱い、
  // ゴール質問が欠けた部分 snapshot を成功として記録しません。
  let harness = Harness::new(json!({ "goals": [], "isError": ["takt_list_goals"] }));
  let mut poller = harness.poller();

  let report = poller.read_cycle_once().await.expect("失敗してもサイクルは完了します");

  assert_eq!(report.projects_read, 0, "ゴールの読み取り失敗は成功件数に数えません");
  assert_eq!(report.projects_failed, 1, "ゴールの読み取り失敗は失敗件数に数えます");
  let slot = harness.store.slot("takt-dash").expect("スロットがあります");
  assert!(!slot.connected, "読み取りに失敗したプロジェクトは connected 偽です");
  assert!(slot.snapshot.is_none(), "部分 snapshot を成功として記録しません");
  poller.shutdown().await;
}

#[tokio::test]
async fn goal_question_tool_error_keeps_previous_inquiries_without_duplicate_events_on_recovery() {
  // g1 のゴール質問だけが isError を返すサイクルでも、g1 の未回答質問を前回値で保持する。
  // 復旧しても同一 inquiry_id の Inquiry 出来事を重複記録しない。
  let harness = Harness::new(json!({
    "goals": [ { "id": "g1", "status": "created", "executionStatus": "active", "objective": "ゴール g1", "integrationBranch": "takt/g1" } ],
    "goalQuestions": { "g1": [ { "id": "q1", "status": "pending", "kind": "question", "body": "どうしますか", "createdAt": "2026-11-13T09:00:00Z" } ] }
  }));
  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("初回のサイクルを実行できます");

  // ゴール質問の道具エラーを注入する。
  harness.rewrite_state(json!({
    "goals": [ { "id": "g1", "status": "created", "executionStatus": "active", "objective": "ゴール g1", "integrationBranch": "takt/g1" } ],
    "goalQuestions": { "g1": [ { "id": "q1", "status": "pending", "kind": "question", "body": "どうしますか", "createdAt": "2026-11-13T09:00:00Z" } ] },
    "isError": ["takt_list_goal_questions:g1"]
  }));
  let during = poller.read_cycle_once().await.expect("道具エラーでもサイクルは完了します");
  assert_eq!(during.projects_read, 1, "ゴール質問の道具エラーは読み取り成功です");
  let slot = harness.store.slot("takt-dash").expect("スロットがあります");
  assert!(slot.connected, "ゴール質問の道具エラーは connected を偽にしません");
  let snapshot = slot.snapshot.expect("状態があります");
  assert!(
    snapshot.inquiries.iter().any(|inquiry| inquiry.inquiry_id == "q1"),
    "道具エラー中も前回の未回答質問を保持します"
  );

  // 復旧させ、同じ q1 が再び読めるようになる。
  harness.rewrite_state(json!({
    "goals": [ { "id": "g1", "status": "created", "executionStatus": "active", "objective": "ゴール g1", "integrationBranch": "takt/g1" } ],
    "goalQuestions": { "g1": [ { "id": "q1", "status": "pending", "kind": "question", "body": "どうしますか", "createdAt": "2026-11-13T09:00:00Z" } ] }
  }));
  poller.read_cycle_once().await.expect("復旧後のサイクルを実行できます");
  poller.shutdown().await;

  let inquiries: Vec<EventKind> = harness
    .activity
    .recent_events(50)
    .unwrap()
    .into_iter()
    .filter(|event| event.kind == EventKind::Inquiry)
    .map(|event| event.kind)
    .collect();
  assert!(
    inquiries.is_empty(),
    "q1 は初回から既知なので、道具エラーを挟んでも復旧で Inquiry 出来事を重複記録しません: {inquiries:?}"
  );
}

#[tokio::test]
async fn notification_tool_error_keeps_awaiting_merge_start_and_avoids_duplicate_blocked_events() {
  let past = (chrono::Utc::now() - chrono::Duration::minutes(300)).to_rfc3339();
  let state = json!({
    "goals": [ { "id": "g1", "status": "awaiting_merge", "executionStatus": "active", "objective": "ゴール g1", "integrationBranch": "takt/g1", "pullRequest": { "number": 5, "url": "https://example.test/pr/5" } } ],
    "tasks": [ { "name": "merge", "goalId": "g1", "status": "completed", "mergeTask": true } ],
    "notifications": { "g1": [
      { "id": "n-merge", "kind": "awaiting_merge", "body": "マージ待ち", "recordedAt": past },
      { "id": "n-blocked", "kind": "blocked", "body": "依存解決に失敗しました", "recordedAt": past }
    ] }
  });
  let harness = Harness::new(state.clone());
  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("初回のサイクルを実行できます");
  let expected_start = harness
    .store
    .pending_decisions(chrono::Utc::now(), takt_dash_backend::model::PendingDecisionFilter::AwaitingMerge)[0]
    .created_at;

  // g1 の通知の道具エラーを注入する。
  let mut errored = state.clone();
  errored["isError"] = json!(["takt_get_goal:g1"]);
  harness.rewrite_state(errored);
  poller.read_cycle_once().await.expect("通知の道具エラーでもサイクルは完了します");

  let during =
    harness.store.pending_decisions(chrono::Utc::now(), takt_dash_backend::model::PendingDecisionFilter::AwaitingMerge);
  assert_eq!(during.len(), 1, "道具エラー中もマージ待ちの判断待ちを保持します");
  assert_eq!(during[0].created_at, expected_start, "道具エラー中も awaiting_merge の起点時刻を前回値で保持します");

  // 復旧させ、blocked 通知が再び読めるようになる。
  harness.rewrite_state(state);
  poller.read_cycle_once().await.expect("復旧後のサイクルを実行できます");
  poller.shutdown().await;

  let blocked: usize =
    harness.activity.recent_events(50).unwrap().into_iter().filter(|event| event.kind == EventKind::Blocked).count();
  assert_eq!(blocked, 0, "n-blocked は初回から既知なので、道具エラーを挟んでも復旧で Blocked 出来事を重複記録しません");
}

#[tokio::test]
async fn standalone_inquiry_tool_error_keeps_connected_and_previous_inquiry() {
  // takt_list_standalone_inquiries の isError は道具エラーであり接続断ではありません。
  // サイクルは成功し、前回の standalone 問い合わせを判断待ちに保持する。
  let harness = Harness::new(json!({
    "goals": [],
    "standaloneInquiries": [ { "id": "s1", "status": "pending", "kind": "operation_request", "body": "作業をお願いします", "createdAt": "2026-11-13T09:00:00Z" } ]
  }));
  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("初回のサイクルを実行できます");

  harness.rewrite_state(json!({
    "goals": [],
    "standaloneInquiries": [ { "id": "s1", "status": "pending", "kind": "operation_request", "body": "作業をお願いします", "createdAt": "2026-11-13T09:00:00Z" } ],
    "isError": ["takt_list_standalone_inquiries"]
  }));
  let report = poller.read_cycle_once().await.expect("standalone inquiries の道具エラーでサイクルは失敗しません");

  assert_eq!(report.projects_read, 1, "standalone inquiries の道具エラーは読み取り成功です");
  let slot = harness.store.slot("takt-dash").expect("スロットがあります");
  assert!(slot.connected, "道具エラーは connected を偽にしません");
  let events: Vec<EventKind> =
    harness.activity.recent_events(10).unwrap().into_iter().map(|event| event.kind).collect();
  assert!(!events.contains(&EventKind::ConnectionLost), "偽の ConnectionLost を記録しません: {events:?}");
  let decisions =
    harness.store.pending_decisions(chrono::Utc::now(), takt_dash_backend::model::PendingDecisionFilter::Inquiry);
  assert!(
    decisions.iter().any(|decision| decision.body == "作業をお願いします"),
    "前回の standalone 問い合わせを保持します"
  );

  // 復旧させ、同じ standalone 問い合わせが再び読めるようになる。s1 は初回から既知なので、
  // 道具エラーを挟んでも復旧で同一 inquiry_id の Inquiry 出来事を重複記録しない。
  harness.rewrite_state(json!({
    "goals": [],
    "standaloneInquiries": [ { "id": "s1", "status": "pending", "kind": "operation_request", "body": "作業をお願いします", "createdAt": "2026-11-13T09:00:00Z" } ]
  }));
  poller.read_cycle_once().await.expect("復旧後のサイクルを実行できます");
  poller.shutdown().await;

  let inquiries: usize =
    harness.activity.recent_events(50).unwrap().into_iter().filter(|event| event.kind == EventKind::Inquiry).count();
  assert_eq!(inquiries, 0, "s1 は初回から既知なので、復旧で Inquiry 出来事を重複記録しません");
}

#[tokio::test]
async fn get_run_tool_error_keeps_connected_and_records_no_connection_lost() {
  // takt_get_run の isError は道具エラーであり接続断ではありません。サイクルは成功し、
  // 偽の ConnectionLost を記録しません。
  let harness = Harness::new(json!({
    "goals": [],
    "tasks": [ { "name": "a", "status": "running", "runSlug": "r1", "startedAt": "2026-11-13T10:00:00Z" } ],
    "isError": ["takt_get_run"]
  }));
  let mut poller = harness.poller();

  let report = poller.read_cycle_once().await.expect("get_run の道具エラーでサイクルは失敗しません");

  assert_eq!(report.projects_read, 1, "get_run の道具エラーではプロジェクトは読み取り成功です");
  assert_eq!(report.projects_failed, 0);
  let slot = harness.store.slot("takt-dash").expect("スロットがあります");
  assert!(slot.connected, "補助読み取りの道具エラーは connected を偽にしません");
  let events: Vec<EventKind> =
    harness.activity.recent_events(10).unwrap().into_iter().map(|event| event.kind).collect();
  assert!(!events.contains(&EventKind::ConnectionLost), "偽の ConnectionLost を記録しません: {events:?}");
  poller.shutdown().await;
}

#[tokio::test]
async fn merge_wait_uses_notification_recorded_at_and_becomes_stale_through_the_cycle() {
  // awaiting_merge 通知の recordedAt を十分に過去にすると、読み取り後に人のマージ待ちが滞留になる。
  let past = (chrono::Utc::now() - chrono::Duration::minutes(300)).to_rfc3339();
  let harness = Harness::new(json!({
    "goals": [ { "id": "g1", "status": "awaiting_merge", "executionStatus": "active", "objective": "ゴール g1", "integrationBranch": "takt/g1", "pullRequest": { "number": 5, "url": "https://example.test/pr/5" } } ],
    "tasks": [ { "name": "merge", "goalId": "g1", "status": "completed", "mergeTask": true } ],
    "notifications": { "g1": [ { "id": "n-merge", "kind": "awaiting_merge", "body": "マージ待ち", "recordedAt": past } ] }
  }));
  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("1サイクルを実行できます");

  let decisions =
    harness.store.pending_decisions(chrono::Utc::now(), takt_dash_backend::model::PendingDecisionFilter::AwaitingMerge);
  poller.shutdown().await;

  assert_eq!(decisions.len(), 1, "人のマージ待ちを判断待ちに含めます");
  assert!(decisions[0].stale, "通知の recordedAt が240分より前なので滞留になります");
}

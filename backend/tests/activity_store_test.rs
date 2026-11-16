//! 出来事の SQLite 記録と、アクティビティのクエリ（一覧・JST
//! 日次集計・サーバー別・完了数）を検証する。

mod common;

use std::path::Path;

use chrono::{DateTime, Utc};
use takt_dash_backend::{
  activity::{ActivityStore, ATTENTION_KINDS},
  domain::RunSlug,
  model::{ActivityEvent, EventKind},
};

fn event(at: &str, server: &str, project: Option<&str>, kind: EventKind, body: &str) -> ActivityEvent {
  ActivityEvent {
    occurred_at: DateTime::parse_from_rfc3339(at).unwrap().with_timezone(&Utc),
    server_id: server.to_owned(),
    project: project.map(str::to_owned),
    kind,
    body: body.to_owned(),
    run_slug: None,
  }
}

#[test]
fn persisted_event_round_trips_all_columns() {
  let store = ActivityStore::open_in_memory().expect("メモリ上の SQLite を開けます");
  let recorded = ActivityEvent {
    run_slug: Some(RunSlug::try_from("20261113-run").expect("テストの run slug は有効です")),
    ..event(
      "2026-11-13T10:00:00Z",
      "mac-mini",
      Some("takt-dash"),
      EventKind::RunStarted,
      "タスクの run が始まりました。",
    )
  };

  store.record(&recorded).expect("出来事を記録できます");
  let events = store.recent_events(10).expect("出来事の一覧を取得できます");

  assert_eq!(events.len(), 1);
  assert_eq!(events[0], recorded);
}

#[test]
fn store_opens_a_file_and_creates_parent_schema() {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let path = directory.path().join("taktdash.sqlite");
  let store = ActivityStore::open(Path::new(&path)).expect("ファイルの SQLite を開けます");
  store
    .record(&event("2026-11-13T10:00:00Z", "mac-mini", Some("p"), EventKind::GoalCreated, "登録"))
    .expect("出来事を記録できます");

  let reopened = ActivityStore::open(&path).expect("同じファイルを再度開けます");
  assert_eq!(reopened.recent_events(10).expect("一覧を取得できます").len(), 1);
}

#[test]
fn recent_events_are_newest_first_and_limited() {
  let store = ActivityStore::open_in_memory().expect("SQLite を開けます");
  store.record(&event("2026-11-13T09:00:00Z", "mac-mini", Some("p"), EventKind::GoalCreated, "古い")).unwrap();
  store.record(&event("2026-11-13T10:00:00Z", "mac-mini", Some("p"), EventKind::RunStarted, "新しい")).unwrap();
  store.record(&event("2026-11-13T09:30:00Z", "mac-mini", Some("p"), EventKind::TaskCompleted, "中間")).unwrap();

  let events = store.recent_events(2).expect("件数を指定して一覧を取得できます");

  let bodies: Vec<&str> = events.iter().map(|event| event.body.as_str()).collect();
  assert_eq!(bodies, vec!["新しい", "中間"], "新しい順に件数だけ返します");
}

#[test]
fn daily_summary_returns_seven_zero_filled_jst_days() {
  let store = ActivityStore::open_in_memory().expect("SQLite を開けます");
  let now = DateTime::parse_from_rfc3339("2026-11-13T12:00:00Z").unwrap().with_timezone(&Utc);

  let summaries = store.daily_summaries(now).expect("日次集計を取得できます");

  assert_eq!(summaries.len(), 7, "直近7日分を返します");
  assert_eq!(summaries[6].date.as_str(), "2026-11-13");
  assert_eq!(summaries[0].date.as_str(), "2026-11-07");
  assert!(summaries.iter().all(|summary| summary.total == 0 && summary.attention == 0), "出来事のない日は0件です");
}

#[test]
fn daily_summary_uses_jst_day_boundary() {
  let store = ActivityStore::open_in_memory().expect("SQLite を開けます");
  // UTC ではどちらも 11-12 だが、JST では 11-13 01:00 と 11-12 23:30。
  store.record(&event("2026-11-12T16:00:00Z", "mac-mini", Some("p"), EventKind::GoalCreated, "jst-11-13")).unwrap();
  store.record(&event("2026-11-12T14:30:00Z", "mac-mini", Some("p"), EventKind::GoalCreated, "jst-11-12")).unwrap();
  let now = DateTime::parse_from_rfc3339("2026-11-13T12:00:00Z").unwrap().with_timezone(&Utc);

  let summaries = store.daily_summaries(now).expect("日次集計を取得できます");
  let by_date = |date: &str| summaries.iter().find(|summary| summary.date.as_str() == date).cloned().unwrap();

  assert_eq!(by_date("2026-11-13").total, 1, "16:00Z は JST の 11-13 に入ります");
  assert_eq!(by_date("2026-11-12").total, 1, "14:30Z は JST の 11-12 に入ります");
}

#[test]
fn daily_attention_counts_only_inquiry_merge_connection_lost() {
  let store = ActivityStore::open_in_memory().expect("SQLite を開けます");
  let day = "2026-11-13T04:00:00Z";
  for kind in [
    EventKind::Inquiry,
    EventKind::AwaitingMerge,
    EventKind::ConnectionLost,
    EventKind::GoalCreated,
    EventKind::RunStarted,
    EventKind::TaskCompleted,
    EventKind::Paused,
    EventKind::Blocked,
  ] {
    store.record(&event(day, "mac-mini", Some("p"), kind, kind.as_str())).unwrap();
  }
  let now = DateTime::parse_from_rfc3339("2026-11-13T12:00:00Z").unwrap().with_timezone(&Utc);

  let summaries = store.daily_summaries(now).expect("日次集計を取得できます");
  let today = summaries.iter().find(|summary| summary.date.as_str() == "2026-11-13").unwrap();

  assert_eq!(today.total, 8, "総数は8種類すべてを数えます");
  assert_eq!(today.attention, 3, "判断待ちと異常は問い合わせ・マージ待ち・応答なしの3種類だけです");
  assert_eq!(ATTENTION_KINDS.len(), 3);
}

#[test]
fn today_counts_by_server_groups_and_orders_by_count() {
  let store = ActivityStore::open_in_memory().expect("SQLite を開けます");
  store.record(&event("2026-11-13T04:00:00Z", "mini", Some("p"), EventKind::GoalCreated, "a")).unwrap();
  store.record(&event("2026-11-13T04:01:00Z", "mini", Some("p"), EventKind::RunStarted, "b")).unwrap();
  store.record(&event("2026-11-13T04:02:00Z", "studio", Some("q"), EventKind::GoalCreated, "c")).unwrap();
  let now = DateTime::parse_from_rfc3339("2026-11-13T12:00:00Z").unwrap().with_timezone(&Utc);

  let counts = store.today_counts_by_server(now).expect("サーバー別の件数を取得できます");

  assert_eq!(counts[0].server_id.as_str(), "mini");
  assert_eq!(counts[0].count, 2);
  assert_eq!(counts[1].server_id.as_str(), "studio");
  assert_eq!(counts[1].count, 1);
}

#[test]
fn completed_tasks_since_counts_only_completed_in_window() {
  let store = ActivityStore::open_in_memory().expect("SQLite を開けます");
  store.record(&event("2026-11-13T10:00:00Z", "mac-mini", Some("p"), EventKind::TaskCompleted, "最近")).unwrap();
  store.record(&event("2026-11-13T09:00:00Z", "mac-mini", Some("p"), EventKind::RunStarted, "開始")).unwrap();
  store.record(&event("2026-11-10T10:00:00Z", "mac-mini", Some("p"), EventKind::TaskCompleted, "古い")).unwrap();
  let since = DateTime::parse_from_rfc3339("2026-11-12T22:00:00Z").unwrap().with_timezone(&Utc);

  let count = store.completed_tasks_since("p", since).expect("完了数を取得できます");

  assert_eq!(count.get(), 1, "窓の中のタスク完了だけを数えます");
}

#[test]
fn record_all_saves_every_event_of_a_group() {
  // 1サイクルで検出した出来事群は、まとめて1回の呼び出しで保存します。
  let store = ActivityStore::open_in_memory().expect("SQLite を開けます");
  let group = vec![
    event("2026-11-13T09:00:00Z", "mac-mini", Some("p"), EventKind::GoalCreated, "1件目"),
    event("2026-11-13T09:00:00Z", "mac-mini", Some("p"), EventKind::RunStarted, "2件目"),
    event("2026-11-13T09:00:00Z", "mac-mini", Some("p"), EventKind::TaskCompleted, "3件目"),
  ];

  store.record_all(&group).expect("出来事群をまとめて記録できます");

  let mut bodies: Vec<String> =
    store.recent_events(10).expect("一覧を取得できます").into_iter().map(|event| event.body).collect();
  bodies.sort();
  assert_eq!(bodies, vec!["1件目", "2件目", "3件目"], "出来事群の全件を保存します");
}

#[test]
fn record_all_saves_nothing_when_a_member_cannot_be_saved() {
  // 出来事群の途中に保存できない要素がある場合、それまでの要素も残さず、群全体を保存しません。
  // 1件ずつ INSERT して途中まで残す実装では、先頭の出来事が残ってこの観測が失敗します。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let path = directory.path().join("events.sqlite");
  let store = ActivityStore::open(&path).expect("ファイルの SQLite を開けます");
  // 直接接続して、特定の本文を保存しようとすると中断するトリガーを仕込みます。
  {
    let connection = rusqlite::Connection::open(&path).expect("同じファイルへ直接接続できます");
    connection
      .execute_batch(
        "CREATE TRIGGER reject_poison BEFORE INSERT ON events WHEN NEW.body = '保存できない出来事'
         BEGIN SELECT RAISE(ABORT, '保存できない出来事'); END;",
      )
      .expect("中断させるトリガーを作れます");
  }
  let group = vec![
    event("2026-11-13T09:00:00Z", "mac-mini", Some("p"), EventKind::GoalCreated, "先頭"),
    event("2026-11-13T09:00:00Z", "mac-mini", Some("p"), EventKind::GoalCreated, "保存できない出来事"),
    event("2026-11-13T09:00:00Z", "mac-mini", Some("p"), EventKind::RunStarted, "末尾"),
  ];

  store.record_all(&group).expect_err("保存できない要素を含む群は失敗します");

  assert_eq!(store.recent_events(10).expect("一覧を取得できます").len(), 0, "群の途中までを残しません");
}

#[test]
fn recent_events_reports_a_stored_run_slug_that_cannot_be_a_domain_value() {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let path = directory.path().join("events.sqlite");
  let store = ActivityStore::open(&path).expect("ファイルの SQLite を開けます");
  {
    let connection = rusqlite::Connection::open(&path).expect("同じファイルへ直接接続できます");
    connection
      .execute(
        "INSERT INTO events (occurred_at, server_id, project, kind, body, run_slug)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params!["2026-11-13T04:00:00.000Z", "mac-mini", "p", "run_started", "run", "runs/1"],
      )
      .expect("保存済みの不正値を再現できます");
  }

  let error = store.recent_events(10).expect_err("run ディレクトリ名にならない保存値は復元できません");

  assert!(
    matches!(error, rusqlite::Error::FromSqlConversionFailure(..)),
    "不正な run slug は SQL の復元エラーになります: {error}"
  );
}

#[test]
fn today_counts_by_server_reports_a_stored_server_id_that_cannot_be_a_domain_value() {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let path = directory.path().join("events.sqlite");
  let store = ActivityStore::open(&path).expect("ファイルの SQLite を開けます");
  {
    let connection = rusqlite::Connection::open(&path).expect("同じファイルへ直接接続できます");
    connection
      .execute(
        "INSERT INTO events (occurred_at, server_id, project, kind, body, run_slug)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params!["2026-11-13T04:00:00.000Z", "mini\n", "p", "goal_created", "登録", Option::<String>::None],
      )
      .expect("保存済みの不正値を再現できます");
  }
  let now = DateTime::parse_from_rfc3339("2026-11-13T12:00:00Z").unwrap().with_timezone(&Utc);

  let error = store.today_counts_by_server(now).expect_err("制御文字を含む保存値は復元できません");

  assert!(
    matches!(error, rusqlite::Error::FromSqlConversionFailure(..)),
    "不正なサーバー ID は SQL の復元エラーになります: {error}"
  );
}

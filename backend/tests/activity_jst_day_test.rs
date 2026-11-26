//! JST の日で数える読み取り（`daily_summaries`、`today_counts_by_server`）の結果を、
//! `date(occurred_at, '+9 hours')` で日を決めていた main の SQL（54b10a6）を原典として比べる
//! （設計: docs/design/issue-318-a12.md の「テスト」）。
//!
//! 原典の SQL は、このファイルに写して固定する。日付の条件を時刻の範囲に変えても、
//! 同じ出来事の集まりと同じ `now` で、返る件数と並びが原典と同じであることを確かめる。

use std::path::Path;

use chrono::{DateTime, Duration, FixedOffset, NaiveDate, Utc};
use proptest::{collection::vec, prelude::*, sample::select};
use rusqlite::{params, Connection};
use takt_dash_backend::{
  activity::{ActivityStore, ATTENTION_KINDS},
  model::{ActivityEvent, EventKind},
};

const HOUR_MS: i64 = 3_600_000;
const DAY_MS: i64 = 24 * HOUR_MS;
/// JST と UTC の差。JST の 09:00 が UTC の日の境目にあたる。
const JST_OFFSET_MS: i64 = 9 * HOUR_MS;

/// 生成する時刻の下限（0001-01-10T00:00:00Z）。SQLite の日付関数は 0000〜9999 年を扱う。
/// 7 日前の JST の日の始まりも 0001 年に収まる位置にする。
const MIN_MS: i64 = -62_134_819_200_000;
/// 生成する時刻の上限（9999-12-30T00:00:00Z）。翌日の JST の日の終わりも 9999 年に収まる。
const MAX_MS: i64 = 253_402_128_000_000;

/// 原典: main の `count_on`（種類を絞らない）。
const ORACLE_TOTAL: &str = "SELECT COUNT(*) FROM events WHERE date(occurred_at, '+9 hours') = ?1";
/// 原典: main の `count_on`（判断待ちの 2 種類に絞る）。
const ORACLE_ATTENTION: &str =
  "SELECT COUNT(*) FROM events WHERE date(occurred_at, '+9 hours') = ?1 AND kind IN (?2, ?3)";
/// 原典: main の `today_counts_by_server`。
const ORACLE_BY_SERVER: &str =
  "SELECT server_id, COUNT(*) FROM events WHERE date(occurred_at, '+9 hours') = ?1 AND server_id != ''
   GROUP BY server_id ORDER BY COUNT(*) DESC, server_id";

const ALL_KINDS: [EventKind; 15] = [
  EventKind::GoalCreated,
  EventKind::RunStarted,
  EventKind::TaskCompleted,
  EventKind::Inquiry,
  EventKind::AwaitingMerge,
  EventKind::Paused,
  EventKind::Blocked,
  EventKind::ConnectionLost,
  EventKind::ConnectionRestored,
  EventKind::TaskFailed,
  EventKind::TaskExceeded,
  EventKind::TaskPrFailed,
  EventKind::RunFailed,
  EventKind::RunAborted,
  EventKind::BackendStarted,
];

/// 空のサーバーは、どのサーバーにも属さない出来事（backend の起動など）。
const SERVERS: [&str; 4] = ["", "mac-mini", "studio", "a"];

fn at_ms(ms: i64) -> DateTime<Utc> {
  DateTime::from_timestamp_millis(ms).expect("生成した時刻は chrono の範囲に収まります")
}

fn at(text: &str) -> DateTime<Utc> {
  DateTime::parse_from_rfc3339(text).expect("時刻を解析できます").with_timezone(&Utc)
}

fn event(occurred_at: DateTime<Utc>, server: &str, kind: EventKind) -> ActivityEvent {
  ActivityEvent {
    occurred_at,
    server_id: server.to_owned(),
    project: Some("takt-dash".to_owned()),
    kind,
    body: "出来事".to_owned(),
    run_slug: None,
  }
}

/// ファイルの SQLite に出来事を記録し、同じファイルを原典の SQL で読む接続を添えて返す。
fn recorded(events: &[ActivityEvent]) -> (tempfile::TempDir, ActivityStore, Connection) {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let path = directory.path().join("activity.sqlite");
  let store = ActivityStore::open(&path).expect("ファイルの SQLite を開けます");
  if !events.is_empty() {
    store.record_all(events).expect("出来事を記録できます");
  }
  let oracle = oracle_connection(&path);
  (directory, store, oracle)
}

fn oracle_connection(path: &Path) -> Connection {
  Connection::open(path).expect("原典の SQL 用に同じファイルを開けます")
}

/// `now` を含む JST の日。
fn jst_today(now: DateTime<Utc>) -> NaiveDate {
  let jst = FixedOffset::east_opt(9 * 3600).expect("JST のオフセットは固定です");
  now.with_timezone(&jst).date_naive()
}

/// 原典の SQL で数えた 7 日分の（日付、総数、判断待ち）。古い日から並べる。
fn oracle_daily(oracle: &Connection, now: DateTime<Utc>) -> Vec<(String, i64, i64)> {
  let today = jst_today(now);
  (0..7)
    .rev()
    .map(|back| {
      let date = (today - Duration::days(back)).format("%Y-%m-%d").to_string();
      let total: i64 = oracle.query_row(ORACLE_TOTAL, params![date], |row| row.get(0)).expect("原典の総数");
      let attention: i64 = oracle
        .query_row(ORACLE_ATTENTION, params![date, ATTENTION_KINDS[0].as_str(), ATTENTION_KINDS[1].as_str()], |row| {
          row.get(0)
        })
        .expect("原典の判断待ち");
      (date, total, attention)
    })
    .collect()
}

/// 原典の SQL で数えた、`now` を含む JST の日のサーバーごとの件数。返った順のまま。
fn oracle_by_server(oracle: &Connection, now: DateTime<Utc>) -> Vec<(String, i64)> {
  let date = jst_today(now).format("%Y-%m-%d").to_string();
  let mut statement = oracle.prepare(ORACLE_BY_SERVER).expect("原典の SQL を用意できます");
  statement
    .query_map(params![date], |row| Ok((row.get(0)?, row.get(1)?)))
    .expect("原典の SQL を流せます")
    .collect::<Result<_, _>>()
    .expect("原典の行を読めます")
}

fn store_daily(store: &ActivityStore, now: DateTime<Utc>) -> Vec<(String, i64, i64)> {
  store
    .daily_summaries(now)
    .expect("日次集計を取得できます")
    .into_iter()
    .map(|summary| (summary.date.as_str().to_owned(), i64::from(summary.total), i64::from(summary.attention)))
    .collect()
}

fn store_by_server(store: &ActivityStore, now: DateTime<Utc>) -> Vec<(String, i64)> {
  store
    .today_counts_by_server(now)
    .expect("サーバー別の件数を取得できます")
    .into_iter()
    .map(|count| (count.server_id.as_str().to_owned(), i64::from(count.count)))
    .collect()
}

/// 原典と、日次集計・サーバー別の件数の両方が同じであることを確かめる。
fn assert_matches_oracle(events: &[ActivityEvent], now: DateTime<Utc>) {
  let (_directory, store, oracle) = recorded(events);
  assert_eq!(store_daily(&store, now), oracle_daily(&oracle, now), "日次集計が原典と同じ（now: {now}）");
  assert_eq!(
    store_by_server(&store, now),
    oracle_by_server(&oracle, now),
    "サーバー別の件数と並びが原典と同じ（now: {now}）"
  );
}

/// JST の日の中の時刻（ミリ秒）。日の境目の前後と、UTC の日の境目（JST の 09:00）の前後に寄せる。
fn time_of_jst_day() -> impl Strategy<Value = i64> {
  prop_oneof![
    Just(0),
    Just(1),
    Just(DAY_MS - 1),
    Just(DAY_MS - 2),
    Just(JST_OFFSET_MS - 1),
    Just(JST_OFFSET_MS),
    Just(JST_OFFSET_MS + 1),
    0..DAY_MS,
  ]
}

/// `now` と、出来事の集まり。出来事の半分ほどは `now` の JST の日の前後（8
/// 日前〜翌日）の境目に寄せ、 残りは範囲の中の任意の時刻にする。
fn now_and_events() -> impl Strategy<Value = (DateTime<Utc>, Vec<ActivityEvent>)> {
  let now_ms = prop_oneof![1_700_000_000_000_i64..2_000_000_000_000, MIN_MS..=MAX_MS];
  now_ms.prop_flat_map(|now_ms| {
    let now = at_ms(now_ms);
    let today_start_ms =
      jst_today(now).and_hms_opt(0, 0, 0).expect("日の始まり").and_utc().timestamp_millis() - JST_OFFSET_MS;
    let near = (-8_i64..=1, time_of_jst_day()).prop_map(move |(day, time)| today_start_ms + day * DAY_MS + time);
    let anywhere = MIN_MS..=MAX_MS;
    let occurred_at = prop_oneof![near, anywhere].prop_map(at_ms);
    let one = (occurred_at, select(SERVERS.as_slice()), select(ALL_KINDS.as_slice()))
      .prop_map(|(occurred_at, server, kind)| event(occurred_at, server, kind));
    (Just(now), vec(one, 0..40))
  })
}

proptest! {
  #![proptest_config(ProptestConfig::with_cases(256))]

  /// 任意の出来事の集まりと任意の `now` で、日次集計とサーバー別の件数（並びも）が原典と同じ。
  #[test]
  fn jst_day_reads_match_the_date_function_oracle((now, events) in now_and_events()) {
    assert_matches_oracle(&events, now);
  }
}

#[test]
fn empty_table_matches_the_oracle() {
  assert_matches_oracle(&[], at("2026-11-13T12:00:00Z"));
}

/// JST の 11-13 23:59:59.999 は 11-13 に、11-14 00:00:00.000 は 11-14 に入る。
#[test]
fn jst_day_boundary_splits_at_jst_midnight() {
  let events = [
    event(at("2026-11-13T14:59:59.999Z"), "mac-mini", EventKind::Inquiry),
    event(at("2026-11-13T15:00:00.000Z"), "studio", EventKind::AwaitingMerge),
    event(at("2026-11-13T15:00:00.001Z"), "studio", EventKind::GoalCreated),
  ];
  let (_directory, store, _oracle) = recorded(&events);

  let daily = store_daily(&store, at("2026-11-14T03:00:00Z"));
  assert_eq!(daily[5], ("2026-11-13".to_owned(), 1, 1), "23:59:59.999 JST は 11-13 に入ります");
  assert_eq!(daily[6], ("2026-11-14".to_owned(), 2, 1), "00:00:00.000 JST からは 11-14 に入ります");

  assert_eq!(
    store_by_server(&store, at("2026-11-13T14:59:59.999Z")),
    vec![("mac-mini".to_owned(), 1)],
    "11-13 の最後の瞬間の now では、11-13 の出来事だけを数えます"
  );
  assert_eq!(
    store_by_server(&store, at("2026-11-13T15:00:00.000Z")),
    vec![("studio".to_owned(), 2)],
    "11-14 の最初の瞬間の now では、11-14 の出来事だけを数えます"
  );
  assert_matches_oracle(&events, at("2026-11-14T03:00:00Z"));
  assert_matches_oracle(&events, at("2026-11-13T14:59:59.999Z"));
}

/// UTC の日の境目（JST の 09:00）は、JST の日を分けない。
#[test]
fn utc_day_boundary_does_not_split_the_jst_day() {
  let events = [
    event(at("2026-11-13T23:59:59.999Z"), "mac-mini", EventKind::GoalCreated),
    event(at("2026-11-14T00:00:00.000Z"), "mac-mini", EventKind::Inquiry),
    event(at("2026-11-14T00:00:00.001Z"), "studio", EventKind::RunStarted),
  ];
  let (_directory, store, _oracle) = recorded(&events);

  let daily = store_daily(&store, at("2026-11-14T12:00:00Z"));
  assert_eq!(daily[6], ("2026-11-14".to_owned(), 3, 1), "UTC の日をまたいでも、3 件とも JST の 11-14 に入ります");
  assert_eq!(daily[5], ("2026-11-13".to_owned(), 0, 0));

  assert_eq!(
    store_by_server(&store, at("2026-11-13T23:59:59.999Z")),
    vec![("mac-mini".to_owned(), 2), ("studio".to_owned(), 1)],
    "UTC の 11-13 の最後の瞬間の now でも、JST の 11-14 の 3 件を数えます"
  );
  assert_matches_oracle(&events, at("2026-11-14T12:00:00Z"));
  assert_matches_oracle(&events, at("2026-11-13T23:59:59.999Z"));
}

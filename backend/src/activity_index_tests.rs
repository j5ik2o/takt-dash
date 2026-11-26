//! JST の日で数える読み取りが、events の occurred_at の索引で行を探すことを確かめる
//! （設計: docs/design/issue-318-a12.md の「索引が効くことの確かめ方」）。
//!
//! 読み取りの公開の関数を呼び、その間に接続で流れた SQL を、束縛した値を埋めた形で trace で集める。
//! 集めた SQL を `EXPLAIN QUERY PLAN` に渡し、events を `SEARCH ... USING ... INDEX
//! events_occurred_at (occurred_at>? AND occurred_at<?)` で読み、`SCAN events` が無いことを見る。
//! 開始と終了の両方の条件が索引で絞られていることまで見るので、片側だけを索引で絞り、
//! もう片側を索引の外で絞る形も落ちる。SQL の文面には依らないので、実装の形を問わない。

use std::sync::{Arc, Mutex};

use chrono::DateTime;
use rusqlite::{
  trace::{TraceEvent, TraceEventCodes},
  StatementStatus,
};

use super::*;

/// 流れた 1 本の SQL（束縛した値を埋めた形）と、それが表を頭から順に読んだ歩数。
struct TracedStatement {
  sql:           String,
  fullscan_step: i32,
}

fn at(text: &str) -> DateTime<Utc> {
  DateTime::parse_from_rfc3339(text).expect("時刻を解析できます").with_timezone(&Utc)
}

fn event(at_text: &str, server: &str, kind: EventKind) -> ActivityEvent {
  ActivityEvent {
    occurred_at: at(at_text),
    server_id: server.to_owned(),
    project: Some("takt-dash".to_owned()),
    kind,
    body: "出来事".to_owned(),
    run_slug: None,
  }
}

/// 範囲の外の日にも行がある store。索引を使わずに頭から読むと、`fullscan_step` が行の数だけ進む。
fn store_with_events() -> ActivityStore {
  let store = ActivityStore::open_in_memory().expect("メモリ上の SQLite を開けます");
  let events: Vec<_> = (0..60)
    .map(|minute| {
      let day = 1 + minute % 28;
      event(
        &format!("2026-10-{day:02}T{:02}:{:02}:00.000Z", minute % 24, minute),
        ["mac-mini", "studio", ""][minute as usize % 3],
        [EventKind::GoalCreated, EventKind::Inquiry, EventKind::AwaitingMerge][minute as usize % 3],
      )
    })
    .chain([
      event("2026-11-13T14:59:59.999Z", "mac-mini", EventKind::Inquiry),
      event("2026-11-13T15:00:00.000Z", "studio", EventKind::AwaitingMerge),
      event("2026-11-14T00:00:00.000Z", "mac-mini", EventKind::GoalCreated),
    ])
    .collect();
  store.record_all(&events).expect("出来事を記録できます");
  store
}

/// `read` の間に、store の接続で終わりまで流れた SQL を集める。
///
/// メモリ上の store は読み取りも writer の接続を使うので、writer の接続に trace を付ければ足りる。
fn traced_reads(store: &ActivityStore, read: impl FnOnce(&ActivityStore)) -> Vec<TracedStatement> {
  let captured = Arc::new(Mutex::new(Vec::new()));
  let sink = captured.clone();
  store
    .writer()
    .trace_v2(
      TraceEventCodes::SQLITE_TRACE_PROFILE,
      Some(move |event: TraceEvent<'_>| {
        if let TraceEvent::Profile(statement, _) = event {
          sink.lock().expect("集めた SQL のロック").push(TracedStatement {
            sql:           statement.expanded_sql().unwrap_or_else(|| statement.sql().into_owned()),
            fullscan_step: statement.get_status(StatementStatus::FullscanStep),
          });
        }
      }),
    )
    .expect("trace を付けられます");
  read(store);
  store.writer().trace_v2(TraceEventCodes::SQLITE_TRACE_PROFILE, None::<fn(TraceEvent<'_>)>).expect("trace を外せます");
  let statements = std::mem::take(&mut *captured.lock().expect("集めた SQL のロック"));
  statements.into_iter().filter(|statement| statement.sql.contains("events")).collect()
}

/// `EXPLAIN QUERY PLAN` の detail の列。
fn query_plan(store: &ActivityStore, sql: &str) -> Vec<String> {
  let connection = store.writer();
  let mut statement = connection.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).expect("実行計画を問い合わせられます");
  statement
    .query_map([], |row| row.get::<_, String>(3))
    .expect("実行計画を読めます")
    .collect::<Result<_, _>>()
    .expect("実行計画の行を読めます")
}

/// 索引 events_occurred_at を、開始と終了の両方の条件で使って events を探す計画か。
fn searches_occurred_at_range(plan: &[String]) -> bool {
  plan.iter().any(|detail| {
    detail.starts_with("SEARCH events USING")
      && detail.contains("INDEX events_occurred_at (occurred_at>? AND occurred_at<?)")
  })
}

fn assert_every_read_searches_the_occurred_at_index(name: &str, read: impl FnOnce(&ActivityStore)) {
  let store = store_with_events();
  let statements = traced_reads(&store, read);
  assert!(!statements.is_empty(), "{name}: events を読む SQL が流れます");
  for statement in &statements {
    let plan = query_plan(&store, &statement.sql);
    assert!(
      searches_occurred_at_range(&plan),
      "{name}: events を occurred_at の索引で、開始と終了の両方の条件で探します\nSQL: {}\n計画: {plan:#?}",
      statement.sql
    );
    assert!(
      !plan.iter().any(|detail| detail.starts_with("SCAN events")),
      "{name}: events を頭から読みません\nSQL: {}\n計画: {plan:#?}",
      statement.sql
    );
    assert_eq!(
      statement.fullscan_step, 0,
      "{name}: 流した SQL は表を頭から順に読みません\nSQL: {}\n計画: {plan:#?}",
      statement.sql
    );
  }
}

#[test]
fn daily_summaries_search_the_occurred_at_index() {
  assert_every_read_searches_the_occurred_at_index("daily_summaries", |store| {
    store.daily_summaries(at("2026-11-14T03:00:00Z")).expect("日次集計を取得できます");
  });
}

#[test]
fn today_counts_by_server_search_the_occurred_at_index() {
  assert_every_read_searches_the_occurred_at_index("today_counts_by_server", |store| {
    store.today_counts_by_server(at("2026-11-14T03:00:00Z")).expect("サーバー別の件数を取得できます");
  });
}

/// 変異: 片側の条件だけを索引で絞り、もう片側を索引の外で絞る SQL は、計画の検査で落ちる。
///
/// 単項の `+` を付けた列は索引の条件に使われないので、その条件は索引で探した行を読んでから絞る。
/// 件数は変わらず、表を頭から読む歩数も 0 のままなので、計画の文で見分けるしかない。
#[test]
fn one_sided_index_range_fails_the_plan_check() {
  let store = store_with_events();
  let both = "SELECT COUNT(*) FROM events WHERE occurred_at >= '2026-11-13T15:00:00.000Z' AND occurred_at < '2026-11-14T15:00:00.000Z'";
  assert!(searches_occurred_at_range(&query_plan(&store, both)), "両側を索引で絞る SQL は通ります: {both}");

  for sql in [
    "SELECT COUNT(*) FROM events WHERE occurred_at >= '2026-11-13T15:00:00.000Z' AND +occurred_at < '2026-11-14T15:00:00.000Z'",
    "SELECT COUNT(*) FROM events WHERE +occurred_at >= '2026-11-13T15:00:00.000Z' AND occurred_at < '2026-11-14T15:00:00.000Z'",
    "SELECT server_id, COUNT(*) FROM events WHERE occurred_at >= '2026-11-13T15:00:00.000Z' AND +occurred_at < '2026-11-14T15:00:00.000Z' AND server_id != '' GROUP BY server_id",
  ] {
    let plan = query_plan(&store, sql);
    assert!(
      plan.iter().any(|detail| detail.contains("INDEX events_occurred_at")),
      "変異の SQL も索引は使います（片側だけ）\nSQL: {sql}\n計画: {plan:#?}"
    );
    assert!(!searches_occurred_at_range(&plan), "片側だけを索引で絞る SQL は落ちます\nSQL: {sql}\n計画: {plan:#?}");
  }
}

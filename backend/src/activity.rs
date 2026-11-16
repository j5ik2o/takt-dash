//! 出来事の SQLite への記録と、アクティビティのクエリ。
//!
//! 日付の区切りは日本標準時（JST）の日で行います。判断待ちと異常に数える種類は
//! 問い合わせ・マージ待ちの2種類だけです。接続の切断・復帰は総数には含めますが数えません。

use std::{path::Path, sync::Mutex};

use chrono::{DateTime, Duration, FixedOffset, Utc};
use rusqlite::{params, Connection};

use crate::{
  domain::{DailyDate, RunSlug, ServerId, TaskCount},
  model::{ActivityEvent, EventKind},
};

/// 判断待ちと異常に数える出来事の種類。
pub const ATTENTION_KINDS: [EventKind; 2] = [EventKind::Inquiry, EventKind::AwaitingMerge];

/// JST の日次集計の1日分。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DailyCount {
  /// JST の日を "YYYY-MM-DD" で表した日付。
  pub date:      DailyDate,
  pub total:     i32,
  pub attention: i32,
}

/// サーバーごとの出来事の件数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerCount {
  pub server_id: ServerId,
  pub count:     i32,
}

/// 出来事の記録とクエリを提供する。
pub struct ActivityStore {
  connection: Mutex<Connection>,
}

impl ActivityStore {
  /// 指定したパスの SQLite を開き、スキーマを用意する。
  pub fn open(path: &Path) -> rusqlite::Result<Self> {
    let connection = Connection::open(path)?;
    let store = ActivityStore { connection: Mutex::new(connection) };
    store.migrate()?;
    Ok(store)
  }

  /// メモリ上の SQLite を開く（テスト用）。
  pub fn open_in_memory() -> rusqlite::Result<Self> {
    let connection = Connection::open_in_memory()?;
    let store = ActivityStore { connection: Mutex::new(connection) };
    store.migrate()?;
    Ok(store)
  }

  fn migrate(&self) -> rusqlite::Result<()> {
    self.connection.lock().expect("SQLite のロックが壊れています").execute_batch(
      "CREATE TABLE IF NOT EXISTS events (
         id INTEGER PRIMARY KEY AUTOINCREMENT,
         occurred_at TEXT NOT NULL,
         server_id TEXT NOT NULL,
         project TEXT,
         kind TEXT NOT NULL,
         body TEXT NOT NULL,
         run_slug TEXT
       );
       CREATE INDEX IF NOT EXISTS events_occurred_at ON events (occurred_at DESC);",
    )
  }

  /// 出来事を1件記録する。
  pub fn record(&self, event: &ActivityEvent) -> rusqlite::Result<()> {
    self.connection.lock().expect("SQLite のロックが壊れています").execute(
      "INSERT INTO events (occurred_at, server_id, project, kind, body, run_slug)
       VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
      params![
        format_timestamp(event.occurred_at),
        event.server_id,
        event.project,
        event.kind.as_str(),
        event.body,
        event.run_slug.as_ref().map(RunSlug::as_str),
      ],
    )?;
    Ok(())
  }

  /// 出来事群を1つのトランザクションとして記録する。
  ///
  /// 群の途中で保存に失敗した場合は、それまでの INSERT を残さず群全体を記録しません。
  pub fn record_all(&self, events: &[ActivityEvent]) -> rusqlite::Result<()> {
    let mut connection = self.connection.lock().expect("SQLite のロックが壊れています");
    let transaction = connection.transaction()?;
    for event in events {
      transaction.execute(
        "INSERT INTO events (occurred_at, server_id, project, kind, body, run_slug)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
          format_timestamp(event.occurred_at),
          event.server_id,
          event.project,
          event.kind.as_str(),
          event.body,
          event.run_slug.as_ref().map(RunSlug::as_str),
        ],
      )?;
    }
    transaction.commit()
  }

  /// 出来事の一覧を新しい順に指定件数だけ返す。
  pub fn recent_events(&self, limit: usize) -> rusqlite::Result<Vec<ActivityEvent>> {
    let connection = self.connection.lock().expect("SQLite のロックが壊れています");
    let mut statement = connection.prepare(
      "SELECT occurred_at, server_id, project, kind, body, run_slug
       FROM events ORDER BY occurred_at DESC, id DESC LIMIT ?1",
    )?;
    let rows = statement.query_map(params![limit as i64], |row| {
      Ok((
        row.get::<_, String>(0)?,
        row.get::<_, String>(1)?,
        row.get::<_, Option<String>>(2)?,
        row.get::<_, String>(3)?,
        row.get::<_, String>(4)?,
        row.get::<_, Option<String>>(5)?,
      ))
    })?;
    let mut events = Vec::new();
    for row in rows {
      let (occurred_at, server_id, project, kind, body, run_slug) = row?;
      let kind =
        EventKind::from_stored(&kind).ok_or_else(|| conversion_error(3, format!("未知の出来事の種類です: {kind}")))?;
      let run_slug = match run_slug {
        | Some(text) => Some(RunSlug::try_from(text.as_str()).map_err(|error| conversion_error(5, error.to_string()))?),
        | None => None,
      };
      events.push(ActivityEvent {
        occurred_at: parse_timestamp(&occurred_at),
        server_id,
        project,
        kind,
        body,
        run_slug,
      });
    }
    Ok(events)
  }

  /// `now` を含む JST の日から遡って7日分の日次集計を、古い日から並べて返す。
  /// 出来事がない日は 0 件として含めます。
  pub fn daily_summaries(&self, now: DateTime<Utc>) -> rusqlite::Result<Vec<DailyCount>> {
    let jst = FixedOffset::east_opt(9 * 3600).expect("JST のオフセットは固定です");
    let today = now.with_timezone(&jst).date_naive();
    let mut summaries = Vec::new();
    for back in (0..7).rev() {
      let day = today - Duration::days(back);
      let date = DailyDate::from(day);
      let total = self.count_on(date.as_str(), None)?;
      let attention = self.count_on(date.as_str(), Some(&ATTENTION_KINDS))?;
      summaries.push(DailyCount { date, total, attention });
    }
    Ok(summaries)
  }

  fn count_on(&self, jst_date: &str, kinds: Option<&[EventKind]>) -> rusqlite::Result<i32> {
    let connection = self.connection.lock().expect("SQLite のロックが壊れています");
    let count: i64 = match kinds {
      | None => connection.query_row(
        "SELECT COUNT(*) FROM events WHERE date(occurred_at, '+9 hours') = ?1",
        params![jst_date],
        |row| row.get(0),
      )?,
      | Some(kinds) => {
        let placeholders = kinds.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
        let sql =
          format!("SELECT COUNT(*) FROM events WHERE date(occurred_at, '+9 hours') = ?1 AND kind IN ({placeholders})");
        let mut values: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(jst_date.to_owned())];
        for kind in kinds {
          values.push(Box::new(kind.as_str().to_owned()));
        }
        let params = rusqlite::params_from_iter(values.iter().map(|value| value.as_ref()));
        connection.query_row(&sql, params, |row| row.get(0))?
      },
    };
    Ok(count as i32)
  }

  /// `now` を含む JST の日の、サーバーごとの出来事の件数を返す。
  pub fn today_counts_by_server(&self, now: DateTime<Utc>) -> rusqlite::Result<Vec<ServerCount>> {
    let jst = FixedOffset::east_opt(9 * 3600).expect("JST のオフセットは固定です");
    let date = now.with_timezone(&jst).date_naive().format("%Y-%m-%d").to_string();
    let connection = self.connection.lock().expect("SQLite のロックが壊れています");
    let mut statement = connection.prepare(
      "SELECT server_id, COUNT(*) FROM events WHERE date(occurred_at, '+9 hours') = ?1
       GROUP BY server_id ORDER BY COUNT(*) DESC, server_id",
    )?;
    let rows = statement.query_map(params![date], |row| {
      let server_id: String = row.get(0)?;
      let server_id = ServerId::try_from(server_id.as_str()).map_err(|error| conversion_error(0, error.to_string()))?;
      Ok(ServerCount { server_id, count: row.get::<_, i64>(1)? as i32 })
    })?;
    let mut counts = Vec::new();
    for row in rows {
      counts.push(row?);
    }
    Ok(counts)
  }

  /// 指定したプロジェクトで、`since` より後に完了したタスクの件数を返す。
  pub fn completed_tasks_since(&self, project: &str, since: DateTime<Utc>) -> rusqlite::Result<TaskCount> {
    let count: i64 = self.connection.lock().expect("SQLite のロックが壊れています").query_row(
      "SELECT COUNT(*) FROM events
       WHERE project = ?1 AND kind = ?2 AND occurred_at >= ?3",
      params![project, EventKind::TaskCompleted.as_str(), format_timestamp(since)],
      |row| row.get(0),
    )?;
    TaskCount::try_from(count).map_err(|error| conversion_error(0, error.to_string()))
  }
}

/// 保存済みの値がドメインの値オブジェクトとして復元できないことを表す SQL の変換エラー。
fn conversion_error(index: usize, message: String) -> rusqlite::Error {
  rusqlite::Error::FromSqlConversionFailure(
    index,
    rusqlite::types::Type::Text,
    Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, message)),
  )
}

/// ミリ秒固定幅の RFC3339 UTC 文字列に変換する。文字列の大小が時刻の大小と一致します。
pub fn format_timestamp(time: DateTime<Utc>) -> String {
  time.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

fn parse_timestamp(text: &str) -> DateTime<Utc> {
  DateTime::parse_from_rfc3339(text)
    .map(|time| time.with_timezone(&Utc))
    .unwrap_or_else(|_| DateTime::<Utc>::UNIX_EPOCH)
}

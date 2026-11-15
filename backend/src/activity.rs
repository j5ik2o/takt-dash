//! 出来事の SQLite への記録と、アクティビティのクエリ。
//!
//! 日付の区切りは日本標準時（JST）の日で行います。判断待ちと異常に数える種類は
//! 問い合わせ・マージ待ち・応答なしの3種類だけです。

use std::{
  path::Path,
  sync::{
    atomic::{AtomicU64, Ordering},
    Condvar, Mutex, MutexGuard,
  },
  time::{Duration as StdDuration, Instant},
};

use chrono::{DateTime, Duration, FixedOffset, Utc};
use rusqlite::{params, Connection, OpenFlags};

use crate::model::{ActivityEvent, EventKind};

/// 読み取り専用の接続を用意する本数。
const READ_CONNECTIONS: usize = 4;
/// SQLite がロック解放を待つ上限。待ちを計測する対象は Rust 側の Mutex と接続プール。
const SQLITE_BUSY_TIMEOUT: StdDuration = StdDuration::from_secs(5);

/// 判断待ちと異常に数える出来事の種類。
pub const ATTENTION_KINDS: [EventKind; 3] = [EventKind::Inquiry, EventKind::AwaitingMerge, EventKind::ConnectionLost];

/// JST の日次集計の1日分。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DailyCount {
  /// JST の日を "YYYY-MM-DD" で表した文字列。
  pub date:      String,
  pub total:     i32,
  pub attention: i32,
}

/// サーバーごとの出来事の件数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerCount {
  pub server_id: String,
  pub count:     i32,
}

/// 出来事の記録とクエリを提供する。
pub struct ActivityStore {
  /// 書き込み用の接続。読み取りが writer を待たないよう、ファイルの SQLite では使わない。
  writer:             Mutex<Connection>,
  readers:            Readers,
  /// 接続のロック取得を待った合計時間（µs）。処理時間と分けて観測するために累積する。
  lock_wait_total_us: AtomicU64,
}

/// 読み取り接続の持ち方。`:memory:` は接続ごとに別の DB になるため、writer と共有する。
enum Readers {
  SharedWithWriter,
  Pool(ReaderPool),
}

/// 読み取り専用の接続を貸し出すプール。空の間は Condvar で待つ。
struct ReaderPool {
  connections: Mutex<Vec<Connection>>,
  available:   Condvar,
}

/// プールから借りた接続を、読み取りの終了時に必ず返す。
struct CheckedOutReader<'a> {
  pool:       &'a ReaderPool,
  connection: Option<Connection>,
}

impl CheckedOutReader<'_> {
  fn connection(&self) -> &Connection {
    self.connection.as_ref().expect("読み取り接続を借りている間だけ参照できます")
  }
}

impl Drop for CheckedOutReader<'_> {
  fn drop(&mut self) {
    if let Some(connection) = self.connection.take() {
      self.pool.connections.lock().expect("読み取り接続プールのロックが壊れています").push(connection);
      self.pool.available.notify_one();
    }
  }
}

impl ReaderPool {
  /// 読み取り専用で `READ_CONNECTIONS` 本の接続を開く。
  fn open(path: &Path) -> rusqlite::Result<Self> {
    let mut connections = Vec::with_capacity(READ_CONNECTIONS);
    for _ in 0..READ_CONNECTIONS {
      let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI | OpenFlags::SQLITE_OPEN_NO_MUTEX,
      )?;
      configure(&connection)?;
      connections.push(connection);
    }
    Ok(ReaderPool { connections: Mutex::new(connections), available: Condvar::new() })
  }

  /// 接続を1本借りる。空なら空くまで待つ。
  fn checkout(&self) -> CheckedOutReader<'_> {
    let mut connections = self.connections.lock().expect("読み取り接続プールのロックが壊れています");
    while connections.is_empty() {
      connections = self.available.wait(connections).expect("読み取り接続プールの待機が壊れています");
    }
    let connection = connections.pop().expect("空でないことを確認済みです");
    drop(connections);
    CheckedOutReader { pool: self, connection: Some(connection) }
  }
}

impl ActivityStore {
  /// 指定したパスの SQLite を開き、スキーマを用意する。
  pub fn open(path: &Path) -> rusqlite::Result<Self> {
    let writer = Connection::open(path)?;
    configure(&writer)?;
    migrate(&writer)?;
    Ok(ActivityStore {
      writer:             Mutex::new(writer),
      readers:            Readers::Pool(ReaderPool::open(path)?),
      lock_wait_total_us: AtomicU64::new(0),
    })
  }

  /// メモリ上の SQLite を開く（テスト用）。
  pub fn open_in_memory() -> rusqlite::Result<Self> {
    let writer = Connection::open_in_memory()?;
    configure(&writer)?;
    migrate(&writer)?;
    Ok(ActivityStore {
      writer:             Mutex::new(writer),
      readers:            Readers::SharedWithWriter,
      lock_wait_total_us: AtomicU64::new(0),
    })
  }

  /// 書き込み用の接続のロックを取得する。取得までに待った時間を累積へ加算する。
  fn writer(&self) -> MutexGuard<'_, Connection> {
    let started_at = Instant::now();
    let guard = self.writer.lock().expect("SQLite のロックが壊れています");
    self.record_wait(started_at);
    guard
  }

  /// 読み取り用の接続で `read` を実行する。接続の取り出しに待った時間を累積へ加算する。
  fn with_reader<T>(&self, read: impl FnOnce(&Connection) -> T) -> T {
    match &self.readers {
      | Readers::SharedWithWriter => {
        let writer = self.writer();
        read(&writer)
      },
      | Readers::Pool(pool) => {
        let started_at = Instant::now();
        let reader = pool.checkout();
        self.record_wait(started_at);
        read(reader.connection())
      },
    }
  }

  /// 接続の取得を待った時間を累積へ1回だけ加算する。
  fn record_wait(&self, started_at: Instant) {
    self.lock_wait_total_us.fetch_add(started_at.elapsed().as_micros() as u64, Ordering::Relaxed);
  }

  /// これまでに接続のロック取得を待った合計時間（µs）。
  pub(crate) fn lock_wait_total_us(&self) -> u64 {
    self.lock_wait_total_us.load(Ordering::Relaxed)
  }

  /// 出来事を1件記録する。
  pub fn record(&self, event: &ActivityEvent) -> rusqlite::Result<()> {
    self.writer().execute(
      "INSERT INTO events (occurred_at, server_id, project, kind, body, run_slug)
       VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
      params![
        format_timestamp(event.occurred_at),
        event.server_id,
        event.project,
        event.kind.as_str(),
        event.body,
        event.run_slug,
      ],
    )?;
    Ok(())
  }

  /// 出来事の一覧を新しい順に指定件数だけ返す。
  pub fn recent_events(&self, limit: usize) -> rusqlite::Result<Vec<ActivityEvent>> {
    self.with_reader(|connection| {
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
        let kind = EventKind::from_stored(&kind).ok_or_else(|| {
          rusqlite::Error::FromSqlConversionFailure(
            3,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("未知の出来事の種類です: {kind}"))),
          )
        })?;
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
    })
  }

  /// `now` を含む JST の日から遡って7日分の日次集計を、古い日から並べて返す。
  /// 出来事がない日は 0 件として含めます。
  pub fn daily_summaries(&self, now: DateTime<Utc>) -> rusqlite::Result<Vec<DailyCount>> {
    self.with_reader(|connection| {
      let jst = FixedOffset::east_opt(9 * 3600).expect("JST のオフセットは固定です");
      let today = now.with_timezone(&jst).date_naive();
      let mut summaries = Vec::new();
      for back in (0..7).rev() {
        let day = today - Duration::days(back);
        let date = day.format("%Y-%m-%d").to_string();
        let total = count_on(connection, date.as_str(), None)?;
        let attention = count_on(connection, date.as_str(), Some(&ATTENTION_KINDS))?;
        summaries.push(DailyCount { date, total, attention });
      }
      Ok(summaries)
    })
  }

  /// `now` を含む JST の日の、サーバーごとの出来事の件数を返す。
  pub fn today_counts_by_server(&self, now: DateTime<Utc>) -> rusqlite::Result<Vec<ServerCount>> {
    self.with_reader(|connection| {
      let jst = FixedOffset::east_opt(9 * 3600).expect("JST のオフセットは固定です");
      let date = now.with_timezone(&jst).date_naive().format("%Y-%m-%d").to_string();
      let mut statement = connection.prepare(
        "SELECT server_id, COUNT(*) FROM events WHERE date(occurred_at, '+9 hours') = ?1
         GROUP BY server_id ORDER BY COUNT(*) DESC, server_id",
      )?;
      let rows = statement.query_map(params![date], |row| {
        Ok(ServerCount { server_id: row.get(0)?, count: row.get::<_, i64>(1)? as i32 })
      })?;
      let mut counts = Vec::new();
      for row in rows {
        counts.push(row?);
      }
      Ok(counts)
    })
  }

  /// 指定したプロジェクトで、`since` より後に完了したタスクの件数を返す。
  pub fn completed_tasks_since(&self, project: &str, since: DateTime<Utc>) -> rusqlite::Result<i32> {
    self.with_reader(|connection| {
      let count: i64 = connection.query_row(
        "SELECT COUNT(*) FROM events
         WHERE project = ?1 AND kind = ?2 AND occurred_at >= ?3",
        params![project, EventKind::TaskCompleted.as_str(), format_timestamp(since)],
        |row| row.get(0),
      )?;
      Ok(count as i32)
    })
  }
}

/// SQLite の待ち上限を設定する。
fn configure(connection: &Connection) -> rusqlite::Result<()> {
  connection.busy_timeout(SQLITE_BUSY_TIMEOUT)
}

/// スキーマを用意する。
fn migrate(connection: &Connection) -> rusqlite::Result<()> {
  connection.execute_batch(
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

/// JST の日付に一致する出来事の件数を数える。
fn count_on(connection: &Connection, jst_date: &str, kinds: Option<&[EventKind]>) -> rusqlite::Result<i32> {
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

/// ミリ秒固定幅の RFC3339 UTC 文字列に変換する。文字列の大小が時刻の大小と一致します。
pub fn format_timestamp(time: DateTime<Utc>) -> String {
  time.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

fn parse_timestamp(text: &str) -> DateTime<Utc> {
  DateTime::parse_from_rfc3339(text)
    .map(|time| time.with_timezone(&Utc))
    .unwrap_or_else(|_| DateTime::<Utc>::UNIX_EPOCH)
}

#[cfg(test)]
mod tests {
  use std::{
    sync::{mpsc, Arc, Barrier},
    thread,
    time::Duration,
  };

  use super::*;

  fn activity_event(at: &str, kind: EventKind, body: &str) -> ActivityEvent {
    ActivityEvent {
      occurred_at: DateTime::parse_from_rfc3339(at).expect("時刻を解析できます").with_timezone(&Utc),
      server_id: "mac-mini".to_owned(),
      project: Some("takt-dash".to_owned()),
      kind,
      body: body.to_owned(),
      run_slug: None,
    }
  }

  fn file_backed_store() -> (tempfile::TempDir, ActivityStore) {
    let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
    let path = directory.path().join("activity.sqlite");
    let store = ActivityStore::open(&path).expect("ファイルの SQLite を開けます");
    (directory, store)
  }

  #[test]
  fn sqlite_lock_wait_is_counted_while_the_connection_is_held() {
    let store = Arc::new(ActivityStore::open_in_memory().expect("メモリ上の SQLite を開けます"));
    let acquired = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));

    let holder = {
      let store = store.clone();
      let acquired = acquired.clone();
      let release = release.clone();
      thread::spawn(move || {
        let writer = store.writer();
        acquired.wait();
        release.wait();
        drop(writer);
      })
    };

    acquired.wait();
    let reader = {
      let store = store.clone();
      thread::spawn(move || store.recent_events(10).expect("保持中のロックを待って読み取れます"))
    };
    // 読み取り側が接続ロックの取得で待つ時間を作ってから、保持側に解放させる。
    thread::sleep(Duration::from_millis(50));
    release.wait();

    reader.join().expect("読み取りスレッドが終了します");
    holder.join().expect("保持スレッドが終了します");
    let waited = store.lock_wait_total_us();
    assert!(waited >= 1_000, "ロック保持中の呼び出しは取得待ちとして加算されます: {waited}µs");
  }

  #[test]
  fn file_backed_reads_do_not_wait_for_the_writer_lock() {
    let (_directory, store) = file_backed_store();
    store
      .record(&activity_event("2026-11-13T10:00:00Z", EventKind::GoalCreated, "登録"))
      .expect("出来事を記録できます");
    let store = Arc::new(store);

    let acquired = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let holder = {
      let store = store.clone();
      let acquired = acquired.clone();
      let release = release.clone();
      thread::spawn(move || {
        let _writer = store.writer();
        acquired.wait();
        release.wait();
      })
    };
    acquired.wait();

    let (read_done, read_finished) = mpsc::channel();
    let reader = {
      let store = store.clone();
      thread::spawn(move || {
        let events = store.recent_events(10).expect("writer ロックを待たずに読み取れます");
        let _ = read_done.send(events.len());
        events
      })
    };

    // writer ロックを保持したまま、読み取りが完了するかを観測する。
    let finished_while_held = read_finished.recv_timeout(Duration::from_millis(500)).ok();
    release.wait();
    let events = reader.join().expect("読み取りスレッドが終了します");
    holder.join().expect("保持スレッドが終了します");

    assert_eq!(finished_while_held, Some(1), "writer ロック保持中でも読み取りは待たずに完了します");
    assert_eq!(events.len(), 1, "writer が記録した出来事を読み取れます");
  }

  #[test]
  fn reader_checkout_wait_is_counted_in_lock_wait_total() {
    let (_directory, store) = file_backed_store();
    let store = Arc::new(store);

    let hold = Arc::new(Barrier::new(READ_CONNECTIONS + 1));
    let release = Arc::new(Barrier::new(READ_CONNECTIONS + 1));
    let holders: Vec<_> = (0..READ_CONNECTIONS)
      .map(|_| {
        let store = store.clone();
        let hold = hold.clone();
        let release = release.clone();
        thread::spawn(move || {
          store.with_reader(|_| {
            hold.wait();
            release.wait();
          });
        })
      })
      .collect();
    hold.wait();

    let waited_before = store.lock_wait_total_us();
    let waiter = {
      let store = store.clone();
      thread::spawn(move || store.with_reader(|_| ()))
    };
    // 読み取り接続が不足したまま待たせてから、保持側に解放させる。
    thread::sleep(Duration::from_millis(50));
    release.wait();
    waiter.join().expect("読み取り接続の待ちが解けて終了します");
    for holder in holders {
      holder.join().expect("読み取り接続を保持したスレッドが終了します");
    }

    let waited = store.lock_wait_total_us().saturating_sub(waited_before);
    assert!(waited >= 1_000, "読み取り接続の取り出し待ちも lock_wait_total_us へ加算されます: {waited}µs");
  }
}

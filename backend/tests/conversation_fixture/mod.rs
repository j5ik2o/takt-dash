//! 会話の読み取りのテストで共有する、一時的な manager の記録と opencode の DB。

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use rusqlite::Connection;
use takt_dash_backend::conversation::{ConversationPaths, ConversationReader};

/// `session_v2` と `session_message` だけを持つ、会話の読み取りに許可された最小のスキーマ。
pub const CONVERSATION_SCHEMA: &str = "
CREATE TABLE session_v2 (
  id TEXT PRIMARY KEY,
  directory TEXT NOT NULL,
  time_updated INTEGER NOT NULL
);
CREATE TABLE session_message (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL,
  type TEXT NOT NULL,
  seq INTEGER NOT NULL,
  time_created INTEGER NOT NULL,
  data TEXT NOT NULL
);
CREATE UNIQUE INDEX session_message_session_seq_idx ON session_message (session_id, seq);
";

/// 実ホームの代わりに使う、manager の記録と opencode の DB の一時的な置き場所。
pub struct ConversationFixture {
  pub home:          PathBuf,
  pub conversations: PathBuf,
  pub database:      PathBuf,
}

impl ConversationFixture {
  /// 一時ディレクトリと、その配下に作った会話の保存先を返す。ディレクトリはテストが保持する。
  pub fn new() -> (tempfile::TempDir, ConversationFixture) {
    let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
    let fixture = ConversationFixture::under(directory.path());
    (directory, fixture)
  }

  /// 既存のホームの配下に、空の DB と conversations ディレクトリを作る。
  pub fn under(home: &Path) -> ConversationFixture {
    std::fs::create_dir_all(home).expect("ホームのディレクトリを作れます");
    let home = std::fs::canonicalize(home).expect("ホームの実パスを解決できます");
    let conversations = home.join(".takt").join("conversations");
    let database = home.join(".local").join("share").join("opencode").join("opencode.db");
    std::fs::create_dir_all(&conversations).expect("conversations ディレクトリを作れます");
    std::fs::create_dir_all(database.parent().expect("DB の親ディレクトリがあります"))
      .expect("opencode のディレクトリを作れます");
    let fixture = ConversationFixture { home, conversations, database };
    fixture.replace_schema(CONVERSATION_SCHEMA);
    fixture
  }

  /// 保存先を指す読み取りを作る。
  pub fn reader(&self) -> ConversationReader {
    ConversationReader::new(ConversationPaths::new(self.conversations.clone(), self.database.clone()))
  }

  /// ホームの配下にプロジェクトの実在ディレクトリを作り、実パスを返す。
  pub fn project_dir(&self, name: &str) -> PathBuf {
    let directory = self.home.join("projects").join(name);
    std::fs::create_dir_all(&directory).expect("プロジェクトのディレクトリを作れます");
    std::fs::canonicalize(&directory).expect("プロジェクトの実パスを解決できます")
  }

  fn connection(&self) -> Connection {
    Connection::open(&self.database).expect("会話の DB を開けます")
  }

  /// 実運用と同じ WAL モードの書き込み接続を開いて返す。テストが保持する間は接続を閉じない。
  pub fn writer(&self) -> Connection {
    let connection = self.connection();
    let mode: String =
      connection.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0)).expect("WAL モードを設定できます");
    assert_eq!(mode.to_lowercase(), "wal", "書き込み接続は WAL モードで開く必要があります");
    connection
  }

  /// 書き込み接続の `journal_mode` を返す。
  pub fn journal_mode(&self) -> String {
    self
      .connection()
      .query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))
      .expect("journal_mode を読めます")
  }

  /// DB を作り直し、指定したスキーマだけを適用する。
  pub fn replace_schema(&self, schema: &str) {
    let candidates =
      [self.database.clone(), self.database.with_extension("db-wal"), self.database.with_extension("db-shm")];
    for path in candidates {
      let _ = std::fs::remove_file(path);
    }
    self.connection().execute_batch(schema).expect("テストのスキーマを適用できます");
  }

  /// `session_v2` に1セッションを記録する。
  pub fn insert_session(&self, id: &str, directory: &Path, time_updated: i64) {
    self
      .connection()
      .execute("INSERT INTO session_v2 (id, directory, time_updated) VALUES (?1, ?2, ?3)", rusqlite::params![
        id,
        directory.display().to_string(),
        time_updated
      ])
      .expect("セッションを記録できます");
  }

  /// `session_v2` に、`directory` 列を持たない1セッションを記録する。
  pub fn insert_session_without_directory(&self, id: &str, time_updated: i64) {
    self
      .connection()
      .execute("INSERT INTO session_v2 (id, time_updated) VALUES (?1, ?2)", rusqlite::params![id, time_updated])
      .expect("directory を持たないセッションを記録できます");
  }

  /// `session_message` に1行を記録する。
  pub fn insert_message(&self, session_id: &str, seq: i64, kind: &str, time_created: i64, data: &str) {
    self
      .connection()
      .execute(
        "INSERT INTO session_message (id, session_id, type, seq, time_created, data)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![format!("msg-{session_id}-{seq}"), session_id, kind, seq, time_created, data],
      )
      .expect("メッセージを記録できます");
  }

  /// `~/.takt/conversations/manager-<stem>.json` を書く。
  pub fn write_manager(&self, stem: &str, session_id: &str, repository_directory: &Path, provider: &str) {
    let record = serde_json::json!({
      "agent": "manager",
      "assignment": {
        "kind": "repository",
        "repositoryDirectory": repository_directory.display().to_string(),
      },
      "provider": provider,
      "schemaVersion": 1,
      "sessionId": session_id,
    });
    std::fs::write(self.conversations.join(format!("manager-{stem}.json")), record.to_string())
      .expect("manager の記録を書けます");
  }

  /// `manager-<stem>.json` の更新時刻を指定秒に固定する。
  pub fn set_manager_modified(&self, stem: &str, seconds_since_epoch: u64) {
    let path = self.conversations.join(format!("manager-{stem}.json"));
    let modified = std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds_since_epoch);
    let file = std::fs::File::options().write(true).open(&path).expect("manager の記録を開けます");
    file.set_modified(modified).expect("manager の記録の更新時刻を設定できます");
  }
}

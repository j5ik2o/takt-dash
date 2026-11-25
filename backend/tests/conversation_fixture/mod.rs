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

/// `## このターンの入力` の包みを持つ user の `data` を、実データと同じ形で組み立てる。
pub fn wrapped_user_data(previous_text: &str, human_text: &str, time: i64) -> String {
  let speech = serde_json::to_string(&serde_json::json!({
    "speaker": { "namespace": "local", "id": "operator" },
    "text": human_text,
  }))
  .expect("人の発言の JSON を組み立てられます");
  let text = format!(
    r#"
## 前のターンで受け取った人の発言

{previous_text}

## このターンの入力

このターンは人の発言から始まりました。発言者と本文を示します。本人の依頼と引用を分けて読み、対象に沿って反映します。

{speech}

Business context (not approval): {{"assignment":{{"kind":"repository","repositoryDirectory":"/tmp/takt-dash"}},"pending":null,"goals":[]}}

次の JSON schema に一致する fenced JSON block をちょうど1つ返してください:

```json
{{
  "type": "object",
  "properties": {{
    "message": {{ "type": "string", "minLength": 1 }},
    "summary": {{ "type": ["object", "null"] }}
  }},
  "required": ["message", "summary"]
}}
```

JSON block の前後に余計なテキストを含めないでください。

StructuredOutput ツールは呼び出さないでください。このリクエストでは利用できません。
"#
  );
  serde_json::to_string(&serde_json::json!({ "time": { "created": time }, "text": text }))
    .expect("user の data を組み立てられます")
}

/// `## このターンの入力` の見出しだけを持ち、`speaker` を持たない JSON を含む user の `data`。
pub fn wrapped_user_without_speaker_data(internal_text: &str, time: i64) -> String {
  let draft = serde_json::to_string(&serde_json::json!({ "text": internal_text }))
    .expect("speaker を持たない JSON を組み立てられます");
  let text = format!(
    "\n## このターンの入力\n\nこのターンはイベントから始まりました。\n\n{draft}\n\nBusiness context (not approval): {{}}\n"
  );
  serde_json::to_string(&serde_json::json!({ "time": { "created": time }, "text": text }))
    .expect("user の data を組み立てられます")
}

/// 応答形式の指示（JSON schema）。実データでは包みの末尾に必ず付く。
const RESPONSE_FORMAT: &str = r#"次の JSON schema に一致する fenced JSON block をちょうど1つ返してください:

```json
{
  "type": "object",
  "properties": {
    "message": { "type": "string", "minLength": 1 },
    "summary": { "type": ["object", "null"] }
  },
  "required": ["message", "summary"]
}
```

JSON block の前後に余計なテキストを含めないでください。"#;

/// 実データの Business context の行。
pub const BUSINESS_CONTEXT: &str = r#"Business context (not approval): {"assignment":{"kind":"repository","repositoryDirectory":"/tmp/takt-dash"},"pending":null,"epics":[]}"#;

/// 人の発言ではないターンの包み。見出しのあとの空行、説明文、JSON の行、Business context、
/// 応答形式の指示の並びを、実データと同じ形で組み立てる。
fn non_speech_turn_data(description: Option<&str>, input: &serde_json::Value, business_context: bool, time: i64) -> String {
  let mut text = String::from("\n## このターンの入力\n\n\n\n\n\n\n\n");
  if let Some(description) = description {
    text.push_str(description);
    text.push_str("\n\n\n\n\n");
  }
  text.push_str(&serde_json::to_string(input).expect("入力の JSON を組み立てられます"));
  text.push_str("\n\n");
  if business_context {
    text.push_str(BUSINESS_CONTEXT);
    text.push_str("\n\n");
  }
  text.push_str(RESPONSE_FORMAT);
  text.push('\n');
  serde_json::to_string(&serde_json::json!({ "time": { "created": time }, "text": text }))
    .expect("user の data を組み立てられます")
}

/// イベントから始まったターンの user の `data`。`{"event":{…},"instruction":"…"}` の行を持つ。
pub fn event_turn_data(description: Option<&str>, event: &serde_json::Value, business_context: bool, time: i64) -> String {
  let input = serde_json::json!({
    "event": event,
    "instruction": "To advance the assigned goals, compare the change conveyed by this event with the current state.",
  });
  non_speech_turn_data(description, &input, business_context, time)
}

/// 保存した質問への人の回答から始まったターンの user の `data`。
pub fn inquiry_answered_data(question: &str, answer: &str, business_context: bool, time: i64) -> String {
  let input = serde_json::json!({
    "kind": "inquiry_answered",
    "repositoryDirectory": "/tmp/takt-dash",
    "goalId": "394569b2-57db-4b07-8bc1-3ca0b51988bc",
    "questionId": "751b5b5f-f32f-4105-b0d3-683f7c32f29c",
    "question": question,
    "answer": { "text": answer, "source": "takt-mcp", "answeredAt": "2026-11-20T08:00:00.000Z" },
  });
  non_speech_turn_data(
    Some("このターンは保存した質問への人の回答から始まりました。配送はイベントや director を経由しています。質問と回答を対応させ、人の判断を対象の作業に反映します。goalId が null の場合はゴールに属さない質問です。"),
    &input,
    business_context,
    time,
  )
}

/// 包みを持たない user の `data`。
pub fn plain_user_data(text: &str, time: i64) -> String {
  serde_json::to_string(&serde_json::json!({ "text": text, "time": time }))
    .expect("包みのない user の data を組み立てられます")
}

/// `message` と `summary` を持つ単一の fenced JSON block を本文に置く assistant の `data`。
pub fn fenced_assistant_data(message: &str, summary_objective: Option<&str>, time: i64) -> String {
  let summary = match summary_objective {
    | Some(objective) => serde_json::json!({ "objective": objective }),
    | None => serde_json::Value::Null,
  };
  let document = serde_json::json!({ "message": message, "summary": summary });
  let text =
    format!("```json\n{}\n```", serde_json::to_string_pretty(&document).expect("fenced JSON block を組み立てられます"));
  serde_json::to_string(&serde_json::json!({
    "content": [{ "type": "text", "text": text }],
    "time": { "created": time },
  }))
  .expect("assistant の data を組み立てられます")
}

/// `message` を持たない fenced JSON block を本文に置く assistant の `data`。
pub fn fenced_assistant_without_message_data(reason: &str, time: i64) -> String {
  let document = serde_json::json!({ "step": 2, "reason": reason });
  let text =
    format!("```json\n{}\n```", serde_json::to_string(&document).expect("fenced JSON block を組み立てられます"));
  serde_json::to_string(&serde_json::json!({
    "content": [{ "type": "text", "text": text }],
    "time": { "created": time },
  }))
  .expect("assistant の data を組み立てられます")
}

/// 解釈できない fenced JSON block を本文に置く assistant の `data`。
pub fn invalid_fenced_assistant_data(time: i64) -> String {
  let text = "```json\n{not json\n```";
  serde_json::to_string(&serde_json::json!({
    "content": [{ "type": "text", "text": text }],
    "time": { "created": time },
  }))
  .expect("assistant の data を組み立てられます")
}

/// 包み付きの user の人の発言へ、指定個数の置換文字（U+FFFD）を含めた `data`。
pub fn garbled_user_data(replacement_count: usize, time: i64) -> String {
  let mut human_text = String::from("この発言は元記録で壊れています。");
  human_text.push_str(&"\u{FFFD}".repeat(replacement_count));
  wrapped_user_data("前のターンの本文", &human_text, time)
}

/// `data` の `text` をそのまま取り出す。
pub fn data_text(data: &str) -> String {
  let value: serde_json::Value = serde_json::from_str(data).expect("data を解釈できます");
  value.get("text").and_then(serde_json::Value::as_str).expect("text を持ちます").to_owned()
}

/// assistant の `data.content` の最初の text 要素をそのまま取り出す。
pub fn assistant_text(data: &str) -> String {
  let value: serde_json::Value = serde_json::from_str(data).expect("data を解釈できます");
  value["content"][0]["text"].as_str().expect("content の text を持ちます").to_owned()
}

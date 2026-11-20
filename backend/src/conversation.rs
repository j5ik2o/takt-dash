//! manager の会話を保存場所から直接読む。
//!
//! `~/.takt/conversations/manager-*.json` の sessionId と assignment を手掛かりに、
//! opencode の `opencode.db` の `session_v2` と `session_message` を読み取り専用で参照します。
//! 参照する列は許可したものだけに限り、provider 内部の `data` は解釈できた本文だけを返します。
//! 最新セッションの判定には、provider に依存しない manager の記録の更新時刻を使います。
//! 保存先やスキーマが無い場合はエラーにせず、空のページと理由を返します。

use std::{
  collections::BTreeMap,
  fs,
  path::{Component, Path, PathBuf},
};

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OpenFlags};
use serde::Deserialize;
use serde_json::Value;

use crate::domain::ProjectName;

/// 会話の保存先。conversations ディレクトリと opencode の DB のパスを持つ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationPaths {
  pub conversations_dir: PathBuf,
  pub database_path:     PathBuf,
}

impl ConversationPaths {
  /// 保存先を明示して作る。
  pub fn new(conversations_dir: PathBuf, database_path: PathBuf) -> Self {
    ConversationPaths { conversations_dir, database_path }
  }

  /// ホームディレクトリの配下の既定の保存先を指す。
  pub fn from_home(home: impl AsRef<Path>) -> Self {
    let home = home.as_ref();
    ConversationPaths {
      conversations_dir: home.join(".takt").join("conversations"),
      database_path:     home.join(".local").join("share").join("opencode").join("opencode.db"),
    }
  }
}

/// 保存先から manager の会話を読む読み取り。
#[derive(Debug, Clone)]
pub struct ConversationReader {
  paths: ConversationPaths,
}

impl ConversationReader {
  /// 保存先を指定して読み取りを作る。
  pub fn new(paths: ConversationPaths) -> Self {
    ConversationReader { paths }
  }

  /// 要求に一致する manager の会話を1ページ分返す。
  ///
  /// 保存先やスキーマの問題はエラーにせず、空のページの `unavailable_reason` に残します。
  pub fn read(&self, query: &ConversationQuery) -> ConversationPage {
    let mut page = ConversationPage {
      project:            query.project.clone(),
      session_id:         String::new(),
      provider:           String::new(),
      messages:           Vec::new(),
      next_after_seq:     query.after_seq,
      has_more:           false,
      skipped:            Vec::new(),
      unavailable_reason: None,
    };

    let records = match self.manager_records() {
      | Ok(records) => records,
      | Err(reason) => {
        page.unavailable_reason = Some(reason);
        return page;
      },
    };
    let project_cwd = normalize(&query.cwd);
    let candidates: Vec<&ManagerRecord> =
      records.iter().filter(|record| normalize(&record.repository_directory) == project_cwd).collect();
    if candidates.is_empty() {
      page.unavailable_reason = Some("このプロジェクトの manager の記録がありません".to_owned());
      return page;
    }

    // 未対応 provider の理由が opencode.db の不在で隠れないよう、記録の選択と provider 判定を DB
    // を開く前に行う。
    let selected = match select_record(&candidates, query.session_id.as_deref()) {
      | Ok(record) => record,
      | Err(reason) => {
        page.unavailable_reason = Some(reason);
        return page;
      },
    };
    if selected.provider != "opencode" {
      page.unavailable_reason =
        Some(format!("provider {} の会話にはまだ対応していません", display_provider(&selected.provider)));
      return page;
    }

    let connection = match open_read_only(&self.paths.database_path) {
      | Ok(connection) => connection,
      | Err(reason) => {
        page.unavailable_reason = Some(reason);
        return page;
      },
    };

    let session_directory = match session_directory(&connection, &selected.session_id) {
      | Ok(Some(directory)) => normalize(&directory),
      | Ok(None) => {
        page.unavailable_reason = Some("manager の記録に対応するセッションがありません".to_owned());
        return page;
      },
      | Err(reason) => {
        page.unavailable_reason = Some(reason);
        return page;
      },
    };
    let configured_roots = configured_roots(&project_cwd, &query.project_roots);
    let owning_root = configured_roots
      .iter()
      .filter(|root| session_directory == **root || session_directory.starts_with(root.as_path()))
      .max_by_key(|root| root.as_os_str().len());
    if owning_root.is_some_and(|root| *root != project_cwd) {
      page.unavailable_reason = Some("manager の記録のセッションが別のプロジェクトに属しています".to_owned());
      return page;
    }

    page.session_id = selected.session_id.clone();
    page.provider = selected.provider.clone();

    let limit = query.limit.max(1);
    let fetch = limit.saturating_add(1);
    let fetch_limit = i64::try_from(fetch).unwrap_or(i64::MAX);
    let messages = match read_messages(&connection, &selected.session_id, query.after_seq, fetch_limit) {
      | Ok(messages) => messages,
      | Err(reason) => {
        page.unavailable_reason = Some(reason);
        return page;
      },
    };
    let has_more = messages.len() > limit;
    let examined = &messages[..messages.len().min(limit)];
    page.next_after_seq = examined.last().map(|row| row.seq).unwrap_or(query.after_seq);
    page.has_more = has_more;

    let mut skipped: BTreeMap<String, u32> = BTreeMap::new();
    for row in examined {
      match classify(row) {
        | Ok((speaker, raw_text)) => {
          let presented = present_body(speaker, raw_text);
          page.messages.push(ConversationMessage {
            seq: row.seq,
            occurred_at: DateTime::<Utc>::from_timestamp_millis(row.time_created)
              .unwrap_or(DateTime::<Utc>::UNIX_EPOCH),
            speaker,
            text: presented.text,
            notice: presented.notice,
          });
        },
        | Err(reason) => *skipped.entry(reason).or_insert(0) += 1,
      }
    }
    page.skipped = skipped.into_iter().map(|(reason, count)| SkippedSummary { reason, count }).collect();
    for summary in &page.skipped {
      tracing::warn!(
        session_id = %selected.session_id,
        reason = %summary.reason,
        count = summary.count,
        "会話の行をスキップしました"
      );
    }
    page
  }

  fn manager_records(&self) -> Result<Vec<ManagerRecord>, String> {
    let entries = fs::read_dir(&self.paths.conversations_dir)
      .map_err(|_| "会話の保存先のディレクトリが見つかりません".to_owned())?;
    let mut records = Vec::new();
    for entry in entries {
      let Ok(entry) = entry else {
        continue;
      };
      let path = entry.path();
      let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        continue;
      };
      if !name.starts_with("manager-") || !name.ends_with(".json") {
        continue;
      }
      let Ok(text) = fs::read_to_string(&path) else {
        continue;
      };
      let Ok(record) = serde_json::from_str::<RawManagerRecord>(&text) else {
        continue;
      };
      if record.agent.as_deref().is_some_and(|agent| agent != "manager") {
        continue;
      }
      let Some(session_id) = record.session_id.filter(|session_id| !session_id.is_empty()) else {
        continue;
      };
      let Some(repository_directory) = record.assignment.and_then(|assignment| assignment.repository_directory) else {
        continue;
      };
      let modified = match fs::metadata(&path).and_then(|metadata| metadata.modified()) {
        | Ok(modified) => modified,
        | Err(_) => return Err("manager の記録の更新時刻を読めません".to_owned()),
      };
      records.push(ManagerRecord {
        session_id,
        repository_directory: PathBuf::from(repository_directory),
        provider: record.provider.unwrap_or_default(),
        modified,
      });
    }
    Ok(records)
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ManagerRecord {
  session_id:           String,
  repository_directory: PathBuf,
  provider:             String,
  modified:             std::time::SystemTime,
}

#[derive(Debug, Deserialize)]
struct RawManagerRecord {
  agent:      Option<String>,
  assignment: Option<RawAssignment>,
  provider:   Option<String>,
  #[serde(rename = "sessionId")]
  session_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawAssignment {
  #[serde(rename = "repositoryDirectory")]
  repository_directory: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MessageRow {
  seq:          i64,
  time_created: i64,
  kind:         String,
  data:         String,
}

fn open_read_only(path: &Path) -> Result<Connection, String> {
  let connection = Connection::open_with_flags(
    path,
    OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX | OpenFlags::SQLITE_OPEN_URI,
  )
  .map_err(|_| "会話のデータベースを開けません".to_owned())?;
  connection
    .execute_batch("PRAGMA query_only = ON;")
    .map_err(|_| "会話のデータベースを読み取り専用にできません".to_owned())?;
  Ok(connection)
}

/// 読み取りの対象にする manager の記録を選ぶ。
///
/// `session_id` を省略したときは、manager の記録の更新時刻で最新を決める。opencode の DB に
/// `session_v2` の行があるかで候補を絞ると、provider を切り替えたプロジェクトで古い opencode の
/// 会話が最新として残るため、provider に依存しない記録の更新時刻だけを順位の根拠にします。
/// 最新の時刻が同じで provider が異なるときは、どれが最新か確定できないため理由を返します。
/// 最新の時刻が同じで provider も同じときは、`fs::read_dir` の列挙順に依存せず
/// `session_id` の昇順で先頭を選び、同じ入力から常に同じ記録を返します。
fn select_record(candidates: &[&ManagerRecord], session_id: Option<&str>) -> Result<ManagerRecord, String> {
  match session_id {
    | Some(session_id) => candidates
      .iter()
      .find(|record| record.session_id == session_id)
      .map(|record| (*record).clone())
      .ok_or_else(|| "指定されたセッションはこのプロジェクトにありません".to_owned()),
    | None => {
      let latest_modified = candidates
        .iter()
        .map(|record| record.modified)
        .max()
        .ok_or_else(|| "このプロジェクトの manager の記録がありません".to_owned())?;
      let mut latest = candidates.iter().copied().filter(|record| record.modified == latest_modified);
      let first = latest.next().ok_or_else(|| "このプロジェクトの manager の記録がありません".to_owned())?;
      let mut provider_differs = false;
      let mut tie_break = first;
      for record in latest {
        if record.provider != first.provider {
          provider_differs = true;
        }
        if (record.session_id.as_str(), record.provider.as_str())
          < (tie_break.session_id.as_str(), tie_break.provider.as_str())
        {
          tie_break = record;
        }
      }
      if provider_differs {
        return Err("複数の provider の manager の記録が同じ更新時刻にあり、最新を確定できません".to_owned());
      }
      Ok(tie_break.clone())
    },
  }
}

fn session_directory(connection: &Connection, session_id: &str) -> Result<Option<PathBuf>, String> {
  match connection
    .query_row("SELECT directory FROM session_v2 WHERE id = ?1", params![session_id], |row| row.get::<_, String>(0))
  {
    | Ok(directory) => Ok(Some(PathBuf::from(directory))),
    | Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
    | Err(_) => Err("会話のデータベースのスキーマを読めません".to_owned()),
  }
}

fn read_messages(
  connection: &Connection,
  session_id: &str,
  after_seq: i64,
  limit: i64,
) -> Result<Vec<MessageRow>, String> {
  let mut statement = connection
    .prepare(
      "SELECT seq, time_created, type, data FROM session_message
       WHERE session_id = ?1 AND seq > ?2 ORDER BY seq ASC LIMIT ?3",
    )
    .map_err(|_| "会話のデータベースのスキーマを読めません".to_owned())?;
  let rows = statement
    .query_map(params![session_id, after_seq, limit], |row| {
      Ok(MessageRow {
        seq:          row.get(0)?,
        time_created: row.get(1)?,
        kind:         row.get(2)?,
        data:         row.get(3)?,
      })
    })
    .map_err(|_| "会話のデータベースのスキーマを読めません".to_owned())?;
  let mut messages = Vec::new();
  for row in rows {
    messages.push(row.map_err(|_| "会話のメッセージを読めません".to_owned())?);
  }
  Ok(messages)
}

fn classify(row: &MessageRow) -> Result<(ConversationSpeaker, String), String> {
  match row.kind.as_str() {
    | "user" => {
      let value = parse_data(&row.data)?;
      let text = text_field(&value).ok_or_else(|| "本文が無い".to_owned())?;
      Ok((ConversationSpeaker::User, text))
    },
    | "assistant" => {
      let value = parse_data(&row.data)?;
      let texts: Vec<&str> = value
        .get("content")
        .and_then(Value::as_array)
        .map(|elements| {
          elements
            .iter()
            .filter(|element| element.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|element| element.get("text").and_then(Value::as_str))
            .filter(|text| !text.is_empty())
            .collect()
        })
        .unwrap_or_default();
      if texts.is_empty() {
        return Err("本文が無い".to_owned());
      }
      Ok((ConversationSpeaker::Manager, texts.join("\n")))
    },
    | "system" => {
      let value = parse_data(&row.data)?;
      let text = text_field(&value).ok_or_else(|| "本文が無い".to_owned())?;
      Ok((ConversationSpeaker::System, text))
    },
    | other => Err(format!("対応していない type {other}")),
  }
}

/// 置換文字（U+FFFD）がこの個数以上なら、元記録の文字化けとして印を付ける。
const GARBLED_NOTICE_THRESHOLD: usize = 10;

/// 表示用に整形した本文と、本文に添える注意書き。
#[derive(Debug, Clone, PartialEq, Eq)]
struct PresentedBody {
  text:   String,
  notice: Option<String>,
}

/// 話者ごとに保存形式から人が読める本文を取り出し、文字化けの印を決める。
///
/// 包みや fenced JSON を解釈できないときは、元の本文をそのまま使います。
fn present_body(speaker: ConversationSpeaker, raw_text: String) -> PresentedBody {
  let text = match speaker {
    | ConversationSpeaker::User => unwrap_user_input(&raw_text).unwrap_or(raw_text),
    | ConversationSpeaker::Manager => unwrap_assistant_message(&raw_text).unwrap_or(raw_text),
    | ConversationSpeaker::System => raw_text,
  };
  let notice = garbled_notice(&text);
  PresentedBody { text, notice }
}

/// `## このターンの入力` の包みから、`speaker` を持つ JSON の `text` を取り出す。
///
/// 見出しより後だけを対象にし、行単位で JSON を解釈します。`speaker` を持たない
/// JSON や、包みの無い本文は対象にしません。前のターンの本文に同じ見出しが
/// 含まれていても、現在の包みは行頭の見出しのうち最後のものです。
fn unwrap_user_input(raw: &str) -> Option<String> {
  const MARKER: &str = "## このターンの入力";
  let start = raw
    .match_indices(MARKER)
    .filter(|(index, _)| *index == 0 || raw.as_bytes()[index - 1] == b'\n')
    .last()
    .map(|(index, _)| index + MARKER.len())?;
  for line in raw[start..].lines() {
    let trimmed = line.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
      continue;
    }
    let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
      continue;
    };
    if value.get("speaker").is_none() {
      continue;
    }
    if let Some(text) = value.get("text").and_then(Value::as_str).filter(|text| !text.is_empty()) {
      return Some(text.to_owned());
    }
  }
  None
}

/// 本文全体が `message` を持つ単一の fenced JSON block のとき、`message` を取り出す。
///
/// 解釈できない、または `message` が無い・空のときは `None` を返します。
fn unwrap_assistant_message(raw: &str) -> Option<String> {
  let trimmed = raw.trim();
  let inner = trimmed.strip_prefix("```json")?.strip_suffix("```")?;
  let value = serde_json::from_str::<Value>(inner.trim()).ok()?;
  value.get("message").and_then(Value::as_str).filter(|message| !message.is_empty()).map(str::to_owned)
}

/// 本文の置換文字を数え、しきい値以上なら元記録の文字化けを示す印を返す。
///
/// 本文そのものは変更しません。
fn garbled_notice(text: &str) -> Option<String> {
  let count = text.chars().filter(|character| *character == '\u{FFFD}').count();
  (count >= GARBLED_NOTICE_THRESHOLD).then(|| format!("元記録で文字化けしています（置換文字 {count} 個）"))
}

fn parse_data(data: &str) -> Result<Value, String> {
  serde_json::from_str(data).map_err(|_| "data を解釈できません".to_owned())
}

fn text_field(value: &Value) -> Option<String> {
  value.get("text").and_then(Value::as_str).filter(|text| !text.is_empty()).map(str::to_owned)
}

fn display_provider(provider: &str) -> &str {
  if provider.is_empty() {
    "不明"
  } else {
    provider
  }
}

fn configured_roots(project_cwd: &Path, project_roots: &[PathBuf]) -> Vec<PathBuf> {
  let mut roots = vec![project_cwd.to_path_buf()];
  for root in project_roots {
    let root = normalize(root);
    if !roots.contains(&root) {
      roots.push(root);
    }
  }
  roots
}

fn normalize(path: &Path) -> PathBuf {
  fs::canonicalize(path).unwrap_or_else(|_| {
    let absolute = if path.is_absolute() {
      path.to_path_buf()
    } else {
      std::env::current_dir().map(|directory| directory.join(path)).unwrap_or_else(|_| path.to_path_buf())
    };
    lexical_normalize(&absolute)
  })
}

fn lexical_normalize(path: &Path) -> PathBuf {
  let mut normalized = PathBuf::new();
  for component in path.components() {
    match component {
      | Component::CurDir => {},
      | Component::ParentDir => {
        if !normalized.pop() {
          normalized.push(component.as_os_str());
        }
      },
      | other => normalized.push(other.as_os_str()),
    }
  }
  normalized
}

/// 会話の取得を要求する。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationQuery {
  pub project:       ProjectName,
  pub cwd:           PathBuf,
  pub project_roots: Vec<PathBuf>,
  pub session_id:    Option<String>,
  pub after_seq:     i64,
  pub limit:         usize,
}

/// 会話の1件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationMessage {
  pub seq:         i64,
  pub occurred_at: DateTime<Utc>,
  pub speaker:     ConversationSpeaker,
  pub text:        String,
  pub notice:      Option<String>,
}

/// 会話の発話者。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversationSpeaker {
  User,
  Manager,
  System,
}

/// 解釈できずスキップした行の理由と件数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedSummary {
  pub reason: String,
  pub count:  u32,
}

/// 会話の1ページ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationPage {
  pub project:            ProjectName,
  pub session_id:         String,
  pub provider:           String,
  pub messages:           Vec<ConversationMessage>,
  pub next_after_seq:     i64,
  pub has_more:           bool,
  pub skipped:            Vec<SkippedSummary>,
  pub unavailable_reason: Option<String>,
}

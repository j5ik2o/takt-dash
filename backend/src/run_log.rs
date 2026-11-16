//! `.takt` 配下の run を保存場所から直接読む。
//!
//! `.takt/tasks.yaml` の対応表から run のディレクトリを解決し、`meta.json` と
//! 本体ログ（`logs/*.jsonl`）を読み取ります。run の識別子からパスを組み立てず、
//! 解決したパスが worktree の置き場かプロジェクトの `.takt/runs` の下にあることを
//! 確認してから読みます。
//!
//! 本体ログは最後の改行までを追記読みし、読んだ位置を reader の中だけで保持します。
//! プロンプト本文（`systemPrompt` などの項目）は結果へ含めず、残す文字列は長さの上限で
//! 要約します。ログ群の最新更新時刻はファイルの更新時刻だけで決め、レコードの
//! `timestamp` は使いません。保存先が無い場合はエラーにせず、残っていないことを
//! 結果として返します。
//!
//! `meta.json`、`logs`、`logs/phase1-execution` は実パスへ正規化し、run の
//! ディレクトリの下にあることを確かめてから読みます。シンボリックリンクで外を指すものは
//! 無いものとして扱います。
//!
//! 前提:
//! - 読み取り層は同期の関数として提供し、非同期の処理の中から直接呼びません。呼ぶ側は
//!   `spawn_blocking` などで別スレッドから呼びます。
//! - offset は永続化しません。再起動時は `meta.json` と本体ログの末尾の一定量だけを読みます。

use std::{
  collections::BTreeMap,
  fs::{self, File},
  io::{self, Read, Seek, SeekFrom},
  os::unix::fs::MetadataExt,
  path::{Component, Path, PathBuf},
};

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;

use crate::domain::RunSlug;

const META_FILE: &str = "meta.json";
const LOGS_DIRECTORY: &str = "logs";
const BODY_LOG_EXTENSION: &str = "jsonl";
const PHASE1_DIRECTORY: &str = "phase1-execution";
const MAX_SUMMARY_CHARS: usize = 512;
const PROMPT_FIELDS: [&str; 4] = ["systemPrompt", "userInstruction", "instruction", "content"];

/// run の場所の解決結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunLocation {
  /// 対応表から解決でき、run データ（`meta.json` または本体ログ）が残っている run。
  Found(PathBuf),
  /// 対応表にはあるが、run データを持つディレクトリが残っていない run。
  Missing,
  /// 対応表に対応する project と run の組が無い。
  NotListed,
  /// 対応表を読めなかった。呼び出し側は前回の値を使える。
  Unreadable(String),
  /// 解決したパスが許可された置き場の外にある。
  Rejected(String),
}

/// `meta.json` から読む run の状態。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunMeta {
  pub status:            Option<String>,
  pub current_step:      Option<String>,
  pub current_iteration: Option<i64>,
  pub phase:             Option<i64>,
  pub updated_at:        Option<String>,
}

/// `meta.json` の読み取り結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetaRead {
  Found(RunMeta),
  Missing,
  Unreadable(String),
}

/// 本体ログの追記読みの結果。
#[derive(Debug, Clone, PartialEq)]
pub struct BodyLogRead {
  /// 4 キーを除去し長さの上限を適用したレコード。
  pub records: Vec<Value>,
  /// 解釈できずに飛ばした行の件数。
  pub skipped: usize,
  /// 本体ログの置き場が残っていない。
  pub missing: bool,
}

#[derive(Debug, Clone, Copy)]
struct Cursor {
  offset: u64,
  inode:  u64,
}

#[derive(Debug, Clone, Default)]
struct TaskEntry {
  run_slug:        Option<String>,
  source_run_slug: Option<String>,
  worktree_path:   Option<PathBuf>,
}

/// `.takt` 配下の run を読む読み取り。読んだ位置はこのインスタンスの中だけで保持する。
#[derive(Debug, Clone)]
pub struct RunLogReader {
  project_cwd:    PathBuf,
  worktrees_root: PathBuf,
  cursors:        BTreeMap<PathBuf, Cursor>,
}

impl RunLogReader {
  /// プロジェクトの作業ディレクトリと worktree の置き場を指定して読み取りを作る。
  pub fn new(project_cwd: PathBuf, worktrees_root: PathBuf) -> Self {
    RunLogReader {
      project_cwd:    normalize(&project_cwd),
      worktrees_root: normalize(&worktrees_root),
      cursors:        BTreeMap::new(),
    }
  }

  /// 検証済みの run の識別子を、対応表と許可ルート検査を通して run のディレクトリへ解決する。
  pub fn resolve(&self, run_slug: &RunSlug) -> RunLocation {
    let run_slug = run_slug.as_str();
    let entries = match self.read_task_entries() {
      | Ok(entries) => entries,
      | Err(reason) => return RunLocation::Unreadable(reason),
    };
    let entry = entries
      .iter()
      .find(|entry| entry.run_slug.as_deref() == Some(run_slug))
      .or_else(|| entries.iter().find(|entry| entry.source_run_slug.as_deref() == Some(run_slug)));
    let Some(entry) = entry else {
      return RunLocation::NotListed;
    };
    let candidates = match self.run_candidates(entry, run_slug) {
      | Ok(candidates) => candidates,
      | Err(reason) => return RunLocation::Rejected(reason),
    };
    match candidates.into_iter().find(|candidate| has_run_data(candidate)) {
      | Some(run_dir) => RunLocation::Found(run_dir),
      | None => RunLocation::Missing,
    }
  }

  /// `meta.json` から status、currentStep、currentIteration、phase、updatedAt を読む。
  pub fn read_meta(&self, run_dir: &Path) -> MetaRead {
    let path = match contained_path(run_dir, Path::new(META_FILE)) {
      | Ok(Some(path)) => path,
      | Ok(None) => return MetaRead::Missing,
      | Err(error) if error.kind() == io::ErrorKind::NotFound => return MetaRead::Missing,
      | Err(error) => return MetaRead::Unreadable(format!("meta.json を読めません: {error}")),
    };
    let text = match fs::read_to_string(&path) {
      | Ok(text) => text,
      | Err(error) if error.kind() == std::io::ErrorKind::NotFound => return MetaRead::Missing,
      | Err(error) => return MetaRead::Unreadable(format!("meta.json を読めません: {error}")),
    };
    let raw: RawMeta = match serde_json::from_str(&text) {
      | Ok(raw) => raw,
      | Err(error) => return MetaRead::Unreadable(format!("meta.json を解釈できません: {error}")),
    };
    MetaRead::Found(RunMeta {
      status:            raw.status,
      current_step:      raw.current_step,
      current_iteration: raw.current_iteration,
      phase:             raw.phase,
      updated_at:        raw.updated_at,
    })
  }

  /// 本体ログの追記分を読む。書きかけの最終行は消費せず、位置を行の先頭に残す。
  pub fn read_body_log(&mut self, run_dir: &Path) -> BodyLogRead {
    let logs_dir = contained_dir(run_dir, Path::new(LOGS_DIRECTORY));
    let missing = logs_dir.is_none();
    let mut records = Vec::new();
    let mut skipped = 0;

    for path in logs_dir.as_deref().map(body_log_files).unwrap_or_default() {
      let Ok(metadata) = fs::metadata(&path) else {
        continue;
      };
      let size = metadata.len();
      let inode = metadata.ino();
      let cursor = self.cursors.entry(path.clone()).or_insert(Cursor { offset: 0, inode });
      if size < cursor.offset || cursor.inode != inode {
        cursor.offset = 0;
        cursor.inode = inode;
      }
      let start = cursor.offset;
      let Ok(mut file) = File::open(&path) else {
        continue;
      };
      if file.seek(SeekFrom::Start(start)).is_err() {
        continue;
      }
      let mut chunk = Vec::new();
      if file.read_to_end(&mut chunk).is_err() {
        continue;
      }
      let Some(last_newline) = chunk.iter().rposition(|byte| *byte == b'\n') else {
        continue;
      };
      let consumed = &chunk[..=last_newline];

      for line in consumed.split(|byte| *byte == b'\n') {
        if line.is_empty() {
          continue;
        }
        let Ok(text) = std::str::from_utf8(line) else {
          skipped += 1;
          continue;
        };
        let Ok(mut record) = serde_json::from_str::<Value>(text) else {
          skipped += 1;
          continue;
        };
        if !record.is_object() {
          skipped += 1;
          continue;
        }
        sanitize(&mut record);
        records.push(record);
      }

      if let Some(cursor) = self.cursors.get_mut(&path) {
        cursor.offset = start + consumed.len() as u64;
        cursor.inode = inode;
      }
    }

    BodyLogRead { records, skipped, missing }
  }

  /// 本体ログと `logs/phase1-execution/` のファイル更新時刻の最大値を返す。
  pub fn latest_log_update(&self, run_dir: &Path) -> Option<DateTime<Utc>> {
    let body_logs =
      contained_dir(run_dir, Path::new(LOGS_DIRECTORY)).as_deref().map(body_log_files).unwrap_or_default();
    let phase1_logs = contained_dir(run_dir, &Path::new(LOGS_DIRECTORY).join(PHASE1_DIRECTORY))
      .as_deref()
      .map(regular_files)
      .unwrap_or_default();
    let mut latest: Option<DateTime<Utc>> = None;
    let paths = body_logs.into_iter().chain(phase1_logs);
    for path in paths {
      let Ok(metadata) = fs::metadata(&path) else {
        continue;
      };
      let Ok(modified) = metadata.modified() else {
        continue;
      };
      let modified = DateTime::<Utc>::from(modified);
      latest = Some(match latest {
        | Some(current) if current > modified => current,
        | _ => modified,
      });
    }
    latest
  }

  fn project_runs_root(&self) -> PathBuf {
    self.project_cwd.join(".takt").join("runs")
  }

  fn read_task_entries(&self) -> Result<Vec<TaskEntry>, String> {
    let path = self.project_cwd.join(".takt").join("tasks.yaml");
    let text = fs::read_to_string(&path).map_err(|error| format!("tasks.yaml を読めません: {error}"))?;
    parse_task_entries(&text)
  }

  /// 対応表の項目から run の候補を、worktree 側・プロジェクト側の順に組み立てる。
  fn run_candidates(&self, entry: &TaskEntry, run_slug: &str) -> Result<Vec<PathBuf>, String> {
    let mut bases = Vec::new();
    if let Some(worktree_path) = &entry.worktree_path {
      if !worktree_path.is_absolute() {
        return Err(format!("worktree_path {} は絶対パスではありません", worktree_path.display()));
      }
      bases.push(normalize(worktree_path));
    }
    bases.push(self.project_cwd.clone());

    let mut candidates = Vec::with_capacity(bases.len());
    for base in bases {
      let run_dir = normalize(&base.join(".takt").join("runs").join(run_slug));
      let allowed = run_dir.starts_with(&self.worktrees_root) || run_dir.starts_with(self.project_runs_root());
      if !allowed {
        return Err(format!("解決したパス {} は許可された置き場の外にあります", run_dir.display()));
      }
      candidates.push(run_dir);
    }
    Ok(candidates)
  }
}

/// run のデータ（`meta.json` または本体ログ）を持つディレクトリかを返す。
fn has_run_data(run_dir: &Path) -> bool {
  if run_dir.join("meta.json").is_file() {
    return true;
  }
  !body_log_files(&run_dir.join("logs")).is_empty()
}

#[derive(Debug, Deserialize)]
struct RawMeta {
  status:            Option<String>,
  #[serde(rename = "currentStep")]
  current_step:      Option<String>,
  #[serde(rename = "currentIteration")]
  current_iteration: Option<i64>,
  phase:             Option<i64>,
  #[serde(rename = "updatedAt")]
  updated_at:        Option<String>,
}

fn parse_task_entries(text: &str) -> Result<Vec<TaskEntry>, String> {
  let mut entries = Vec::new();
  let mut current: Option<TaskEntry> = None;
  let mut saw_tasks = false;

  for line in text.lines() {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
      continue;
    }
    if trimmed == "tasks:" {
      saw_tasks = true;
      continue;
    }
    if let Some(content) = list_item_content(line) {
      if let Some(entry) = current.take() {
        entries.push(entry);
      }
      let mut entry = TaskEntry::default();
      if let Some((key, value)) = split_field(content) {
        apply_field(&mut entry, key, value);
      }
      current = Some(entry);
      continue;
    }
    if let Some(content) = item_field_content(line) {
      let Some(entry) = current.as_mut() else {
        continue;
      };
      if let Some((key, value)) = split_field(content) {
        apply_field(entry, key, value);
      }
    }
  }
  if let Some(entry) = current {
    entries.push(entry);
  }
  if !saw_tasks {
    return Err("tasks の一覧がありません".to_owned());
  }
  Ok(entries)
}

fn list_item_content(line: &str) -> Option<&str> {
  let rest = line.strip_prefix("  -")?;
  if rest.is_empty() {
    return Some("");
  }
  rest.strip_prefix(' ')
}

fn item_field_content(line: &str) -> Option<&str> {
  let rest = line.strip_prefix("    ")?;
  if rest.starts_with(' ') {
    return None;
  }
  Some(rest)
}

fn split_field(content: &str) -> Option<(&str, &str)> {
  let (key, value) = content.split_once(':')?;
  let key = key.trim_end();
  if key.is_empty() || key.contains(char::is_whitespace) {
    return None;
  }
  let value = value.trim();
  if value.is_empty() || value.starts_with('>') || value.starts_with('|') {
    return None;
  }
  Some((key, value))
}

fn apply_field(entry: &mut TaskEntry, key: &str, value: &str) {
  match key {
    | "run_slug" => entry.run_slug = Some(value.to_owned()),
    | "source_run_slug" => entry.source_run_slug = Some(value.to_owned()),
    | "worktree_path" => entry.worktree_path = Some(PathBuf::from(value)),
    | _ => {},
  }
}

fn sanitize(value: &mut Value) {
  match value {
    | Value::Object(map) => {
      map.retain(|key, _| !PROMPT_FIELDS.contains(&key.as_str()));
      for value in map.values_mut() {
        sanitize(value);
      }
    },
    | Value::Array(items) => {
      for item in items {
        sanitize(item);
      }
    },
    | Value::String(text) => {
      if text.chars().count() > MAX_SUMMARY_CHARS {
        *text = text.chars().take(MAX_SUMMARY_CHARS).collect();
      }
    },
    | _ => {},
  }
}

/// run_dir の下の `relative` を実パスへ正規化し、正規化した run_dir の下にあれば返す。
/// シンボリックリンクで外を指す場合は `Ok(None)`、正規化できない場合はそのエラーを返す。
fn contained_path(run_dir: &Path, relative: &Path) -> io::Result<Option<PathBuf>> {
  let root = fs::canonicalize(run_dir)?;
  let child = fs::canonicalize(run_dir.join(relative))?;
  Ok(child.starts_with(&root).then_some(child))
}

/// run_dir の下に収まるディレクトリとして `relative` の実パスを返す。外を指す、無い、
/// ディレクトリでない場合は `None` を返す。
fn contained_dir(run_dir: &Path, relative: &Path) -> Option<PathBuf> {
  contained_path(run_dir, relative).ok().flatten().filter(|path| path.is_dir())
}

fn body_log_files(directory: &Path) -> Vec<PathBuf> {
  let mut files = regular_files(directory);
  files.retain(|path| path.extension().and_then(|extension| extension.to_str()) == Some(BODY_LOG_EXTENSION));
  files
}

fn regular_files(directory: &Path) -> Vec<PathBuf> {
  let Ok(entries) = fs::read_dir(directory) else {
    return Vec::new();
  };
  let mut files = Vec::new();
  for entry in entries.flatten() {
    let path = entry.path();
    if entry.file_type().is_ok_and(|file_type| file_type.is_file()) {
      files.push(path);
    }
  }
  files.sort();
  files
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

//! `.takt` 配下の run を保存場所から直接読む。
//!
//! `.takt/tasks.yaml` の対応表から run のディレクトリを解決し、`meta.json` と
//! 本体ログ（`logs/*.jsonl`）とレポートの一覧（`reports/`）を読み取ります。レポートの本文と、
//! プロジェクト側の `interventions.jsonl` に記録された指示書も読みます。
//! run の識別子からパスを組み立てず、解決したパスが worktree の置き場か
//! プロジェクトの `.takt/runs` の下にあることを確認してから読みます。
//!
//! 本体ログは最後の改行までを追記読みし、読んだ位置を reader の中だけで保持します。読み位置を
//! まだ持たないファイルを先頭から読むか、末尾の一定量から読むかは [`InitialRead`] で決めます。
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
//! - offset は永続化しません。run 詳細の読み取りは [`InitialRead::Tail`] で、読み位置を持たない
//!   本体ログを末尾の一定量だけ読みます。

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

use crate::domain::{InstructionId, ReportFilename, RunSlug};

const META_FILE: &str = "meta.json";
const LOGS_DIRECTORY: &str = "logs";
const BODY_LOG_EXTENSION: &str = "jsonl";
const PHASE1_DIRECTORY: &str = "phase1-execution";
const REPORTS_DIRECTORY: &str = "reports";
/// takt の summary が一覧に載せるレポートの名前の接尾辞（takt の `isMarkdownReport` と同じ判定）。
const REPORT_FILE_SUFFIX: &str = ".md";
const MAX_SUMMARY_CHARS: usize = 512;
const PROMPT_FIELDS: [&str; 4] = ["systemPrompt", "userInstruction", "instruction", "content"];
/// takt の LiveInterventionFileStore が指示書のイベントを書くファイル名。
const INTERVENTIONS_FILE: &str = "interventions.jsonl";

/// 読み位置をまだ持たない本体ログを、どこから読み始めるか。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitialRead {
  /// 先頭から読む。定期読み取りが使います。
  Start,
  /// 末尾の指定バイト数だけを読む。範囲の先頭にかかった書きかけの行は捨てます。
  Tail(u64),
}

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
  pub workflow:          Option<String>,
  pub start_time:        Option<String>,
  pub status:            Option<String>,
  pub current_step:      Option<String>,
  pub current_iteration: Option<i64>,
  pub phase:             Option<i64>,
  pub updated_at:        Option<String>,
  /// タスクの本文。
  pub task:              Option<String>,
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
  pub records:      Vec<Value>,
  /// 解釈できずに飛ばした行の件数。
  pub skipped:      usize,
  /// 本体ログの置き場が残っていない。
  pub missing:      bool,
  /// この呼び出しで読んだ本体ログのバイト数の合計。末尾から読み始めるときに、範囲の直前が行の
  /// 区切りかを確かめるために読む1バイトは含めません。
  pub bytes_read:   u64,
  /// 末尾の範囲から読み始め、範囲より前の記録を読んでいないファイルがあった。
  pub skipped_head: bool,
  /// 読み位置を使わずに読み始めたファイル（初めて読む、縮小した、置き換わった）があった。
  pub restarted:    bool,
}

/// run の `reports/` の下にある1件のレポート。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportFile {
  /// `reports/` からの `/` 区切りの相対パス。
  pub filename:   String,
  pub size_bytes: u64,
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
  initial_read:   InitialRead,
}

impl RunLogReader {
  /// プロジェクトの作業ディレクトリと worktree の置き場を指定して、本体ログを先頭から読む読み取りを
  /// 作る。
  pub fn new(project_cwd: PathBuf, worktrees_root: PathBuf) -> Self {
    Self::with_initial_read(project_cwd, worktrees_root, InitialRead::Start)
  }

  /// 読み位置を持たない本体ログの読み始めを指定して読み取りを作る。
  pub fn with_initial_read(project_cwd: PathBuf, worktrees_root: PathBuf, initial_read: InitialRead) -> Self {
    RunLogReader {
      project_cwd: normalize(&project_cwd),
      worktrees_root: normalize(&worktrees_root),
      cursors: BTreeMap::new(),
      initial_read,
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
      workflow:          raw.workflow,
      start_time:        raw.start_time,
      status:            raw.status,
      current_step:      raw.current_step,
      current_iteration: raw.current_iteration,
      phase:             raw.phase,
      updated_at:        raw.updated_at,
      task:              raw.task,
    })
  }

  /// 本体ログの追記分を読む。書きかけの最終行は消費せず、位置を行の先頭に残す。
  ///
  /// 読み位置を持たないファイル（初めて読む、縮小した、置き換わった）は [`InitialRead`] に従って
  /// 読み始めます。末尾から読み始めた場合は、範囲の直前の1バイトで行の区切りかを確かめ、範囲の
  /// 先頭が行の途中なら、その書きかけの行を解釈せずに捨てます。範囲の先頭が行頭なら捨てません。
  pub fn read_body_log(&mut self, run_dir: &Path) -> BodyLogRead {
    let logs_dir = contained_dir(run_dir, Path::new(LOGS_DIRECTORY));
    let missing = logs_dir.is_none();
    let mut records = Vec::new();
    let mut skipped = 0;
    let mut bytes_read = 0;
    let mut skipped_head = false;
    let mut restarted = false;

    for path in logs_dir.as_deref().map(body_log_files).unwrap_or_default() {
      let Ok(metadata) = fs::metadata(&path) else {
        continue;
      };
      let size = metadata.len();
      let inode = metadata.ino();
      let (start, from_tail) = match self.cursors.get(&path) {
        | Some(cursor) if cursor.inode == inode && size >= cursor.offset => (cursor.offset, false),
        | _ => {
          restarted = true;
          match self.initial_read {
            | InitialRead::Tail(limit) if size > limit => (size - limit, true),
            | InitialRead::Tail(_) | InitialRead::Start => (0, false),
          }
        },
      };
      // 末尾から読むときは、範囲の直前の1バイトも読み、範囲の先頭が行頭かを確かめる。
      let read_from = if from_tail { start - 1 } else { start };
      let Ok(mut file) = File::open(&path) else {
        continue;
      };
      if file.seek(SeekFrom::Start(read_from)).is_err() {
        continue;
      }
      let mut chunk = Vec::new();
      if file.read_to_end(&mut chunk).is_err() {
        continue;
      }
      let line_start = if from_tail {
        let Some((boundary, range)) = chunk.split_first() else {
          continue;
        };
        bytes_read += range.len() as u64;
        if *boundary == b'\n' {
          1
        } else {
          // 改行が無ければ完結した行が無いため、読み位置を持たずに次回も末尾から読み直す。
          let Some(first_newline) = range.iter().position(|byte| *byte == b'\n') else {
            continue;
          };
          1 + first_newline + 1
        }
      } else {
        bytes_read += chunk.len() as u64;
        0
      };
      if from_tail {
        skipped_head = true;
      }
      let body = &chunk[line_start..];
      let consumed_len = body.iter().rposition(|byte| *byte == b'\n').map_or(0, |last_newline| last_newline + 1);

      for line in body[..consumed_len].split(|byte| *byte == b'\n') {
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

      self.cursors.insert(path, Cursor { offset: read_from + (line_start + consumed_len) as u64, inode });
    }

    BodyLogRead { records, skipped, missing, bytes_read, skipped_head, restarted }
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

  /// `reports/` の下にある、名前が `.md` で終わる通常ファイルを、相対パスの昇順で返す。
  /// シンボリックリンクはたどらない。
  pub fn list_reports(&self, run_dir: &Path) -> Vec<ReportFile> {
    let Some(reports_dir) = contained_dir(run_dir, Path::new(REPORTS_DIRECTORY)) else {
      return Vec::new();
    };
    let mut reports = Vec::new();
    collect_reports(&reports_dir, "", &mut reports);
    reports.sort_by(|left, right| left.filename.cmp(&right.filename));
    reports
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

/// レポートか指示書の本文の読み取り結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentRead {
  Found(RunContent),
  /// 対象が無い。レポートでは `reports/` の外を指す場合と `.md` の通常ファイルでない場合も含む。
  Missing,
  Unreadable(String),
}

/// レポートか指示書の本文。上限を超えた分は、読み取り層が文字の境界で切り詰めます。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunContent {
  pub content:    String,
  /// 本文全体の大きさ（バイト）。
  pub size_bytes: u64,
  /// 本文全体の大きさが上限を超え、切り詰めた。
  pub truncated:  bool,
}

/// 本文の先頭 `max_bytes` までを、文字の境界で切り詰めた本文にする。`size_bytes` は本文全体の
/// 大きさです。レポートと指示書の本文は、どちらもこの規則で切り詰めます。
fn bounded_content(mut bytes: Vec<u8>, size_bytes: u64, max_bytes: u64) -> RunContent {
  let limit = usize::try_from(max_bytes).unwrap_or(usize::MAX);
  bytes.truncate(limit);
  RunContent { content: utf8_prefix(bytes), size_bytes, truncated: size_bytes > max_bytes }
}

/// run の `reports/` の下にあるレポートの本文を、先頭から `max_bytes` まで読む。
///
/// `reports/` と対象を実パスへ正規化し、対象が `reports/` の下にある `.md` の通常ファイルの
/// ときだけ読みます。シンボリックリンクで外を指すものは無いものとして扱います。
pub fn read_report(run_dir: &Path, filename: &ReportFilename, max_bytes: u64) -> ContentRead {
  let Some(reports_dir) = contained_dir(run_dir, Path::new(REPORTS_DIRECTORY)) else {
    return ContentRead::Missing;
  };
  let path = match fs::canonicalize(reports_dir.join(filename.as_str())) {
    | Ok(path) => path,
    | Err(error) if error.kind() == io::ErrorKind::NotFound => return ContentRead::Missing,
    | Err(error) => return ContentRead::Unreadable(format!("レポートを解決できません: {error}")),
  };
  let is_report =
    path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.ends_with(REPORT_FILE_SUFFIX));
  if !path.starts_with(&reports_dir) || !is_report || !path.is_file() {
    return ContentRead::Missing;
  }
  let file = match File::open(&path) {
    | Ok(file) => file,
    | Err(error) => return ContentRead::Unreadable(format!("レポートを開けません: {error}")),
  };
  let size_bytes = match file.metadata() {
    | Ok(metadata) => metadata.len(),
    | Err(error) => return ContentRead::Unreadable(format!("レポートの大きさを読めません: {error}")),
  };
  let mut bytes = Vec::new();
  if let Err(error) = file.take(max_bytes).read_to_end(&mut bytes) {
    return ContentRead::Unreadable(format!("レポートを読めません: {error}"));
  }
  ContentRead::Found(bounded_content(bytes, size_bytes, max_bytes))
}

/// 切り詰めで途中に残った不完全な文字を落とし、残りを文字列にする。不正な UTF-8
/// は置換文字にします。
fn utf8_prefix(bytes: Vec<u8>) -> String {
  match String::from_utf8(bytes) {
    | Ok(text) => text,
    | Err(error) => {
      let utf8_error = error.utf8_error();
      let bytes = error.into_bytes();
      match utf8_error.error_len() {
        | None => String::from_utf8_lossy(&bytes[..utf8_error.valid_up_to()]).into_owned(),
        | Some(_) => String::from_utf8_lossy(&bytes).into_owned(),
      }
    },
  }
}

/// 指示書の状態。takt の live intervention の状態と同じ4種類です。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstructionState {
  Pending,
  DeliveredSameSession,
  DeliveredNextStep,
  UnconsumedWarned,
}

impl InstructionState {
  /// takt が使う状態の名前を返す。
  pub fn as_str(self) -> &'static str {
    match self {
      | InstructionState::Pending => "pending",
      | InstructionState::DeliveredSameSession => "deliveredSameSession",
      | InstructionState::DeliveredNextStep => "deliveredNextStep",
      | InstructionState::UnconsumedWarned => "unconsumedWarned",
    }
  }
}

/// run への1件の指示書。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instruction {
  pub instruction_id: InstructionId,
  pub issued_at:      DateTime<Utc>,
  pub state:          InstructionState,
  pub content:        String,
}

/// 指示書の読み取り結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstructionsRead {
  /// 記録が無い run は空の一覧です。
  Found(Vec<Instruction>),
  Unreadable(String),
}

/// プロジェクト側の `.takt/runs/<run_slug>/interventions.jsonl` を読み、takt と同じ規則で
/// イベント（issued / delivered / terminal）を指示書の状態へ畳み込む。
///
/// takt の LiveInterventionFileStore は worktree で動く run でもプロジェクトの cwd 側へ書くため、
/// worktree 側は読みません。ファイルが run のディレクトリの外を指す場合は読みません。
pub fn read_instructions(project_cwd: &Path, run_slug: &RunSlug) -> InstructionsRead {
  let run_dir = project_cwd.join(".takt").join("runs").join(run_slug.as_str());
  let path = match contained_path(&run_dir, Path::new(INTERVENTIONS_FILE)) {
    | Ok(Some(path)) => path,
    | Ok(None) => {
      return InstructionsRead::Unreadable(format!("{INTERVENTIONS_FILE} が run のディレクトリの外を指しています"))
    },
    | Err(error) if error.kind() == io::ErrorKind::NotFound => return InstructionsRead::Found(Vec::new()),
    | Err(error) => return InstructionsRead::Unreadable(format!("{INTERVENTIONS_FILE} を解決できません: {error}")),
  };
  let text = match fs::read_to_string(&path) {
    | Ok(text) => text,
    | Err(error) if error.kind() == io::ErrorKind::NotFound => return InstructionsRead::Found(Vec::new()),
    | Err(error) => return InstructionsRead::Unreadable(format!("{INTERVENTIONS_FILE} を読めません: {error}")),
  };
  match fold_interventions(&text) {
    | Ok(instructions) => InstructionsRead::Found(instructions),
    | Err(reason) => InstructionsRead::Unreadable(format!("{INTERVENTIONS_FILE} を解釈できません: {reason}")),
  }
}

/// 指定した ID の指示書の本文を、先頭から `max_bytes` まで返す。指示書の読み方は
/// [`read_instructions`] と同じです。
pub fn read_instruction_content(
  project_cwd: &Path,
  run_slug: &RunSlug,
  instruction_id: InstructionId,
  max_bytes: u64,
) -> ContentRead {
  match read_instructions(project_cwd, run_slug) {
    | InstructionsRead::Found(instructions) => {
      match instructions.into_iter().find(|instruction| instruction.instruction_id == instruction_id) {
        | Some(instruction) => {
          let bytes = instruction.content.into_bytes();
          let size_bytes = bytes.len() as u64;
          ContentRead::Found(bounded_content(bytes, size_bytes, max_bytes))
        },
        | None => ContentRead::Missing,
      }
    },
    | InstructionsRead::Unreadable(reason) => ContentRead::Unreadable(reason),
  }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum InterventionEvent {
  #[serde(rename_all = "camelCase")]
  Issued { instruction_id: i64, issued_at: String, content: String },
  #[serde(rename_all = "camelCase")]
  Delivered { instruction_ids: Vec<i64>, mode: String },
  #[serde(rename_all = "camelCase")]
  Terminal { unconsumed_instruction_ids: Vec<i64> },
}

fn fold_interventions(text: &str) -> Result<Vec<Instruction>, String> {
  let mut instructions: Vec<Instruction> = Vec::new();
  for (index, line) in text.lines().enumerate() {
    if line.trim().is_empty() {
      continue;
    }
    let event: InterventionEvent =
      serde_json::from_str(line).map_err(|error| format!("{} 行目: {error}", index + 1))?;
    match event {
      | InterventionEvent::Issued { instruction_id, issued_at, content } => {
        let instruction_id = InstructionId::try_from(instruction_id)
          .map_err(|error| format!("{} 行目の instructionId: {error}", index + 1))?;
        let issued_at = DateTime::parse_from_rfc3339(&issued_at)
          .map_err(|error| format!("{} 行目の issuedAt {issued_at}: {error}", index + 1))?
          .with_timezone(&Utc);
        if instructions.iter().any(|instruction| instruction.instruction_id == instruction_id) {
          return Err(format!("{} 行目の instructionId {instruction_id} が重複しています", index + 1));
        }
        instructions.push(Instruction { instruction_id, issued_at, state: InstructionState::Pending, content });
      },
      | InterventionEvent::Delivered { instruction_ids, mode } => {
        let state = match mode.as_str() {
          | "same_session" => InstructionState::DeliveredSameSession,
          | "next_step" | "batch_boundary" => InstructionState::DeliveredNextStep,
          | other => return Err(format!("{} 行目の mode {other} は未知の値です", index + 1)),
        };
        set_state(&mut instructions, &instruction_ids, state, index)?;
      },
      | InterventionEvent::Terminal { unconsumed_instruction_ids } => {
        set_state(&mut instructions, &unconsumed_instruction_ids, InstructionState::UnconsumedWarned, index)?;
      },
    }
  }
  Ok(instructions)
}

fn set_state(
  instructions: &mut [Instruction],
  instruction_ids: &[i64],
  state: InstructionState,
  index: usize,
) -> Result<(), String> {
  for instruction_id in instruction_ids {
    let instruction = instructions
      .iter_mut()
      .find(|instruction| i64::from(instruction.instruction_id.get()) == *instruction_id)
      .ok_or_else(|| format!("{} 行目の instructionId {instruction_id} は発行されていません", index + 1))?;
    instruction.state = state;
  }
  Ok(())
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
  workflow:          Option<String>,
  #[serde(rename = "startTime")]
  start_time:        Option<String>,
  status:            Option<String>,
  #[serde(rename = "currentStep")]
  current_step:      Option<String>,
  #[serde(rename = "currentIteration")]
  current_iteration: Option<i64>,
  phase:             Option<i64>,
  #[serde(rename = "updatedAt")]
  updated_at:        Option<String>,
  task:              Option<String>,
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

fn collect_reports(directory: &Path, prefix: &str, reports: &mut Vec<ReportFile>) {
  let Ok(entries) = fs::read_dir(directory) else {
    return;
  };
  for entry in entries.flatten() {
    let Ok(file_type) = entry.file_type() else {
      continue;
    };
    let name = entry.file_name().to_string_lossy().into_owned();
    let is_report = name.ends_with(REPORT_FILE_SUFFIX);
    let relative = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
    if file_type.is_dir() {
      collect_reports(&entry.path(), &relative, reports);
    } else if file_type.is_file() && is_report {
      let Ok(metadata) = entry.metadata() else {
        continue;
      };
      reports.push(ReportFile { filename: relative, size_bytes: metadata.len() });
    }
  }
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

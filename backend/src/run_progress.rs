//! run の保存場所（[`crate::run_log`]）から読んだ記録を、run ごとに集計して保持する。
//!
//! 定期読み取りと run 詳細は、同じ run を何度も読みます。本体ログは読み取り層が前回の続きだけを
//! 返すため、step の記録と現在ステップの開始はこのモジュールの集計に積み上げ、読むたびに
//! 先頭から数え直しません。集計の規則は takt_get_run（takt の `readSessionPage`）と同じです。
//!
//! 読み取り層は同期の関数なので、非同期の文脈からは [`read_run`] などを通して `spawn_blocking`
//! の中で呼びます。
//!
//! 定期読み取りの取っ手は本体ログを先頭から読みます。run 詳細の取っ手は
//! [`InitialRead::Tail`] で末尾の一定量から読み始めます。そのため、本体ログが上限より大きい run を
//! 初めて開いたときは、ステップの記録と現在ステップの実行回数が末尾の範囲に含まれる分だけに
//! なります。読み位置を持った後は、どちらの取っ手も追記分だけを読みます。
//!
//! run 詳細の追記の配信（[`read_run_updates`]）のため、ログの行には通し番号を付け、行を初期化する
//! たびに世代（epoch）を変えます。取っ手はプロジェクトで1つを共有するため、読み位置ではなく、
//! 利用者が送る世代と番号で「前回の続き」を選びます。

use std::{
  borrow::Borrow,
  collections::{BTreeMap, VecDeque},
  path::{Path, PathBuf},
  sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex, OnceLock,
  },
  time::{SystemTime, UNIX_EPOCH},
};

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::{
  domain::{InstructionId, ReportFilename, RunSlug},
  model::{ReportEntry, RunEntry, RunStatus, RunStepEntry, StepLogEntry, TaskEntry},
  run_log::{
    self, ContentRead, InitialRead, Instruction, InstructionsRead, MetaRead, RunLocation, RunLogReader, SessionActivity,
  },
};

/// takt が既定で worktree を置く、プロジェクトの親ディレクトリの下の名前。
const WORKTREES_DIRECTORY: &str = "takt-worktrees";
/// 1プロジェクトで集計を保持する run の上限。takt の並列度（サーバーあたり数件）に対して十分に
/// 大きく、run 詳細を開いた run が増え続けても保持量が上限で止まるようにします。
const MAX_TRACKED_RUNS: usize = 32;
/// 一覧の run の要約に使う、タスクの本文の1行目の文字数の上限（takt と同じ）。
const TASK_SUMMARY_CHARS: usize = 80;
/// ログの末尾として保持する記録の件数。
const LOG_TAIL_RECORDS: usize = 20;
/// run 詳細の追記の配信のために、番号付きで保持する行の上限。画面が保持する行の上限と同じです。
/// 利用者の位置がこれより古い行を指すときは、続きを示せないため全体を置き換えます。
const LOG_UPDATE_BUFFER_RECORDS: usize = 200;

/// 世代の初期値はプロセスの開始時刻（ミリ秒）から作り、backend を起動し直しても前のプロセスの
/// 世代と重ならないようにします。
static EPOCH_BASE: OnceLock<u64> = OnceLock::new();
static NEXT_EPOCH: AtomicU64 = AtomicU64::new(0);

/// プロセスの中で重ならない新しい世代を返す。
fn fresh_epoch() -> u64 {
  let base = *EPOCH_BASE.get_or_init(|| {
    SystemTime::now()
      .duration_since(UNIX_EPOCH)
      .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
      .unwrap_or_default()
  });
  base.saturating_add(NEXT_EPOCH.fetch_add(1, Ordering::Relaxed))
}

/// 利用者が前回受け取った行の位置（世代と最後の行の番号）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogCursor {
  pub epoch:    u64,
  pub sequence: u64,
}

/// 番号付きのログの1行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
  pub sequence: u64,
  pub text:     String,
}

/// run 詳細の追記分。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunUpdates {
  /// 行の番号の世代。
  pub epoch:    u64,
  /// 真なら `lines` で全体を置き換え、偽なら前回の続きとして足す。
  pub reset:    bool,
  pub lines:    Vec<LogLine>,
  pub activity: SessionActivity,
}

/// run を保存場所から読めなかった理由。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RunReadFailure {
  #[error("run は tasks.yaml の対応表にありません")]
  NotListed,
  #[error("run の保存場所が残っていません")]
  Missing,
  #[error("run を読めませんでした: {0}")]
  Unreadable(String),
  #[error("run の保存場所が許可された置き場の外にあります: {0}")]
  Rejected(String),
  #[error("run の meta.json が不正です: {0}")]
  InvalidMeta(String),
}

/// 1回の読み取りで得た run の状態。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunLogView {
  pub status: RunStatus,
  pub workflow: Option<String>,
  pub current_step: Option<String>,
  pub current_iteration: Option<i32>,
  pub phase: Option<i32>,
  pub start_time: Option<DateTime<Utc>>,
  pub current_step_started_at: Option<DateTime<Utc>>,
  pub current_step_execution_count: Option<i32>,
  /// `meta.json` の task（タスクの本文）。
  pub task_text: Option<String>,
  /// この読み取りでディスクから読んだ本体ログのバイト数。
  pub body_log_bytes_read: u64,
  completions: Vec<StepCompletion>,
  latest_log_update: Option<DateTime<Utc>>,
  reports: Vec<ReportEntry>,
  log_tail: Vec<String>,
}

impl RunLogView {
  /// 一覧の run を作る。ステップ位置は workflow の一覧を持つ呼び出し側が補います。
  pub fn run_entry(&self, run_slug: &RunSlug, task: &TaskEntry) -> RunEntry {
    let started_at = self.started_at(Some(task));
    RunEntry {
      run_slug: run_slug.as_str().to_owned(),
      session_id: String::new(),
      task_name: task.name.clone(),
      goal_id: task.goal_id.clone(),
      summary: self.task_text.as_deref().map(task_summary).unwrap_or_default(),
      status: self.status,
      current_step: self.current_step.clone(),
      current_step_number: None,
      total_steps: None,
      current_iteration: self.current_iteration,
      started_at,
      current_step_started_at: self.current_step_started_at,
      last_log_at: self.resolve_last_log_at(started_at),
      steps: self
        .completions
        .iter()
        .map(|completion| RunStepEntry {
          step_name:  completion.step.clone(),
          model:      completion.model.clone().unwrap_or_default(),
          started_at: completion.timestamp.unwrap_or(DateTime::<Utc>::UNIX_EPOCH),
          ended_at:   None,
        })
        .collect(),
      reports: self.reports.clone(),
    }
  }

  /// run 詳細のステップの記録。takt_get_run の summary と同じく本文は持ちません。
  pub fn step_logs(&self) -> Vec<StepLogEntry> {
    self
      .completions
      .iter()
      .map(|completion| StepLogEntry {
        step_name: completion.step.clone(),
        persona:   completion.persona.clone(),
        status:    completion.status.clone(),
        content:   String::new(),
        timestamp: completion.timestamp.unwrap_or(DateTime::<Utc>::UNIX_EPOCH),
        iteration: completion.iteration,
      })
      .collect()
  }

  /// `reports/` の下のレポートの一覧（名前と大きさ）。
  pub fn reports(&self) -> &[ReportEntry] {
    &self.reports
  }

  /// 本体ログの末尾の記録を「時刻 種類 ステップ」の行にして、古い順に改行で連結したもの。
  pub fn log_tail(&self) -> String {
    self.log_tail.join("\n")
  }

  /// run の開始時刻を決める。meta.json の startTime を使い、無ければタスクの開始時刻を使います。
  /// 一覧と詳細はこの1か所を通り、開始時刻とそれに落ちる最終ログ時刻が食い違わないようにします。
  pub fn started_at(&self, task: Option<&TaskEntry>) -> Option<DateTime<Utc>> {
    self.start_time.or_else(|| task.and_then(|task| task.started_at))
  }

  /// 最終ログ時刻を決める。一覧と詳細はこの1か所を通り、無音の判定が食い違わないようにします。
  ///
  /// 時刻を持つ step_complete の最新、現在ステップの開始、ログ群の更新時刻のうち最も新しいものを
  /// 採ります。どれも無ければ run の開始時刻です。時刻を持たない step_complete は候補にしません。
  pub fn resolve_last_log_at(&self, started_at: Option<DateTime<Utc>>) -> Option<DateTime<Utc>> {
    self
      .completions
      .iter()
      .filter_map(|completion| completion.timestamp)
      .chain(self.current_step_started_at)
      .chain(self.latest_log_update)
      .max()
      .or(started_at)
  }
}

/// takt のタスク要約と同じ規則（前後の空白を除き、1行目の先頭 80 文字）で、タスクの本文を縮める。
///
/// takt は UTF-16 の単位で 80 を数えますが、ここでは文字（Unicode のスカラー値）で数えます。
/// 差が出るのは BMP の外の文字（絵文字など）を含むときだけです。1 行目が 80 文字を超えて切った
/// ときだけ、末尾に省略記号を付けます（issue #230）。1 行目だけにするのは要約で、印は付けません。
pub(crate) fn task_summary(task_text: &str) -> String {
  crate::model::truncate_chars(task_text.trim().lines().next().unwrap_or_default(), TASK_SUMMARY_CHARS)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StepCompletion {
  step:      String,
  persona:   String,
  status:    String,
  timestamp: Option<DateTime<Utc>>,
  iteration: i32,
  /// そのステップを始めた step_start の model。
  model:     Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StepStart {
  step:            Option<String>,
  workflow:        Option<String>,
  iteration:       Option<i64>,
  stack_depth:     Option<usize>,
  stack_root_step: Option<String>,
  /// parallel が無いか、parallel の親（`role == "parent"`）か。
  top_level:       bool,
  timestamp:       Option<DateTime<Utc>>,
  model:           Option<String>,
}

/// 1つの run の集計。本体ログの読み取り位置もここに持ちます。
#[derive(Debug)]
struct RunProgress {
  reader:        RunLogReader,
  /// 読み位置を持たない本体ログの読み始め。末尾から読む集計は、読み直しのときに自分で初期化します。
  initial_read:  InitialRead,
  /// 最初の `workflow_start` を読むまでは記録を数えない（takt と同じ）。
  active:        bool,
  completions:   Vec<StepCompletion>,
  starts:        Vec<StepStart>,
  log_tail:      VecDeque<String>,
  /// 番号付きの行。`log_tail` と同じ箇所で足し、同じ箇所で初期化します。
  lines:         VecDeque<LogLine>,
  /// `lines` の番号の世代。`lines` を初期化するたびに変えます。
  epoch:         u64,
  /// 次に足す行の番号。
  next_sequence: u64,
  last_read:     u64,
}

impl RunProgress {
  fn new(project_cwd: PathBuf, worktrees_root: PathBuf, initial_read: InitialRead) -> Self {
    RunProgress {
      reader: RunLogReader::with_initial_read(project_cwd, worktrees_root, initial_read),
      initial_read,
      active: false,
      completions: Vec::new(),
      starts: Vec::new(),
      log_tail: VecDeque::new(),
      lines: VecDeque::new(),
      epoch: fresh_epoch(),
      next_sequence: 1,
      last_read: 0,
    }
  }

  fn resolve(&self, run_slug: &RunSlug) -> Result<PathBuf, RunReadFailure> {
    match self.reader.resolve(run_slug) {
      | RunLocation::Found(run_dir) => Ok(run_dir),
      | RunLocation::Missing => Err(RunReadFailure::Missing),
      | RunLocation::NotListed => Err(RunReadFailure::NotListed),
      | RunLocation::Unreadable(reason) => Err(RunReadFailure::Unreadable(reason)),
      | RunLocation::Rejected(reason) => Err(RunReadFailure::Rejected(reason)),
    }
  }

  fn read(&mut self, run_slug: &RunSlug) -> Result<RunLogView, RunReadFailure> {
    let run_dir = self.resolve(run_slug)?;
    let meta = match self.reader.read_meta(&run_dir) {
      | MetaRead::Found(meta) => meta,
      | MetaRead::Missing => return Err(RunReadFailure::Missing),
      | MetaRead::Unreadable(reason) => return Err(RunReadFailure::Unreadable(reason)),
    };
    let status =
      meta.status.as_deref().ok_or_else(|| RunReadFailure::InvalidMeta("status がありません".to_owned())).and_then(
        |status| {
          RunStatus::try_from(status).map_err(|error| RunReadFailure::InvalidMeta(format!("status {status}: {error}")))
        },
      )?;
    let start_time = meta
      .start_time
      .as_deref()
      .map(|text| {
        DateTime::parse_from_rfc3339(text)
          .map(|time| time.with_timezone(&Utc))
          .map_err(|error| RunReadFailure::InvalidMeta(format!("startTime {text}: {error}")))
      })
      .transpose()?;
    let current_iteration = meta.current_iteration.map(|value| meta_i32(value, "currentIteration")).transpose()?;
    let phase = meta.phase.map(|value| meta_i32(value, "phase")).transpose()?;

    let body_log_bytes_read = self.ingest_body_log(&run_dir);
    let (current_step_started_at, current_step_execution_count) =
      self.current_step_start(meta.current_step.as_deref(), meta.current_iteration, meta.workflow.as_deref());
    Ok(RunLogView {
      status,
      workflow: meta.workflow,
      current_step: meta.current_step,
      current_iteration,
      phase,
      start_time,
      current_step_started_at,
      current_step_execution_count,
      task_text: meta.task,
      body_log_bytes_read,
      completions: self.completions.clone(),
      latest_log_update: self.reader.latest_log_update(&run_dir),
      reports: self
        .reader
        .list_reports(&run_dir)
        .into_iter()
        .map(|report| ReportEntry {
          filename:   report.filename,
          // ファイルの大きさが i64 を超えることは無いが、u64 からの変換を明示するため上限で止める。
          size_bytes: i64::try_from(report.size_bytes).unwrap_or(i64::MAX),
        })
        .collect(),
      log_tail: self.log_tail.iter().cloned().collect(),
    })
  }

  /// 本体ログの追記分を集計へ取り込み、読んだバイト数を返す。
  fn ingest_body_log(&mut self, run_dir: &Path) -> u64 {
    // 読み取り層はファイルの縮小や置き換えで読み位置を捨て、読み直す。先頭から読む集計は、先頭の
    // workflow_start で初期化される。末尾から読む集計は workflow_start を読まないことがあるため、
    // 読み直したときは記録を取り込む前に自分で初期化する。
    //
    // 本体ログが複数ある run で、どれかのファイルを読み位置なしで読んだ（新しく現れた、縮んだ、
    // 置き換わった）ときは、集計全体を初期化する。ほかのファイルは読み位置が末尾にあり、そのまま
    // では集計に戻らないため、全ファイルの読み位置を捨てて同じ呼び出しの中で読み直す。こうして
    // 集計は、新しく作った取っ手が読む結果と同じになる。
    let tail = matches!(self.initial_read, InitialRead::Tail(_));
    let had_positions = self.reader.has_body_log_positions();
    let mut body_log = self.reader.read_body_log(run_dir);
    if tail && body_log.restarted && had_positions {
      self.reader.forget_body_log_positions();
      let first_bytes = body_log.bytes_read;
      body_log = self.reader.read_body_log(run_dir);
      body_log.bytes_read += first_bytes;
    }
    if tail && body_log.restarted {
      self.active = false;
      self.completions.clear();
      self.starts.clear();
      self.clear_log_lines();
    }
    // 末尾から読み始めた場合は、範囲より前にある workflow_start を読めないため、ここから数える。
    if body_log.skipped_head {
      self.active = true;
    }
    for record in &body_log.records {
      self.ingest(record);
    }
    body_log.bytes_read
  }

  /// 本体ログの追記分を取り込み、`after` の続きの行とセッションログの活動の印を返す。
  ///
  /// `after` が無い、世代が違う、番号がこの世代の範囲に無い、または保持している行より古い行を
  /// 指すときは、続きを示せないため `reset` を立て、`log_tail` と同じ末尾の行を返します。
  fn read_updates(&mut self, run_slug: &RunSlug, after: Option<LogCursor>) -> Result<RunUpdates, RunReadFailure> {
    let run_dir = self.resolve(run_slug)?;
    self.ingest_body_log(&run_dir);
    let activity = self.reader.read_session_activity(&run_dir);
    let continued = after.filter(|cursor| self.continues_from(*cursor));
    let lines = match continued {
      | Some(cursor) => self.lines.iter().filter(|line| line.sequence > cursor.sequence).cloned().collect(),
      | None => self.lines.iter().skip(self.lines.len().saturating_sub(LOG_TAIL_RECORDS)).cloned().collect(),
    };
    Ok(RunUpdates { epoch: self.epoch, reset: continued.is_none(), lines, activity })
  }

  /// `cursor` の後の行を、取りこぼしなく今の行から選べるか。
  fn continues_from(&self, cursor: LogCursor) -> bool {
    let latest = self.next_sequence - 1;
    let oldest_kept = self.lines.front().map_or(self.next_sequence, |line| line.sequence);
    cursor.epoch == self.epoch && cursor.sequence <= latest && cursor.sequence + 1 >= oldest_kept
  }

  /// ログの行を初期化する。番号付きの行も同時に捨て、世代を変える。
  fn clear_log_lines(&mut self) {
    self.log_tail.clear();
    self.lines.clear();
    self.epoch = fresh_epoch();
  }

  fn ingest(&mut self, record: &Value) {
    if record.get("type").and_then(Value::as_str) == Some("workflow_start") {
      self.clear_log_lines();
    }
    self.push_log_tail(record);
    match record.get("type").and_then(Value::as_str) {
      | Some("workflow_start") => {
        self.active = true;
        self.completions.clear();
        self.starts.clear();
      },
      | Some("step_start") if self.active => self.starts.push(StepStart::from_record(record)),
      | Some("step_complete") if self.active => {
        let mut completion = StepCompletion::from_record(record);
        completion.model = self.start_model(&completion.step, record.get("iteration").and_then(Value::as_i64));
        self.completions.push(completion);
      },
      | _ => {},
    }
  }

  /// step_complete より前で最も新しい、同じ step（両方が iteration を持てば同じ iteration）の
  /// step_start の model。
  fn start_model(&self, step: &str, iteration: Option<i64>) -> Option<String> {
    self
      .starts
      .iter()
      .rev()
      .find(|start| {
        start.step.as_deref() == Some(step)
          && start.iteration.zip(iteration).is_none_or(|(started, completed)| started == completed)
      })
      .and_then(|start| start.model.clone())
  }

  /// 記録を「時刻 種類 ステップ」の1行にしてログの末尾へ足す。無い部分は省き、種類の無い記録は
  /// 足しません。
  fn push_log_tail(&mut self, record: &Value) {
    let Some(kind) = record.get("type").and_then(Value::as_str) else {
      return;
    };
    let line =
      [record.get("timestamp").and_then(Value::as_str), Some(kind), record.get("step").and_then(Value::as_str)]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    if self.log_tail.len() == LOG_TAIL_RECORDS {
      self.log_tail.pop_front();
    }
    self.log_tail.push_back(line.clone());
    if self.lines.len() == LOG_UPDATE_BUFFER_RECORDS {
      self.lines.pop_front();
    }
    self.lines.push_back(LogLine { sequence: self.next_sequence, text: line });
    self.next_sequence += 1;
  }

  /// 現在ステップの開始時刻と実行回数を、takt_get_run と同じ規則で求める。
  ///
  /// 最上位（stack の深さ 1 以下、parallel の子でない）の同名・同 workflow の step_start を
  /// 数え、開始時刻はそのうち現在の iteration のものです。該当が無く、最後の step_start が
  /// 入れ子の中で外側の step が現在ステップなら、その step_start を使います。
  fn current_step_start(
    &self,
    current_step: Option<&str>,
    current_iteration: Option<i64>,
    workflow: Option<&str>,
  ) -> (Option<DateTime<Utc>>, Option<i32>) {
    let Some(current_step) = current_step else {
      return (None, None);
    };
    if self.starts.is_empty() {
      return (None, None);
    }
    let mut count = 0;
    let mut started_at = None;
    for start in &self.starts {
      let matches = start.step.as_deref() == Some(current_step)
        && start.workflow.as_deref().is_none_or(|start_workflow| Some(start_workflow) == workflow)
        && start.stack_depth.is_none_or(|depth| depth <= 1)
        && start.top_level;
      if matches {
        count += 1;
        if start.iteration == current_iteration {
          started_at = start.timestamp;
        }
      }
    }
    if started_at.is_none() {
      let leaf = self.starts.iter().rev().find(|start| start.top_level);
      if let Some(leaf) = leaf.filter(|leaf| {
        leaf.stack_depth.is_some_and(|depth| depth > 1) && leaf.stack_root_step.as_deref() == Some(current_step)
      }) {
        started_at = leaf.timestamp;
        count = self.starts.iter().filter(|start| start.step == leaf.step && start.workflow == leaf.workflow).count();
      }
    }
    (started_at, Some(i32::try_from(count).unwrap_or(i32::MAX)))
  }
}

fn meta_i32(value: i64, key: &str) -> Result<i32, RunReadFailure> {
  i32::try_from(value).map_err(|_| RunReadFailure::InvalidMeta(format!("{key} が i32 の範囲外です: {value}")))
}

/// 本体ログの時刻を読む。本体ログは takt が書く外部の記録なので、読めない時刻はその記録の
/// 時刻が無いものとして扱い、run 全体の読み取りは止めません。
fn record_time(record: &Value) -> Option<DateTime<Utc>> {
  let text = record.get("timestamp").and_then(Value::as_str)?;
  DateTime::parse_from_rfc3339(text).ok().map(|time| time.with_timezone(&Utc))
}

fn record_string(record: &Value, key: &str) -> Option<String> {
  record.get(key).and_then(Value::as_str).map(str::to_owned)
}

impl StepCompletion {
  fn from_record(record: &Value) -> Self {
    StepCompletion {
      step:      record_string(record, "step").unwrap_or_default(),
      persona:   record_string(record, "persona").unwrap_or_default(),
      status:    record_string(record, "status").unwrap_or_default(),
      timestamp: record_time(record),
      model:     None,
      iteration: record
        .get("iteration")
        .and_then(Value::as_i64)
        .and_then(|iteration| i32::try_from(iteration).ok())
        .unwrap_or_default(),
    }
  }
}

impl StepStart {
  fn from_record(record: &Value) -> Self {
    let stack = record.get("stack").and_then(Value::as_array);
    let top_level = match record.get("parallel") {
      | None | Some(Value::Null) => true,
      | Some(parallel) => parallel.get("role").and_then(Value::as_str) == Some("parent"),
    };
    StepStart {
      step: record_string(record, "step"),
      workflow: record_string(record, "workflow"),
      iteration: record.get("iteration").and_then(Value::as_i64),
      stack_depth: stack.map(Vec::len),
      stack_root_step: stack.and_then(|stack| stack.first()).and_then(|root| record_string(root, "step")),
      top_level,
      timestamp: record_time(record),
      model: record_string(record, "model"),
    }
  }
}

/// 1プロジェクトの run の集計。プロジェクトの cwd と、run ごとの集計を持ちます。
#[derive(Debug)]
pub struct ProjectRunLogs {
  project_cwd:    PathBuf,
  worktrees_root: PathBuf,
  initial_read:   InitialRead,
  runs:           BTreeMap<RunSlug, RunProgress>,
  read_sequence:  u64,
}

/// takt が既定で worktree を置く場所（`<cwd>/../takt-worktrees`）。
fn worktrees_root(project_cwd: &Path) -> PathBuf {
  project_cwd.join("..").join(WORKTREES_DIRECTORY)
}

impl ProjectRunLogs {
  /// プロジェクトの cwd から、本体ログを先頭から読む集計を作る。worktree の置き場は takt
  /// の既定（`<cwd>/../takt-worktrees`）です。
  pub fn new(project_cwd: PathBuf) -> Self {
    Self::with_initial_read(project_cwd, InitialRead::Start)
  }

  /// 読み位置を持たない本体ログの読み始めを指定して集計を作る。
  pub fn with_initial_read(project_cwd: PathBuf, initial_read: InitialRead) -> Self {
    let worktrees_root = worktrees_root(&project_cwd);
    ProjectRunLogs { project_cwd, worktrees_root, initial_read, runs: BTreeMap::new(), read_sequence: 0 }
  }

  /// run を読み、前回の続きを集計へ足して現在の状態を返す。
  pub fn read(&mut self, run_slug: &RunSlug) -> Result<RunLogView, RunReadFailure> {
    self.with_progress(run_slug, |progress| progress.read(run_slug))
  }

  /// run の追記分を読み、`after` の続きの行と活動の印を返す。集計の取り出し方と追い出しは
  /// [`ProjectRunLogs::read`] と同じです。
  pub fn read_updates(&mut self, run_slug: &RunSlug, after: Option<LogCursor>) -> Result<RunUpdates, RunReadFailure> {
    self.with_progress(run_slug, |progress| progress.read_updates(run_slug, after))
  }

  /// run の集計を取り出すか作り、最後に読んだ順を記録して `read` を呼ぶ。呼んだ後に、保持する
  /// run の上限を超えた分を、最後に読んだのが古い順に捨てる。
  fn with_progress<T>(&mut self, run_slug: &RunSlug, read: impl FnOnce(&mut RunProgress) -> T) -> T {
    self.read_sequence += 1;
    let sequence = self.read_sequence;
    let progress = self
      .runs
      .entry(run_slug.clone())
      .or_insert_with(|| RunProgress::new(self.project_cwd.clone(), self.worktrees_root.clone(), self.initial_read));
    progress.last_read = sequence;
    let result = read(progress);
    self.evict_least_recently_read();
    result
  }

  /// 指定した run 以外の集計を捨てる。
  pub fn retain(&mut self, run_slugs: &[RunSlug]) {
    self.runs.retain(|run_slug, _| run_slugs.contains(run_slug));
  }

  fn evict_least_recently_read(&mut self) {
    while self.runs.len() > MAX_TRACKED_RUNS {
      let Some(oldest) =
        self.runs.iter().min_by_key(|(_, progress)| progress.last_read).map(|(run_slug, _)| run_slug.clone())
      else {
        return;
      };
      self.runs.remove(&oldest);
    }
  }
}

/// 非同期の処理から共有する、1プロジェクトの集計の取っ手。
pub type RunLogHandle = Arc<Mutex<ProjectRunLogs>>;

/// プロジェクトの cwd から取っ手を作る。
pub fn run_log_handle(project_cwd: PathBuf) -> RunLogHandle {
  Arc::new(Mutex::new(ProjectRunLogs::new(project_cwd)))
}

/// プロジェクトごとの集計の取っ手と、それぞれを作ったときの cwd。
///
/// 取っ手は作ったときの cwd の保存場所だけを読むため、cwd が変わったプロジェクトは作り直します。
/// Poller と GetRunDetail のサービスは、それぞれ自分の表を持ちます。既定の表は本体ログを先頭から
/// 読み、run 詳細の表は [`RunLogHandles::with_initial_read`] で末尾から読みます。
#[derive(Debug)]
pub struct RunLogHandles<K> {
  handles:      BTreeMap<K, (PathBuf, RunLogHandle)>,
  initial_read: InitialRead,
}

impl<K> Default for RunLogHandles<K> {
  fn default() -> Self {
    Self::with_initial_read(InitialRead::Start)
  }
}

impl<K> RunLogHandles<K> {
  /// 作る取っ手が、読み位置を持たない本体ログを `initial_read` に従って読む表を作る。
  pub fn with_initial_read(initial_read: InitialRead) -> Self {
    RunLogHandles { handles: BTreeMap::new(), initial_read }
  }
}

impl<K: Ord + Clone> RunLogHandles<K> {
  /// プロジェクトの取っ手を返す。初めてのプロジェクトと、cwd が変わったプロジェクトは作り直します。
  pub fn handle_for(&mut self, key: &K, cwd: &Path) -> RunLogHandle {
    match self.handles.get(key) {
      | Some((known_cwd, handle)) if known_cwd == cwd => handle.clone(),
      | _ => {
        let handle = Arc::new(Mutex::new(ProjectRunLogs::with_initial_read(cwd.to_path_buf(), self.initial_read)));
        self.handles.insert(key.clone(), (cwd.to_path_buf(), handle.clone()));
        handle
      },
    }
  }

  /// プロジェクトの取っ手を捨てる。
  pub fn remove<Q>(&mut self, key: &Q)
  where
    K: Borrow<Q>,
    Q: Ord + ?Sized, {
    self.handles.remove(key);
  }

  /// `keep` が真のプロジェクトの取っ手だけを残す。
  pub fn retain(&mut self, mut keep: impl FnMut(&K) -> bool) {
    self.handles.retain(|key, _| keep(key));
  }
}

/// 読み取り層を別スレッドで呼び、run を読む。
pub async fn read_run(handle: &RunLogHandle, run_slug: RunSlug) -> Result<RunLogView, RunReadFailure> {
  let handle = handle.clone();
  tokio::task::spawn_blocking(move || {
    let mut logs =
      handle.lock().map_err(|error| RunReadFailure::Unreadable(format!("集計を取得できません: {error}")))?;
    logs.read(&run_slug)
  })
  .await
  .map_err(|error| RunReadFailure::Unreadable(format!("読み取りのスレッドが終了しました: {error}")))?
}

/// 読み取り層を別スレッドで呼び、run 詳細の追記分を読む。
pub async fn read_run_updates(
  handle: &RunLogHandle,
  run_slug: RunSlug,
  after: Option<LogCursor>,
) -> Result<RunUpdates, RunReadFailure> {
  let handle = handle.clone();
  tokio::task::spawn_blocking(move || {
    let mut logs =
      handle.lock().map_err(|error| RunReadFailure::Unreadable(format!("集計を取得できません: {error}")))?;
    logs.read_updates(&run_slug, after)
  })
  .await
  .map_err(|error| RunReadFailure::Unreadable(format!("読み取りのスレッドが終了しました: {error}")))?
}

/// 別スレッドで、指定した run 以外の集計を捨てる。
pub async fn retain_runs(handle: &RunLogHandle, run_slugs: Vec<RunSlug>) {
  let handle = handle.clone();
  let result = tokio::task::spawn_blocking(move || {
    let mut logs = handle.lock().map_err(|error| format!("集計を取得できません: {error}"))?;
    logs.retain(&run_slugs);
    Ok::<(), String>(())
  })
  .await
  .map_err(|error| format!("整理のスレッドが終了しました: {error}"))
  .and_then(|result| result);
  if let Err(error) = result {
    tracing::warn!(error = %error, "run の集計の整理を完了できませんでした。");
  }
}

/// 別スレッドで run の場所を解決し、`reports/` の下のレポートの本文を先頭から `max_bytes`
/// まで読む。
pub async fn read_report_content(
  project_cwd: PathBuf,
  run_slug: RunSlug,
  filename: ReportFilename,
  max_bytes: u64,
) -> Result<ContentRead, RunReadFailure> {
  tokio::task::spawn_blocking(move || {
    let reader = RunLogReader::new(project_cwd.clone(), worktrees_root(&project_cwd));
    match reader.resolve(&run_slug) {
      | RunLocation::Found(run_dir) => Ok(run_log::read_report(&run_dir, &filename, max_bytes)),
      | RunLocation::Missing => Err(RunReadFailure::Missing),
      | RunLocation::NotListed => Err(RunReadFailure::NotListed),
      | RunLocation::Unreadable(reason) => Err(RunReadFailure::Unreadable(reason)),
      | RunLocation::Rejected(reason) => Err(RunReadFailure::Rejected(reason)),
    }
  })
  .await
  .map_err(|error| RunReadFailure::Unreadable(format!("読み取りのスレッドが終了しました: {error}")))?
}

/// 別スレッドで、プロジェクト側の `interventions.jsonl` から run の指示書を読む。
pub async fn read_run_instructions(
  project_cwd: PathBuf,
  run_slug: RunSlug,
) -> Result<Vec<Instruction>, RunReadFailure> {
  tokio::task::spawn_blocking(move || match run_log::read_instructions(&project_cwd, &run_slug) {
    | InstructionsRead::Found(instructions) => Ok(instructions),
    | InstructionsRead::Unreadable(reason) => Err(RunReadFailure::Unreadable(reason)),
  })
  .await
  .map_err(|error| RunReadFailure::Unreadable(format!("読み取りのスレッドが終了しました: {error}")))?
}

/// 別スレッドで、プロジェクト側の `interventions.jsonl` から指定した ID の指示書の本文を、先頭から
/// `max_bytes` まで読む。
///
/// レポートの読み取り（[`read_report_content`]）と同じく、run の保存場所を解決できない
/// （対応表に無い・対応表を読めない）場合は失敗として返し、呼び出し側が takt_get_run へ
/// 切り替えられるようにします。
pub async fn read_instruction_body(
  project_cwd: PathBuf,
  run_slug: RunSlug,
  instruction_id: InstructionId,
  max_bytes: u64,
) -> Result<ContentRead, RunReadFailure> {
  tokio::task::spawn_blocking(move || {
    let reader = RunLogReader::new(project_cwd.clone(), worktrees_root(&project_cwd));
    match reader.resolve(&run_slug) {
      | RunLocation::NotListed => Err(RunReadFailure::NotListed),
      | RunLocation::Unreadable(reason) => Err(RunReadFailure::Unreadable(reason)),
      | _ => Ok(run_log::read_instruction_content(&project_cwd, &run_slug, instruction_id, max_bytes)),
    }
  })
  .await
  .map_err(|error| RunReadFailure::Unreadable(format!("読み取りのスレッドが終了しました: {error}")))?
}

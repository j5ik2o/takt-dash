//! run の保存場所（[`crate::run_log`]）から読んだ記録を、run ごとに集計して保持する。
//!
//! 定期読み取りと run 詳細は、同じ run を何度も読みます。本体ログは読み取り層が前回の続きだけを
//! 返すため、step の記録と現在ステップの開始はこのモジュールの集計に積み上げ、読むたびに
//! 先頭から数え直しません。集計の規則は takt_get_run（takt の `readSessionPage`）と同じです。
//!
//! 読み取り層は同期の関数なので、非同期の文脈からは [`read_run`] を通して `spawn_blocking`
//! の中で呼びます。

use std::{
  borrow::Borrow,
  collections::BTreeMap,
  path::{Path, PathBuf},
  sync::{Arc, Mutex},
};

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::{
  domain::RunSlug,
  model::{ReportEntry, RunEntry, RunStatus, RunStepEntry, StepLogEntry, TaskEntry},
  run_log::{MetaRead, RunLocation, RunLogReader},
};

/// takt が既定で worktree を置く、プロジェクトの親ディレクトリの下の名前。
const WORKTREES_DIRECTORY: &str = "takt-worktrees";
/// 1プロジェクトで集計を保持する run の上限。takt の並列度（サーバーあたり数件）に対して十分に
/// 大きく、run 詳細を開いた run が増え続けても保持量が上限で止まるようにします。
const MAX_TRACKED_RUNS: usize = 32;

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
  completions: Vec<StepCompletion>,
  latest_log_update: Option<DateTime<Utc>>,
  reports: Vec<ReportEntry>,
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
      summary: String::new(),
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
          model:      String::new(),
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct StepCompletion {
  step:      String,
  persona:   String,
  status:    String,
  timestamp: Option<DateTime<Utc>>,
  iteration: i32,
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
}

/// 1つの run の集計。本体ログの読み取り位置もここに持ちます。
#[derive(Debug)]
struct RunProgress {
  reader:      RunLogReader,
  /// 最初の `workflow_start` を読むまでは記録を数えない（takt と同じ）。
  active:      bool,
  completions: Vec<StepCompletion>,
  starts:      Vec<StepStart>,
  last_read:   u64,
}

impl RunProgress {
  fn new(reader: RunLogReader) -> Self {
    RunProgress { reader, active: false, completions: Vec::new(), starts: Vec::new(), last_read: 0 }
  }

  fn read(&mut self, run_slug: &RunSlug) -> Result<RunLogView, RunReadFailure> {
    let run_dir = match self.reader.resolve(run_slug) {
      | RunLocation::Found(run_dir) => run_dir,
      | RunLocation::Missing => return Err(RunReadFailure::Missing),
      | RunLocation::NotListed => return Err(RunReadFailure::NotListed),
      | RunLocation::Unreadable(reason) => return Err(RunReadFailure::Unreadable(reason)),
      | RunLocation::Rejected(reason) => return Err(RunReadFailure::Rejected(reason)),
    };
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

    // 読み取り層はファイルの縮小や置き換えで位置を 0 に戻す。そのときは先頭の workflow_start で
    // 集計が初期化されるため、ここで集計を消す必要はない。
    for record in self.reader.read_body_log(&run_dir).records {
      self.ingest(&record);
    }
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
    })
  }

  fn ingest(&mut self, record: &Value) {
    match record.get("type").and_then(Value::as_str) {
      | Some("workflow_start") => {
        self.active = true;
        self.completions.clear();
        self.starts.clear();
      },
      | Some("step_start") if self.active => self.starts.push(StepStart::from_record(record)),
      | Some("step_complete") if self.active => self.completions.push(StepCompletion::from_record(record)),
      | _ => {},
    }
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
    }
  }
}

/// 1プロジェクトの run の集計。プロジェクトの cwd と、run ごとの集計を持ちます。
#[derive(Debug)]
pub struct ProjectRunLogs {
  project_cwd:    PathBuf,
  worktrees_root: PathBuf,
  runs:           BTreeMap<RunSlug, RunProgress>,
  read_sequence:  u64,
}

impl ProjectRunLogs {
  /// プロジェクトの cwd から作る。worktree の置き場は takt
  /// の既定（`<cwd>/../takt-worktrees`）です。
  pub fn new(project_cwd: PathBuf) -> Self {
    let worktrees_root = project_cwd.join("..").join(WORKTREES_DIRECTORY);
    ProjectRunLogs { project_cwd, worktrees_root, runs: BTreeMap::new(), read_sequence: 0 }
  }

  /// run を読み、前回の続きを集計へ足して現在の状態を返す。
  pub fn read(&mut self, run_slug: &RunSlug) -> Result<RunLogView, RunReadFailure> {
    self.read_sequence += 1;
    let sequence = self.read_sequence;
    let progress = self
      .runs
      .entry(run_slug.clone())
      .or_insert_with(|| RunProgress::new(RunLogReader::new(self.project_cwd.clone(), self.worktrees_root.clone())));
    progress.last_read = sequence;
    let view = progress.read(run_slug);
    self.evict_least_recently_read();
    view
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
/// Poller と GetRunDetail のサービスは、それぞれ自分の表を持ちます。
#[derive(Debug)]
pub struct RunLogHandles<K> {
  handles: BTreeMap<K, (PathBuf, RunLogHandle)>,
}

impl<K> Default for RunLogHandles<K> {
  fn default() -> Self {
    RunLogHandles { handles: BTreeMap::new() }
  }
}

impl<K: Ord + Clone> RunLogHandles<K> {
  /// プロジェクトの取っ手を返す。初めてのプロジェクトと、cwd が変わったプロジェクトは作り直します。
  pub fn handle_for(&mut self, key: &K, cwd: &Path) -> RunLogHandle {
    match self.handles.get(key) {
      | Some((known_cwd, handle)) if known_cwd == cwd => handle.clone(),
      | _ => {
        let handle = run_log_handle(cwd.to_path_buf());
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

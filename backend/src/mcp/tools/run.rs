//! takt_get_run の応答から run を作る。
//!
//! run は通常、run の保存場所（[`crate::run_progress`]）から読みます。ローカルに保存場所を読めない
//! プロジェクト（SSH 越しに扱うリモートのサーバーのプロジェクトなど）の run だけ、takt-mcp の
//! takt_get_run の応答から作ります。同じプロジェクトの run のレポート本文も、takt_get_run の
//! view report から読みます。

use chrono::{DateTime, Utc};
use serde_json::{json, Value};

use super::{
  parse::{domain_value, optional_string, parse_time, string_field},
  ToolError, ToolSet,
};
use crate::{
  domain::{ReportFilename, RunSlug},
  model::{ReportEntry, RunEntry, RunStatus, RunStepEntry, StepLogEntry, TaskEntry},
  run_log::RunContent,
  run_progress::task_summary,
};

/// takt_get_run から読んだ run。stepLogs は全ページを集めたものです。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteRun {
  pub run_slug: RunSlug,
  pub session_id: String,
  /// 応答の summary。無ければ taskText の1行目（takt のタスク要約と同じ規則）です。
  pub summary: String,
  pub task_text: String,
  pub workflow: Option<String>,
  pub status: RunStatus,
  pub current_step: Option<String>,
  pub current_iteration: Option<i32>,
  pub phase: Option<i32>,
  pub started_at: Option<DateTime<Utc>>,
  pub current_step_started_at: Option<DateTime<Utc>>,
  pub current_step_execution_count: Option<i32>,
  pub steps: Vec<RunStepEntry>,
  pub step_logs: Vec<StepLogEntry>,
  pub reports: Vec<ReportEntry>,
  /// 応答の lastLogAt。
  last_log_at: Option<DateTime<Utc>>,
}

impl RemoteRun {
  /// 一覧の run を作る。ステップ位置は workflow の一覧を持つ呼び出し側が補います。
  pub fn run_entry(&self, task: &TaskEntry) -> RunEntry {
    let started_at = self.started_at(Some(task));
    RunEntry {
      run_slug: self.run_slug.as_str().to_owned(),
      session_id: self.session_id.clone(),
      task_name: task.name.clone(),
      goal_id: task.goal_id.clone(),
      summary: self.summary.clone(),
      status: self.status,
      current_step: self.current_step.clone(),
      current_step_number: None,
      total_steps: None,
      current_iteration: self.current_iteration,
      started_at,
      current_step_started_at: self.current_step_started_at,
      last_log_at: self.resolve_last_log_at(started_at),
      steps: self.steps.clone(),
      reports: self.reports.clone(),
    }
  }

  /// run の開始時刻。応答の開始時刻を使い、無ければタスクの開始時刻を使います。
  pub fn started_at(&self, task: Option<&TaskEntry>) -> Option<DateTime<Utc>> {
    self.started_at.or_else(|| task.and_then(|task| task.started_at))
  }

  /// 最終ログ時刻を決める。
  ///
  /// 応答の `lastLogAt` を最優先し、無いときだけ、時刻を持つステップの記録の最新と現在ステップの
  /// 開始時刻の新しい方を使います。どちらも無ければ run の開始時刻です。
  pub fn resolve_last_log_at(&self, started_at: Option<DateTime<Utc>>) -> Option<DateTime<Utc>> {
    let latest_step = self
      .step_logs
      .iter()
      .map(|log| log.timestamp)
      .filter(|timestamp| *timestamp != DateTime::<Utc>::UNIX_EPOCH)
      .max();
    self.last_log_at.or(latest_step.into_iter().chain(self.current_step_started_at).max()).or(started_at)
  }
}

/// takt_get_run の応答（先頭ページ）と、全ページから集めたステップの記録から run を作る。
pub(super) fn parse_remote_run(
  value: &Value,
  steps: Vec<RunStepEntry>,
  step_logs: Vec<StepLogEntry>,
) -> Result<RemoteRun, ToolError> {
  let run_slug = domain_value("run slug", RunSlug::try_from(string_field(value, "runSlug")?.as_str()))?;
  let task_text = optional_string(value, "taskText").unwrap_or_default();
  let summary = optional_string(value, "summary").unwrap_or_else(|| task_summary(&task_text));
  let started_at = match parse_time(value.get("startedAt"))? {
    | Some(time) => Some(time),
    | None => parse_time(value.get("startTime"))?,
  };
  Ok(RemoteRun {
    run_slug,
    session_id: optional_string(value, "sessionId").unwrap_or_default(),
    summary,
    task_text,
    workflow: optional_string(value, "workflow").filter(|workflow| !workflow.is_empty()),
    status: parse_run_status(value.get("status"))?,
    current_step: optional_string(value, "currentStep"),
    current_iteration: optional_i32(value, "currentIteration"),
    phase: optional_i32(value, "phase"),
    started_at,
    current_step_started_at: parse_time(value.get("currentStepStartedAt"))?,
    current_step_execution_count: optional_i32(value, "currentStepExecutionCount"),
    steps,
    step_logs,
    reports: report_summaries(value),
    last_log_at: parse_time(value.get("lastLogAt"))?,
  })
}

/// 任意の整数フィールドを読む。i32 に収まらない値は無いものとして扱います。
fn optional_i32(value: &Value, key: &str) -> Option<i32> {
  value.get(key).and_then(Value::as_i64).and_then(|value| i32::try_from(value).ok())
}

/// run の状態を既知の値だけから解析する。欠落と未知の値は解析エラーにします。
fn parse_run_status(value: Option<&Value>) -> Result<RunStatus, ToolError> {
  let text =
    value.and_then(Value::as_str).ok_or_else(|| ToolError::Parse("run の status が文字列ではありません".to_owned()))?;
  domain_value("run の status", RunStatus::try_from(text))
}

/// 応答の stepLogs 配列から、一覧のステップを出現順に取り出す。
///
/// 開始時刻は `startedAt` を優先し、無ければ `timestamp` で補い、どちらも無ければ `UNIX_EPOCH`
/// に落とします。takt の応答に `model` は無いため、あれば読み、無ければ空です。
pub(super) fn step_entries(value: &Value) -> Vec<RunStepEntry> {
  value
    .get("stepLogs")
    .and_then(Value::as_array)
    .map(|logs| {
      logs
        .iter()
        .map(|log| RunStepEntry {
          step_name:  optional_string(log, "step").unwrap_or_default(),
          model:      optional_string(log, "model").unwrap_or_default(),
          started_at: parse_time(log.get("startedAt"))
            .ok()
            .flatten()
            .or_else(|| parse_time(log.get("timestamp")).ok().flatten())
            .unwrap_or(DateTime::<Utc>::UNIX_EPOCH),
          ended_at:   parse_time(log.get("endedAt")).ok().flatten(),
        })
        .collect()
    })
    .unwrap_or_default()
}

/// 応答の stepLogs 配列から、run 詳細のステップの記録を出現順に取り出す。
pub(super) fn step_log_entries(value: &Value) -> Vec<StepLogEntry> {
  value
    .get("stepLogs")
    .and_then(Value::as_array)
    .map(|logs| {
      logs
        .iter()
        .map(|log| StepLogEntry {
          step_name: optional_string(log, "step").unwrap_or_default(),
          persona:   optional_string(log, "persona").unwrap_or_default(),
          status:    optional_string(log, "status").unwrap_or_default(),
          content:   optional_string(log, "content").unwrap_or_default(),
          timestamp: parse_time(log.get("timestamp")).ok().flatten().unwrap_or(DateTime::<Utc>::UNIX_EPOCH),
          iteration: optional_i32(log, "iteration").unwrap_or_default(),
        })
        .collect()
    })
    .unwrap_or_default()
}

/// 応答の reports 配列からレポートの一覧（名前と大きさ）を出現順に取り出す。
fn report_summaries(value: &Value) -> Vec<ReportEntry> {
  value
    .get("reports")
    .and_then(Value::as_array)
    .map(|reports| {
      reports
        .iter()
        .map(|report| ReportEntry {
          filename:   optional_string(report, "filename").unwrap_or_default(),
          size_bytes: report.get("sizeBytes").and_then(Value::as_i64).unwrap_or_default(),
        })
        .collect()
    })
    .unwrap_or_default()
}

/// stepLogPage.nextOffset を取り出す。null または欠落なら None（追従の終わり）です。
pub(super) fn step_log_next_offset(value: &Value) -> Option<usize> {
  value
    .get("stepLogPage")
    .and_then(|page| page.get("nextOffset"))
    .and_then(Value::as_u64)
    .and_then(|offset| usize::try_from(offset).ok())
}

/// takt_get_run の view report が1回で返す本文の上限（バイト）。takt-mcp の入力契約で
/// maxBytes は 4〜4096 です。
const REPORT_PAGE_MAX_BYTES: u64 = 4096;
/// takt_get_run の view report に渡せる maxBytes の下限（バイト）。
const REPORT_PAGE_MIN_BYTES: u64 = 4;

impl ToolSet<'_> {
  /// takt_get_run の view report から、run のレポート `filename` の本文を先頭から `max_bytes`
  /// まで読む。
  ///
  /// takt_get_run は1回の応答で最大 [`REPORT_PAGE_MAX_BYTES`] バイトの本文を
  /// `report.{content,totalBytes,nextOffset}` で返すため、nextOffset をたどって連結します。
  /// 対象のレポートが無いと takt-mcp は道具エラーを返します。応答に `report` が無い、型が違う、
  /// または nextOffset が進まない場合は解析エラーにします。
  pub async fn read_remote_report(
    &mut self,
    run_slug: &RunSlug,
    filename: &ReportFilename,
    max_bytes: u64,
  ) -> Result<RunContent, ToolError> {
    let mut content = String::new();
    let mut offset = 0_u64;
    loop {
      let remaining = max_bytes.saturating_sub(content.len() as u64);
      let value = self
        .call_json(
          "takt_get_run",
          json!({
            "runSlug": run_slug.as_str(),
            "view": "report",
            "report": filename.as_str(),
            "offset": offset,
            "maxBytes": remaining.clamp(REPORT_PAGE_MIN_BYTES, REPORT_PAGE_MAX_BYTES),
          }),
        )
        .await?;
      let page = parse_report_page(&value)?;
      content.push_str(&page.content);
      let next = match page.next_offset {
        | Some(next) if next <= offset || page.content.is_empty() => {
          return Err(ToolError::Parse("takt_get_run の report の nextOffset が進みません".to_owned()));
        },
        | next => next,
      };
      let collected = content.len() as u64;
      match next {
        | Some(next) if collected + REPORT_PAGE_MIN_BYTES <= max_bytes => offset = next,
        | _ => {
          truncate_at_char_boundary(&mut content, max_bytes);
          let truncated = (content.len() as u64) < page.total_bytes;
          return Ok(RunContent { content, size_bytes: page.total_bytes, truncated });
        },
      }
    }
  }
}

/// takt_get_run の view report が返す、レポート本文の1ページ。
struct ReportPage {
  content:     String,
  /// レポート全体の大きさ（バイト）。
  total_bytes: u64,
  /// 続きの読み位置。最後のページでは `None` です。
  next_offset: Option<u64>,
}

/// takt_get_run の view report の応答から `report.{content,totalBytes,nextOffset}` を読む。
///
/// `report` が無い、または各項目の型が違う場合は解析エラーにします。
fn parse_report_page(value: &Value) -> Result<ReportPage, ToolError> {
  let report = value
    .get("report")
    .filter(|report| report.is_object())
    .ok_or_else(|| ToolError::Parse("takt_get_run の応答に report がありません".to_owned()))?;
  let content = string_field(report, "content")?;
  let total_bytes = report
    .get("totalBytes")
    .and_then(Value::as_u64)
    .ok_or_else(|| ToolError::Parse("report の totalBytes が数値ではありません".to_owned()))?;
  let next_offset = match report.get("nextOffset") {
    | None | Some(Value::Null) => None,
    | Some(next) => {
      Some(next.as_u64().ok_or_else(|| ToolError::Parse("report の nextOffset が数値ではありません".to_owned()))?)
    },
  };
  Ok(ReportPage { content, total_bytes, next_offset })
}

/// 本文を `max_bytes` 以内の文字の境界で切り詰める。
fn truncate_at_char_boundary(content: &mut String, max_bytes: u64) {
  let mut end = usize::try_from(max_bytes).unwrap_or(usize::MAX).min(content.len());
  while !content.is_char_boundary(end) {
    end -= 1;
  }
  content.truncate(end);
}

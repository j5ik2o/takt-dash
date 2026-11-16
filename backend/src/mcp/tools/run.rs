//! run 一覧・詳細の応答解析と最終ログ時刻の決定。

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::{
  parse::{optional_string, parse_run_start_time, parse_time, string_field},
  ToolError,
};
use crate::model::{
  ReportEntry, RunDetail, RunEntry, RunInstructionEntry, RunReportEntry, RunStatus, RunStepEntry, StepLogEntry,
  TaskEntry,
};

/// 一覧と詳細で共通の最終ログ時刻の決定順です。
///
/// 応答の `lastLogAt` を最優先し、無いときだけ全ページのステップ記録の最新時刻と現在ステップの
/// 開始時刻の新しい方を使います。どちらも無ければ run の開始時刻に落とします。概要（`parse_run`）と
/// 詳細（`parse_run_detail`）が同じ決定順を通り、無音判定が食い違わないようにします。
///
/// 時刻を持つステップ記録が 1 件も無いときは、欠落時刻（`UNIX_EPOCH`）を最新として採らず、
/// 現在ステップの開始時刻だけを第 2 候補にします。
fn resolve_last_log_at(
  value: &Value,
  latest_step_log: Option<DateTime<Utc>>,
  current_step_started_at: Option<DateTime<Utc>>,
  started_at: Option<DateTime<Utc>>,
) -> Result<Option<DateTime<Utc>>, ToolError> {
  let second = match latest_step_log {
    | Some(latest) => Some(latest.max(current_step_started_at.unwrap_or(latest))),
    | None => current_step_started_at,
  };
  Ok(parse_time(value.get("lastLogAt"))?.or(second).or(started_at))
}

pub(super) fn parse_run(
  value: &Value,
  task: &TaskEntry,
  latest_step_log: Option<DateTime<Utc>>,
  steps: Vec<RunStepEntry>,
) -> Result<RunEntry, ToolError> {
  let run_slug = string_field(value, "runSlug")?;
  let started_at = parse_run_start_time(value)?.or(task.started_at);
  let current_step_started_at = parse_time(value.get("currentStepStartedAt"))?;
  let last_log_at = resolve_last_log_at(value, latest_step_log, current_step_started_at, started_at)?;
  Ok(RunEntry {
    run_slug,
    session_id: optional_string(value, "sessionId").unwrap_or_default(),
    task_name: task.name.clone(),
    goal_id: task.goal_id.clone(),
    summary: optional_string(value, "summary").unwrap_or_default(),
    status: parse_run_status(value.get("status")),
    current_step: optional_string(value, "currentStep"),
    current_step_number: None,
    total_steps: None,
    current_iteration: value.get("currentIteration").and_then(Value::as_i64).map(|value| value as i32),
    started_at,
    current_step_started_at,
    last_log_at,
    steps,
    reports: report_summaries(value),
  })
}

/// run の状態を寛容に解析する。欠落と未知の値は継続中として扱います。
fn parse_run_status(value: Option<&Value>) -> RunStatus {
  match value.and_then(Value::as_str) {
    | Some("completed") => RunStatus::Completed,
    | Some("aborted") => RunStatus::Aborted,
    | Some("failed") => RunStatus::Failed,
    | _ => RunStatus::Running,
  }
}

/// 本文つきの stepLogs のうち、時刻を持つものの最新を返す。
///
/// `step_log_entries` は timestamp 欠落を `UNIX_EPOCH` にするため、その実値を最新時刻の候補に
/// 含めません。時刻を持つ stepLog が無ければ `None` を返します。
fn latest_step_log_at(step_logs: &[StepLogEntry]) -> Option<DateTime<Utc>> {
  step_logs.iter().map(|log| log.timestamp).filter(|timestamp| *timestamp != DateTime::<Utc>::UNIX_EPOCH).max()
}

/// 応答の stepLogs 配列からステップの記録を出現順に取り出す。
///
/// 実 takt-mcp の応答で欠ける項目を解析エラーにしないため、開始時刻は `startedAt` を優先し、
/// 無ければ `timestamp` で補い、どちらも無ければ `UNIX_EPOCH` に落とします。開始時刻を run の
/// 開始時刻で補うことはしません（run の開始時刻は step の開始より前になり得るため）。
pub(super) fn step_entries(value: &Value) -> Vec<RunStepEntry> {
  let Some(logs) = value.get("stepLogs").and_then(Value::as_array) else {
    return Vec::new();
  };
  let mut steps = Vec::new();
  for log in logs {
    let step_name = optional_string(log, "step").unwrap_or_default();
    let started_at =
      parse_time(log.get("startedAt")).ok().flatten().or_else(|| parse_time(log.get("timestamp")).ok().flatten());
    let ended_at = parse_time(log.get("endedAt")).ok().flatten();
    steps.push(RunStepEntry {
      step_name,
      model: optional_string(log, "model").unwrap_or_default(),
      started_at: started_at.unwrap_or(DateTime::<Utc>::UNIX_EPOCH),
      ended_at,
    });
  }
  steps
}

/// 応答の reports 配列からレポートの要約を出現順に取り出す。
pub(super) fn report_summaries(value: &Value) -> Vec<ReportEntry> {
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

/// stepLogPage.nextOffset を取り出す。null または欠落なら None（追従終端）。
pub(super) fn step_log_next_offset(value: &Value) -> Option<usize> {
  value.get("stepLogPage").and_then(|page| page.get("nextOffset")).and_then(Value::as_u64).map(|offset| offset as usize)
}

pub(super) fn parse_run_detail(
  value: &Value,
  run_slug: &str,
  task: Option<&TaskEntry>,
  step_logs: Vec<StepLogEntry>,
  reports: Vec<RunReportEntry>,
  instructions: Vec<RunInstructionEntry>,
) -> Result<RunDetail, ToolError> {
  let started_at = parse_run_start_time(value)?;
  let current_step_started_at = parse_time(value.get("currentStepStartedAt"))?;
  let last_log_at = resolve_last_log_at(value, latest_step_log_at(&step_logs), current_step_started_at, started_at)?;
  let task_name = optional_string(value, "taskName").or_else(|| task.map(|task| task.name.clone())).unwrap_or_default();
  let goal_id = optional_string(value, "goalId").or_else(|| task.and_then(|task| task.goal_id.clone()));
  Ok(RunDetail {
    run_slug: run_slug.to_owned(),
    session_id: optional_string(value, "sessionId").unwrap_or_default(),
    task_name,
    goal_id,
    task_text: optional_string(value, "taskText").unwrap_or_default(),
    summary: optional_string(value, "summary").unwrap_or_default(),
    workflow: optional_string(value, "workflow").unwrap_or_default(),
    status: parse_run_status(value.get("status")),
    started_at,
    current_step: optional_string(value, "currentStep"),
    current_iteration: value.get("currentIteration").and_then(Value::as_i64).map(|value| value as i32),
    current_step_started_at,
    current_step_execution_count: value
      .get("currentStepExecutionCount")
      .and_then(Value::as_i64)
      .map(|value| value as i32),
    phase: value.get("phase").and_then(Value::as_i64).map(|value| value as i32),
    step_logs,
    log_tail: optional_string(value, "logTail").unwrap_or_default(),
    reports,
    instructions,
    last_log_at,
  })
}

/// 応答の stepLogs 配列から、本文を含むステップの記録を出現順に取り出す。
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
          iteration: log.get("iteration").and_then(Value::as_i64).unwrap_or_default() as i32,
        })
        .collect()
    })
    .unwrap_or_default()
}

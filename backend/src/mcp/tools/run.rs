//! run 詳細の組み立て。
//!
//! 状態とステップの記録は run の保存場所（[`crate::run_progress`]）から、レポートと指示書と
//! run の説明は takt_get_run の summary から読みます。

use serde_json::Value;

use super::{
  parse::{domain_value, optional_string},
  ToolError,
};
use crate::{
  domain::{GoalId, RunSlug, TaskName},
  model::{ReportEntry, RunDetail, RunInstructionEntry, RunReportEntry, TaskEntry},
  run_progress::RunLogView,
};

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

/// run の保存場所から読んだ状態と、takt_get_run の summary から run 詳細を組み立てる。
///
/// 開始時刻と最終ログ時刻は一覧の run（[`RunLogView::run_entry`]）と同じ規則で決めます。
pub(super) fn assemble_run_detail(
  summary: &Value,
  view: &RunLogView,
  run_slug: &RunSlug,
  task: Option<&TaskEntry>,
  reports: Vec<RunReportEntry>,
  instructions: Vec<RunInstructionEntry>,
) -> Result<RunDetail, ToolError> {
  let started_at = view.started_at(task);
  let task_name = match optional_string(summary, "taskName") {
    | Some(text) if text.is_empty() => task.map(|task| task.name.clone()),
    | Some(text) => Some(domain_value("タスク名", TaskName::try_from(text.as_str()))?),
    | None => task.map(|task| task.name.clone()),
  };
  let goal_id = match optional_string(summary, "goalId") {
    | Some(text) if text.is_empty() => task.and_then(|task| task.goal_id.clone()),
    | Some(text) => Some(domain_value("ゴール ID", GoalId::try_from(text.as_str()))?),
    | None => task.and_then(|task| task.goal_id.clone()),
  };
  Ok(RunDetail {
    run_slug: run_slug.clone(),
    session_id: optional_string(summary, "sessionId").unwrap_or_default(),
    task_name,
    goal_id,
    task_text: optional_string(summary, "taskText").unwrap_or_default(),
    summary: optional_string(summary, "summary").unwrap_or_default(),
    workflow: view.workflow.clone().unwrap_or_default(),
    status: view.status,
    started_at,
    current_step: view.current_step.clone(),
    current_iteration: view.current_iteration,
    current_step_started_at: view.current_step_started_at,
    current_step_execution_count: view.current_step_execution_count,
    phase: view.phase,
    step_logs: view.step_logs(),
    log_tail: optional_string(summary, "logTail").unwrap_or_default(),
    reports,
    instructions,
    last_log_at: view.resolve_last_log_at(started_at),
  })
}

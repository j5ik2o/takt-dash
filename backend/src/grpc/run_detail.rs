//! GetRunDetail の応答のうち run の状態を表す部分を、読み取り元ごとに組み立てる。
//!
//! 読み取り元は2つです。ローカルで読める run は run の保存場所の集計（[`RunLogView`]）から、
//! 保存場所の対応表に無い run と対応表を読めない run（SSH 越しに扱うリモートのサーバーの
//! プロジェクトなど）は takt-mcp の takt_get_run（[`RemoteRun`]）から組み立てます。どちらも
//! 最終ログ時刻を返し、ログの無音の判定は呼び出し側が状態ストアの閾値で行います。
//!
//! 詳細では、要約に含まれない指示書の一覧とタスク本文も takt_get_run の view から読みます。
//! GetRunUpdates の追記分も、ローカルで続きを読めない run は takt_get_run から組み立てます
//! （[`remote_run_updates`]）。

use std::{
  collections::hash_map::DefaultHasher,
  hash::{Hash, Hasher},
};

use chrono::{DateTime, Utc};
use tonic::Status;

use super::{
  mapping::{optional_timestamp, run_status, step_log},
  run_instruction, run_read_failure_status, ResolvedProject,
};
use crate::{
  domain::RunSlug,
  mcp::{McpClient, RemoteRun, ToolError, ToolSet},
  model::{ReportEntry, TaskEntry},
  run_log::SessionActivity,
  run_progress::{LogCursor, LogLine, RunLogView, RunReadFailure, RunUpdates},
  takt_dash::v1::{GetRunDetailResponse, RunReport},
};

/// 保存場所の集計から run の状態を埋め、最終ログ時刻を返す。
pub(super) fn fill_from_run_log(
  response: &mut GetRunDetailResponse,
  view: &RunLogView,
  task: Option<&TaskEntry>,
) -> Option<DateTime<Utc>> {
  let started_at = view.started_at(task);
  let last_log_at = view.resolve_last_log_at(started_at);
  response.task_text = view.task_text.clone().unwrap_or_default();
  if response.summary.is_empty() {
    response.summary = crate::run_progress::task_summary(&response.task_text);
  }
  response.workflow = view.workflow.clone().unwrap_or_default();
  response.status = run_status(view.status) as i32;
  response.started_at = optional_timestamp(started_at);
  response.current_step = view.current_step.clone();
  response.current_iteration = view.current_iteration;
  response.current_step_started_at = optional_timestamp(view.current_step_started_at);
  response.current_step_execution_count = view.current_step_execution_count;
  response.phase = view.phase;
  response.step_logs = view.step_logs().iter().map(step_log).collect();
  response.log_tail = view.log_tail();
  response.reports = run_reports(view.reports());
  response.last_log_at = optional_timestamp(last_log_at);
  last_log_at
}

/// takt_get_run から run を読んで状態を埋め、最終ログ時刻を返す。
///
/// takt_get_run でも読めなければ、保存場所の読み取りの失敗 `failure` のステータスを返します。
pub(super) async fn fill_from_takt_get_run(
  response: &mut GetRunDetailResponse,
  resolved: &ResolvedProject,
  run_slug: &RunSlug,
  task: Option<&TaskEntry>,
  failure: RunReadFailure,
) -> Result<Option<DateTime<Utc>>, Status> {
  let remote = match read_remote_run_detail(resolved, run_slug).await {
    | Ok(remote) => remote,
    | Err(error) => {
      tracing::warn!(run_slug = %run_slug.as_str(), local = %failure, error = %error, "run 詳細を保存場所からも takt_get_run からも読めませんでした。");
      return Err(run_read_failure_status(failure));
    },
  };
  let started_at = remote.started_at(task);
  let last_log_at = remote.resolve_last_log_at(started_at);
  response.task_text = remote.task_text.clone();
  if response.summary.is_empty() {
    response.summary = remote.summary.clone();
  }
  response.workflow = remote.workflow.clone().unwrap_or_default();
  response.status = run_status(remote.status) as i32;
  response.started_at = optional_timestamp(started_at);
  response.current_step = remote.current_step.clone();
  response.current_iteration = remote.current_iteration;
  response.current_step_started_at = optional_timestamp(remote.current_step_started_at);
  response.current_step_execution_count = remote.current_step_execution_count;
  response.phase = remote.phase;
  response.step_logs = remote.step_logs.iter().map(step_log).collect();
  response.log_tail = remote.log_tail();
  response.reports = run_reports(&remote.reports);
  response.instructions = remote.instructions.iter().map(run_instruction).collect();
  response.last_log_at = optional_timestamp(last_log_at);
  Ok(last_log_at)
}

/// ログ更新では要約用の本文取得を避け、ステップログを読んで子プロセスを終了する。
async fn read_remote_run_logs(resolved: &ResolvedProject, run_slug: &RunSlug) -> Result<RemoteRun, ToolError> {
  let mut client = McpClient::new(resolved.client_config.clone());
  let result = ToolSet::new(&mut client, resolved.cwd.clone()).read_remote_run_logs(run_slug).await;
  client.shutdown().await;
  result
}

/// 詳細用に、指示書の一覧とタスク本文まで takt_get_run から読み、子プロセスを終了する。
///
/// 要約の応答は指示書の一覧を含まず、タスク本文も含みません（taskOmission だけを返します）。
/// ローカルの読み取りが要約の値で詳細を組み立てるのと同じ値を返すため、同じ接続で
/// view instructions と view task から取り直します。一覧の読み取り（poller）はこの形を
/// 使いません。
async fn read_remote_run_detail(resolved: &ResolvedProject, run_slug: &RunSlug) -> Result<RemoteRun, ToolError> {
  let mut client = McpClient::new(resolved.client_config.clone());
  let result = async {
    let mut toolset = ToolSet::new(&mut client, resolved.cwd.clone());
    let mut remote = toolset.read_remote_run(run_slug).await?;
    remote.instructions = toolset.read_remote_instructions(run_slug).await?;
    if remote.task_text.is_empty() {
      remote.task_text = toolset.read_remote_task_text(run_slug).await?;
    }
    Ok::<RemoteRun, ToolError>(remote)
  }
  .await;
  client.shutdown().await;
  result
}

/// GetRunUpdates の追記分を takt_get_run から組み立てる。
///
/// ローカルで本体ログの続きを読めない run のための経路です。ステップの記録から作った行の
/// 末尾を返し、続きの位置は行の集まりから決まる世代で判定します。前回と同じ世代で末尾の
/// 行まで受け取り済みなら、追記分は空で reset しません。セッションログからの活動の印は
/// takt_get_run からは得られないため、印なしで返します。
///
/// takt_get_run でも読めなければ、保存場所の読み取りの失敗 `failure` のステータスを返します。
pub(super) async fn remote_run_updates(
  resolved: &ResolvedProject,
  run_slug: &RunSlug,
  after: Option<LogCursor>,
  failure: RunReadFailure,
) -> Result<RunUpdates, Status> {
  let remote = match read_remote_run_logs(resolved, run_slug).await {
    | Ok(remote) => remote,
    | Err(error) => {
      tracing::warn!(run_slug = %run_slug.as_str(), local = %failure, error = %error, "run の追記分を保存場所からも takt_get_run からも読めませんでした。");
      return Err(run_read_failure_status(failure));
    },
  };
  Ok(run_updates_from_remote(&remote, after))
}

/// リモートの run の行と続きの位置から追記分を作る。
fn run_updates_from_remote(remote: &RemoteRun, after: Option<LogCursor>) -> RunUpdates {
  let epoch = lines_epoch(&remote.log_lines());
  let tail = remote.log_tail_lines();
  let received_all = after.is_some_and(|cursor| cursor.epoch == epoch && cursor.sequence >= tail.len() as u64);
  let lines = if received_all {
    Vec::new()
  } else {
    tail.iter().enumerate().map(|(index, text)| LogLine { sequence: index as u64 + 1, text: text.clone() }).collect()
  };
  RunUpdates { epoch, reset: !received_all, lines, activity: SessionActivity::default() }
}

/// JavaScript の Number.MAX_SAFE_INTEGER（2の53乗 - 1）。世代を Web 側で損失なく
/// 扱える上限です。
const JS_MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;

/// 行の集まりから決まる世代。ステップが進んで行が変われば世代も変わります。
/// 世代は proto の i64 で往復し、Web 側では JavaScript の数値として解釈するため、
/// 安全な整数の範囲に収めます。
fn lines_epoch(lines: &[String]) -> u64 {
  let mut hasher = DefaultHasher::new();
  lines.hash(&mut hasher);
  hasher.finish() & JS_MAX_SAFE_INTEGER
}

fn run_reports(reports: &[ReportEntry]) -> Vec<RunReport> {
  reports.iter().map(|report| RunReport { filename: report.filename.clone(), size_bytes: report.size_bytes }).collect()
}

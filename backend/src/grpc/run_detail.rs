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
//!
//! GetRunDetail の全体は総上限（[`super::RUN_DETAIL_BUDGET`]）で打ち切ります（[`within_budget`]、
//! issue #59）。回り道の道具の時間切れは「run が無い」ではなく「応答なし」として返します
//! （[`remote_failure_status`]）。

use std::{
  collections::hash_map::DefaultHasher,
  future::Future,
  hash::{Hash, Hasher},
  time::Duration,
};

use chrono::{DateTime, Utc};
use tonic::Status;

use super::{
  mapping::{optional_timestamp, run_status, step_log},
  resolve::ReadSource,
  run_instruction, run_read_failure_status, TaktDashServiceImpl,
};
use crate::{
  domain::RunSlug,
  mcp::{RemoteRun, ToolError, TransportError},
  model::{ReportEntry, TaskEntry},
  run_log::SessionActivity,
  run_progress::{read_run, read_run_instructions, LogCursor, LogLine, RunLogView, RunReadFailure, RunUpdates},
  takt_dash::v1::{self, GetRunDetailResponse, RunReport},
};

/// `work` を総上限 `budget` の中で進める。上限に達したら DEADLINE_EXCEEDED を返す。
///
/// tokio の `timeout` は内側の future を先に poll
/// するので、上限ちょうどに終わった仕事は結果を返します。
pub(super) async fn within_budget<T>(
  budget: Duration,
  run_slug: &RunSlug,
  work: impl Future<Output = Result<T, Status>>,
) -> Result<T, Status> {
  match tokio::time::timeout(budget, work).await {
    | Ok(result) => result,
    | Err(_) => {
      tracing::warn!(run_slug = %run_slug.as_str(), budget_ms = budget.as_millis() as u64, "run 詳細の読み取りが総上限に達しました。");
      Err(Status::deadline_exceeded(format!(
        "run {} の詳細を {} 秒以内に読めませんでした。takt-mcp の読み取りが遅れています。",
        run_slug.as_str(),
        budget.as_secs_f64()
      )))
    },
  }
}

/// 回り道（takt_get_run）でも読めなかったときの状態。
///
/// - 道具の時間切れは「応答なし」（DEADLINE_EXCEEDED）。
/// - 時間切れ以外の輸送の失敗（起動・入出力・送れなかった要求・HTTP など）は UNAVAILABLE。HTTP の
///   セッションの確立の時間切れも、書き込みの再送の判定のため `NotSent`
///   に包まれて届くので、ここに入る。
/// - takt-mcp が答えた失敗（道具のエラー、JSON-RPC のエラー、解析の失敗）は、保存場所の読み取りの
///   失敗 `local` の状態。
///
/// 輸送の失敗は run が無いことを意味しないため、保存場所の対応表に無い run（`NotListed`）を
/// NOT_FOUND（画面の 404）にしません。
pub(super) fn remote_failure_status(local: RunReadFailure, remote: &ToolError) -> Status {
  match remote {
    | ToolError::Transport(TransportError::Timeout(seconds)) => Status::deadline_exceeded(format!(
      "takt-mcp から {seconds} 秒以内に run の詳細の応答がありませんでした（保存場所: {local}）。"
    )),
    | ToolError::Transport(TransportError::Rpc { .. }) => run_read_failure_status(local),
    | ToolError::Transport(error) => {
      Status::unavailable(format!("takt-mcp から run の詳細を読めませんでした: {error}（保存場所: {local}）。"))
    },
    | ToolError::Tool(_) | ToolError::Parse(_) => run_read_failure_status(local),
  }
}

impl TaktDashServiceImpl {
  /// 状態ストアと run の保存場所（読めなければ takt_get_run）から GetRunDetail の応答を組み立てる。
  pub(super) async fn read_run_detail(&self, run_slug: RunSlug) -> Result<GetRunDetailResponse, Status> {
    let target = self.resolve_run_target(&run_slug)?;
    let stored_run = target.view.runs.iter().find(|run| run.run_slug == run_slug.as_str()).cloned();
    // 完了した run は実行中一覧から消えるため、状態ストアのタスクからゴールとタスク名を補います。
    // 再開した run は、タスクの sourceRunSlug が元 run の runSlug と一致します。
    let stored_task = target
      .view
      .tasks
      .iter()
      .find(|task| task.run_slug.as_ref() == Some(&run_slug))
      .or_else(|| target.view.tasks.iter().find(|task| task.source_run_slug.as_deref() == Some(run_slug.as_str())))
      .cloned();
    let run_logs = self.run_log_handle(&target.resolved.project, &target.resolved.cwd)?;
    let read = read_run(&run_logs, run_slug.clone()).await;
    // 指示書も run の保存場所から読みます。保存場所を読めない run は takt_get_run から
    // 読む経路へ回るため、失敗はここで確定させず、保存場所から読めた場合にだけ返します。
    let instructions = read_run_instructions(target.resolved.cwd.clone(), run_slug.clone()).await;
    let goal_id = stored_run
      .as_ref()
      .and_then(|run| run.goal_id.clone())
      .or_else(|| stored_task.as_ref().and_then(|task| task.goal_id.clone()));
    let task_name = stored_run
      .as_ref()
      .map(|run| run.task_name.clone())
      .or_else(|| stored_task.as_ref().map(|task| task.name.clone()))
      .map(|task_name| task_name.as_str().to_owned())
      .unwrap_or_default();
    let mut response = GetRunDetailResponse {
      run_slug: run_slug.as_str().to_owned(),
      session_id: stored_run.as_ref().map(|run| run.session_id.clone()).unwrap_or_default(),
      server_id: target.resolved.server_id.as_str().to_owned(),
      project: target.view.name.clone(),
      goal_id: goal_id.map(|goal_id| goal_id.as_str().to_owned()).unwrap_or_default(),
      task_name,
      summary: stored_run.as_ref().map(|run| run.summary.clone()).unwrap_or_default(),
      instructions: instructions.as_ref().map(|list| list.iter().map(run_instruction).collect()).unwrap_or_default(),
      server_connected: target.view.connected,
      server_last_read_at: optional_timestamp(target.view.last_read_at),
      ..Default::default()
    };
    match read {
      | Ok(view) => {
        instructions.map_err(run_read_failure_status)?;
        tracing::info!(
          run_slug = %run_slug.as_str(),
          body_log_bytes_read = view.body_log_bytes_read,
          "run 詳細のために本体ログを読みました。"
        );
        let last_log_at = fill_from_run_log(&mut response, &view, stored_task.as_ref());
        response.log_silent = self.store.is_log_silent_at(last_log_at, self.now());
      },
      // 掃除済みの worktree など、run の保存場所が残っていない run はエラーにせず、状態ストアの値と
      // ログが残っていないことを返します。
      | Err(RunReadFailure::Missing) => {
        let started_at = stored_run
          .as_ref()
          .and_then(|run| run.started_at)
          .or_else(|| stored_task.as_ref().and_then(|task| task.started_at));
        let last_log_at = stored_run.as_ref().and_then(|run| run.last_log_at);
        response.logs_removed = true;
        response.status =
          stored_run.as_ref().map_or(v1::RunStatus::Unspecified as i32, |run| run_status(run.status) as i32);
        response.started_at = optional_timestamp(started_at);
        response.current_step = stored_run.as_ref().and_then(|run| run.current_step.clone());
        response.current_iteration = stored_run.as_ref().and_then(|run| run.current_iteration);
        response.current_step_started_at =
          optional_timestamp(stored_run.as_ref().and_then(|run| run.current_step_started_at));
        response.last_log_at = optional_timestamp(last_log_at);
        response.log_silent = self.store.is_log_silent_at(last_log_at, self.now());
      },
      // 保存場所の対応表に無い run と、対応表を読めない run（SSH 越しに扱うリモートのサーバーの
      // プロジェクトなど）は takt_get_run から読みます。takt_get_run でも読めなければ、保存場所の
      // 読み取りの失敗を返します。
      | Err(failure @ (RunReadFailure::NotListed | RunReadFailure::Unreadable(_))) => {
        let mut source = self.read_source(&target.resolved);
        let last_log_at = fill_from_takt_get_run(
          &mut response,
          &mut source,
          &target.resolved.cwd,
          &run_slug,
          stored_task.as_ref(),
          failure,
        )
        .await?;
        response.log_silent = self.store.is_log_silent_at(last_log_at, self.now());
      },
      | Err(failure) => return Err(run_read_failure_status(failure)),
    }
    Ok(response)
  }
}

/// 保存場所の集計から run の状態を埋め、最終ログ時刻を返す。
pub(super) fn fill_from_run_log(
  response: &mut GetRunDetailResponse,
  view: &RunLogView,
  task: Option<&TaskEntry>,
) -> Option<DateTime<Utc>> {
  let started_at = view.started_at(task);
  let last_log_at = view.resolve_last_log_at(started_at);
  response.task_text = view.task_text.clone().unwrap_or_default();
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
/// takt_get_run でも読めなければ、[`remote_failure_status`] の状態（時間切れは DEADLINE_EXCEEDED、
/// それ以外は保存場所の読み取りの失敗 `failure` の状態）を返します。
pub(super) async fn fill_from_takt_get_run(
  response: &mut GetRunDetailResponse,
  source: &mut ReadSource,
  cwd: &std::path::Path,
  run_slug: &RunSlug,
  task: Option<&TaskEntry>,
  failure: RunReadFailure,
) -> Result<Option<DateTime<Utc>>, Status> {
  let remote = match source.read_remote_run_detail(cwd, run_slug).await {
    | Ok(remote) => remote,
    | Err(error) => {
      tracing::warn!(run_slug = %run_slug.as_str(), local = %failure, error = %error, "run 詳細を保存場所からも takt_get_run からも読めませんでした。");
      return Err(remote_failure_status(failure, &error));
    },
  };
  let started_at = remote.started_at(task);
  let last_log_at = remote.resolve_last_log_at(started_at);
  response.task_text = remote.task_text.clone();
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

/// GetRunUpdates の追記分を takt_get_run から組み立てる。
///
/// ローカルで本体ログの続きを読めない run のための経路です。ステップの記録から作った行の
/// 末尾を返し、続きの位置は行の集まりから決まる世代で判定します。前回と同じ世代で末尾の
/// 行まで受け取り済みなら、追記分は空で reset しません。セッションログからの活動の印は
/// takt_get_run からは得られないため、印なしで返します。
///
/// takt_get_run でも読めなければ、保存場所の読み取りの失敗 `failure` のステータスを返します。
pub(super) async fn remote_run_updates(
  source: &mut ReadSource,
  cwd: &std::path::Path,
  run_slug: &RunSlug,
  after: Option<LogCursor>,
  failure: RunReadFailure,
) -> Result<RunUpdates, Status> {
  let remote = match source.read_remote_run(cwd, run_slug).await {
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

#[cfg(test)]
mod tests {
  use proptest::prelude::*;

  use super::*;
  use crate::mcp::ToolCallError;

  fn run_slug() -> RunSlug {
    RunSlug::try_from("run-1").expect("テストの runSlug は有効です")
  }

  proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    /// 設計の PBT 1：返る時刻は min(work, budget)。work が budget 以下なら仕事の結果、超えれば
    /// DEADLINE_EXCEEDED（上限ちょうどは、内側を先に poll するので結果を返す）。終わらない仕事も
    /// budget で返る。
    #[test]
    fn within_budget_returns_at_the_earlier_of_the_work_and_the_budget(
      budget in 1u64..=120,
      work in prop::option::of(0u64..=180),
    ) {
      let runtime = tokio::runtime::Builder::new_current_thread().enable_time().start_paused(true).build().unwrap();
      runtime.block_on(async move {
        let started = tokio::time::Instant::now();
        let result = within_budget(Duration::from_secs(budget), &run_slug(), async move {
          match work {
            | Some(work) => tokio::time::sleep(Duration::from_secs(work)).await,
            | None => std::future::pending::<()>().await,
          }
          Ok::<_, Status>(work)
        })
        .await;
        let elapsed = started.elapsed();
        match work {
          | Some(work) if work <= budget => {
            assert_eq!(result.expect("上限の中の仕事は結果を返します"), Some(work));
            assert_eq!(elapsed, Duration::from_secs(work));
          },
          | _ => {
            let status = result.expect_err("上限を超える仕事は打ち切ります");
            assert_eq!(status.code(), tonic::Code::DeadlineExceeded);
            assert!(status.message().contains("run-1"), "説明に run を書きます: {}", status.message());
            assert_eq!(elapsed, Duration::from_secs(budget));
          },
        }
      });
    }

    /// 設計の PBT 3：回り道の道具の時間切れは必ず DEADLINE_EXCEEDED。時間切れ以外の輸送の失敗は
    /// UNAVAILABLE で、NOT_FOUND にはならない。takt-mcp が答えた失敗は保存場所の失敗の状態。
    #[test]
    fn remote_failure_status_never_turns_a_transport_failure_into_not_found(
      local_kind in 0usize..2,
      remote_kind in 0usize..12,
      seconds in any::<u64>(),
      code in any::<i64>(),
      text in ".{0,16}",
    ) {
      let local = || match local_kind {
        | 0 => RunReadFailure::NotListed,
        | _ => RunReadFailure::Unreadable(text.clone()),
      };
      let remote = match remote_kind {
        | 0 => ToolError::Transport(TransportError::Timeout(seconds)),
        | 1 => ToolError::Transport(TransportError::Io(text.clone())),
        | 2 => ToolError::Transport(TransportError::ProcessExited),
        | 3 => ToolError::Transport(TransportError::Spawn(text.clone())),
        | 4 => ToolError::Transport(TransportError::NotSent(text.clone())),
        | 5 => ToolError::Transport(TransportError::Backoff(seconds)),
        | 6 => ToolError::Transport(TransportError::InvalidJson(text.clone())),
        | 7 => ToolError::Transport(TransportError::Http(text.clone())),
        | 8 => ToolError::Transport(TransportError::Protocol(text.clone())),
        | 9 => ToolError::Transport(TransportError::Rpc { code, message: text.clone() }),
        | 10 => ToolError::Tool(ToolCallError { tool: "takt_get_run".to_owned(), message: text.clone() }),
        | _ => ToolError::Parse(text.clone()),
      };
      let status = remote_failure_status(local(), &remote);
      match remote_kind {
        | 0 => prop_assert_eq!(status.code(), tonic::Code::DeadlineExceeded),
        | 1..=8 => prop_assert_eq!(status.code(), tonic::Code::Unavailable),
        | _ => prop_assert_eq!(status.code(), run_read_failure_status(local()).code()),
      }
    }
  }
}

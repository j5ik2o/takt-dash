//! GetRunUpdates の要求の検証と応答の組み立て。
//!
//! 本体ログの追記分の読み取りは [`crate::run_progress`] が行い、ここでは proto の要求値を
//! 続きの位置に変換し、読み取り結果を proto の応答の形にします。ローカルで続きを読めない
//! run は、[`super::run_detail`] の takt_get_run 経路へ委ねます。

use tonic::Status;

use super::{mapping::optional_timestamp, run_detail::remote_run_updates, run_read_failure_status, ResolvedProject};
use crate::{
  domain::RunSlug,
  run_progress::{read_run_updates, LogCursor, RunLogHandle, RunReadFailure, RunUpdates},
  takt_dash::v1::{GetRunUpdatesResponse, RunLogLine, RunSessionActivity},
};

/// 要求の続きの位置を検証する。世代と番号は両方を送るか両方を省き、どちらも負でない値です。
pub(super) fn parse_log_cursor(epoch: Option<i64>, after_sequence: Option<i64>) -> Result<Option<LogCursor>, Status> {
  match (epoch, after_sequence) {
    | (None, None) => Ok(None),
    | (Some(epoch), Some(sequence)) => {
      let epoch = u64::try_from(epoch).map_err(|_| Status::invalid_argument(format!("epoch が負です: {epoch}")))?;
      let sequence = u64::try_from(sequence)
        .map_err(|_| Status::invalid_argument(format!("after_sequence が負です: {sequence}")))?;
      Ok(Some(LogCursor { epoch, sequence }))
    },
    | _ => Err(Status::invalid_argument("epoch と after_sequence は両方を指定するか、両方を省いてください。")),
  }
}

/// GetRunUpdates の応答を作る。
///
/// ローカルの保存場所から続きを読むのが基本です。保存場所が残っていない run はエラーにせず、
/// ログが残っていないことを返します。SSH 接続の run、およびローカルの対応表に無い run
/// と、対応表を読めない run は、GetRunDetail と同じく takt_get_run
/// から行を組み立てます。takt_get_run でも読めなければ、保存場所の読み取りの 失敗を返します。
pub(super) async fn read_updates_response(
  run_logs: &RunLogHandle,
  resolved: &ResolvedProject,
  run_slug: RunSlug,
  after: Option<LogCursor>,
) -> Result<GetRunUpdatesResponse, Status> {
  let local = if resolved.client_config.is_ssh() {
    Err(RunReadFailure::NotListed)
  } else {
    read_run_updates(run_logs, run_slug.clone(), after).await
  };
  match local {
    | Ok(updates) => Ok(run_updates_response(&run_slug, updates)),
    | Err(RunReadFailure::Missing) => {
      Ok(GetRunUpdatesResponse { run_slug: run_slug.as_str().to_owned(), logs_removed: true, ..Default::default() })
    },
    | Err(failure @ (RunReadFailure::NotListed | RunReadFailure::Unreadable(_))) => {
      let updates = remote_run_updates(resolved, &run_slug, after, failure).await?;
      Ok(run_updates_response(&run_slug, updates))
    },
    | Err(failure) => Err(run_read_failure_status(failure)),
  }
}

/// 追記分を応答の形にする。番号と件数は i64 の範囲に収まらない分を上限で止めます。
pub(super) fn run_updates_response(run_slug: &RunSlug, updates: RunUpdates) -> GetRunUpdatesResponse {
  let to_i64 = |value: u64| i64::try_from(value).unwrap_or(i64::MAX);
  GetRunUpdatesResponse {
    run_slug:     run_slug.as_str().to_owned(),
    epoch:        to_i64(updates.epoch),
    reset:        updates.reset,
    lines:        updates
      .lines
      .into_iter()
      .map(|line| RunLogLine { sequence: to_i64(line.sequence), text: line.text })
      .collect(),
    activity:     Some(RunSessionActivity {
      latest_tool:    updates.activity.latest_tool,
      tool_use_count: to_i64(updates.activity.tool_use_count),
      updated_at:     optional_timestamp(updates.activity.updated_at),
    }),
    logs_removed: false,
  }
}

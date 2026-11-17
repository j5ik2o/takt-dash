//! GetRunUpdates の要求の検証と応答の組み立て。
//!
//! 本体ログの追記分の読み取りは [`crate::run_progress`] が行い、ここでは proto の要求値を
//! 続きの位置に変換し、読み取り結果を proto の応答の形にする純粋な変換だけを置きます。

use tonic::Status;

use super::mapping::optional_timestamp;
use crate::{
  domain::RunSlug,
  run_progress::{LogCursor, RunUpdates},
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

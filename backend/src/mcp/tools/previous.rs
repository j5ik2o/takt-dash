//! 前回 snapshot との比較・引き継ぎと、補助読み取りの失敗の縮退判定、道具呼び出しの結果分類。

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};

use super::ToolError;
use crate::{
  domain::{GoalId, RunSlug},
  mcp::{
    client::{CallError, TransportError},
    metrics::ToolCallOutcome,
  },
  model::{ExecutionStatus, GoalEntry, GoalStatus, NotificationEntry, ProjectSnapshot, RunEntry, TerminalReads},
};

/// 輸送エラーのうち期限切れかどうか。
pub(super) fn is_timeout(error: &ToolError) -> bool {
  matches!(error, ToolError::Transport(TransportError::Timeout(_)))
}

/// 補助読み取りの失敗のうち、前回値へ縮退してよい失敗かどうか。
///
/// 道具エラーは従来どおり縮退します。輸送の期限切れも、高負荷で1件の補助読み取りが
/// 遅れただけでプロジェクト全体を失敗させないため、縮退の対象に加えます。それ以外の
/// 輸送エラー（プロセス終了・起動失敗など）は接続自体が壊れている兆候として、
/// 従来どおり全体の失敗にします。
pub(super) fn degrades_to_previous(error: &ToolError) -> bool {
  matches!(error, ToolError::Tool(_)) || is_timeout(error)
}

/// 道具呼び出しの結果を集計用の結果へ分類する。応答の解析エラーは道具呼び出し自体の結果では
/// ないため、ここでは扱いません。
pub(super) fn call_outcome(result: &Result<String, CallError>) -> ToolCallOutcome {
  match result {
    | Ok(_) => ToolCallOutcome::Succeeded,
    | Err(CallError::Tool(_)) => ToolCallOutcome::Failed,
    | Err(CallError::Transport(TransportError::Timeout(_))) => ToolCallOutcome::TimedOut,
    | Err(CallError::Transport(_)) => ToolCallOutcome::Failed,
  }
}

/// 終端（`status == Completed` または `execution_status == Aborted`）のゴールか。
pub(super) fn is_terminal(goal: &GoalEntry) -> bool {
  goal.status == GoalStatus::Completed || goal.execution_status == ExecutionStatus::Aborted
}

/// 補助読み取りの項目 `item` を省ける、前回と同じ状態の終端ゴールかを判定する。
///
/// 前回の同じゴールが同じ終端状態で、かつ前回その状態でこの項目を読めていた場合だけ省けます。
/// 初回観測、状態変化、`AwaitingMerge` などの非終端、前回の終端状態で読み取りに失敗した項目は
/// 読み取り対象のままです。問い合わせへの回答の後に前回の記録から外されたゴールも、
/// 質問を読み直します。
pub(super) fn is_unchanged_terminal_goal(
  goal: &GoalEntry,
  prev: Option<&ProjectSnapshot>,
  item: impl Fn(&TerminalReads) -> &BTreeSet<GoalId>,
) -> bool {
  let Some(prev) = prev else {
    return false;
  };
  let Some(previous) = prev.goals.iter().find(|entry| entry.goal_id == goal.goal_id) else {
    return false;
  };
  previous.status == goal.status
    && previous.execution_status == goal.execution_status
    && is_terminal(goal)
    && item(&prev.terminal_reads).contains(&goal.goal_id)
}

/// 前回 snapshot の同じ runSlug の run を返す。前回が無ければ `None` です。
pub(super) fn previous_run(prev: Option<&ProjectSnapshot>, run_slug: &RunSlug) -> Option<RunEntry> {
  prev.and_then(|snapshot| snapshot.runs.iter().find(|entry| entry.run_slug == run_slug.as_str())).cloned()
}

/// 前回 snapshot の同じゴールの通知と、マージ待ちの起点時刻を引き継ぐ。前回が無ければ何もしません。
pub(super) fn inherit_previous_notifications(
  notifications: &mut Vec<NotificationEntry>,
  awaiting_merge_at: &mut BTreeMap<GoalId, DateTime<Utc>>,
  goal: &GoalEntry,
  prev: Option<&ProjectSnapshot>,
) {
  let Some(prev) = prev else {
    return;
  };
  notifications.extend(prev.notifications.iter().filter(|entry| entry.goal_id == goal.goal_id).cloned());
  if let Some(recorded_at) =
    prev.goals.iter().find(|entry| entry.goal_id == goal.goal_id).and_then(|entry| entry.awaiting_merge_at)
  {
    awaiting_merge_at.insert(goal.goal_id.clone(), recorded_at);
  }
}

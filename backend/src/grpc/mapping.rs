//! model 型から proto のメッセージ型への写像。
//!
//! 純粋な変換だけを置きます。状態ストアの読み取りや I/O はここに置かず、[`super`] の
//! サービスが行います。列挙値・時刻・optional の写像は proto/taktdash.proto の定義に従います。

use chrono::{DateTime, Utc};

use crate::{
  model::{self, ExecutionStatus, GoalStatus, InquiryKind, PendingDecisionType, RunStatus, StepLogEntry, TaskStatus},
  takt_dash::v1,
};

/// proto の Timestamp へ変換する。
pub fn timestamp(time: DateTime<Utc>) -> prost_types::Timestamp {
  prost_types::Timestamp { seconds: time.timestamp(), nanos: time.timestamp_subsec_nanos() as i32 }
}

/// optional の時刻を proto の Timestamp へ変換する。値がなければ未設定のまま返します。
pub fn optional_timestamp(time: Option<DateTime<Utc>>) -> Option<prost_types::Timestamp> {
  time.map(timestamp)
}

/// ゴールの状態を proto の列挙値へ変換する。
pub fn goal_status(status: GoalStatus) -> v1::GoalStatus {
  match status {
    | GoalStatus::Created => v1::GoalStatus::Created,
    | GoalStatus::AwaitingMerge => v1::GoalStatus::AwaitingMerge,
    | GoalStatus::Completed => v1::GoalStatus::Completed,
  }
}

/// ゴールの実行状態を proto の列挙値へ変換する。
pub fn execution_status(status: ExecutionStatus) -> v1::ExecutionStatus {
  match status {
    | ExecutionStatus::Active => v1::ExecutionStatus::Active,
    | ExecutionStatus::Paused => v1::ExecutionStatus::Paused,
    | ExecutionStatus::Aborted => v1::ExecutionStatus::Aborted,
  }
}

/// タスクの状態を proto の列挙値へ変換する。
pub fn task_status(status: TaskStatus) -> v1::TaskStatus {
  match status {
    | TaskStatus::Pending => v1::TaskStatus::Pending,
    | TaskStatus::Running => v1::TaskStatus::Running,
    | TaskStatus::Completed => v1::TaskStatus::Completed,
    | TaskStatus::Failed => v1::TaskStatus::Failed,
    | TaskStatus::Exceeded => v1::TaskStatus::Exceeded,
    | TaskStatus::PrFailed => v1::TaskStatus::PrFailed,
  }
}

/// run の状態を proto の列挙値へ変換する。値のない未知の状態は未指定へ写します。
pub fn run_status(status: RunStatus) -> v1::RunStatus {
  match status {
    | RunStatus::Running => v1::RunStatus::Running,
    | RunStatus::Completed => v1::RunStatus::Completed,
    | RunStatus::Aborted => v1::RunStatus::Aborted,
    | RunStatus::Failed => v1::RunStatus::Failed,
  }
}

/// 出来事の種類を proto の列挙値へ変換する。
pub fn activity_kind(kind: model::EventKind) -> v1::ActivityKind {
  match kind {
    | model::EventKind::GoalCreated => v1::ActivityKind::GoalCreated,
    | model::EventKind::RunStarted => v1::ActivityKind::RunStarted,
    | model::EventKind::TaskCompleted => v1::ActivityKind::TaskCompleted,
    | model::EventKind::Inquiry => v1::ActivityKind::Inquiry,
    | model::EventKind::AwaitingMerge => v1::ActivityKind::AwaitingMerge,
    | model::EventKind::Paused => v1::ActivityKind::Paused,
    | model::EventKind::Blocked => v1::ActivityKind::Blocked,
    | model::EventKind::ConnectionLost => v1::ActivityKind::ConnectionLost,
    | model::EventKind::ConnectionRestored => v1::ActivityKind::ConnectionRestored,
  }
}

/// プロジェクトの接続状態を proto の列挙値へ変換する。まだ読めていない場合は未指定です。
pub fn connection_state(state: Option<model::ConnectionState>) -> v1::ConnectionState {
  match state {
    | Some(model::ConnectionState::Connected) => v1::ConnectionState::Connected,
    | Some(model::ConnectionState::Delayed) => v1::ConnectionState::Delayed,
    | Some(model::ConnectionState::Unresponsive) => v1::ConnectionState::Unresponsive,
    | None => v1::ConnectionState::Unspecified,
  }
}

/// 判断待ちの種類を proto の列挙値へ変換する。
pub fn pending_decision_type(kind: PendingDecisionType) -> v1::PendingDecisionType {
  match kind {
    | PendingDecisionType::Inquiry => v1::PendingDecisionType::Inquiry,
    | PendingDecisionType::AwaitingMerge => v1::PendingDecisionType::AwaitingMerge,
  }
}

/// 判断待ちの問い合わせの種類を proto の列挙値へ変換する。
///
/// 人のマージ待ちのように種類を持たない判断待ちは、proto の定義どおり未指定へ写します。
pub fn inquiry_kind(kind: Option<InquiryKind>) -> v1::InquiryKind {
  match kind {
    | Some(InquiryKind::Question) => v1::InquiryKind::Question,
    | Some(InquiryKind::OperationRequest) => v1::InquiryKind::OperationRequest,
    | None => v1::InquiryKind::Unspecified,
  }
}

/// ステップの記録を proto の StepLog へ変換する。
pub fn step_log(entry: &StepLogEntry) -> v1::StepLog {
  v1::StepLog {
    step_name: entry.step_name.clone(),
    persona:   entry.persona.clone(),
    status:    entry.status.clone(),
    content:   entry.content.clone(),
    timestamp: Some(timestamp(entry.timestamp)),
    iteration: entry.iteration,
  }
}

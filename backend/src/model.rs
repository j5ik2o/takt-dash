//! takt-mcp から集約した内部状態の型。
//!
//! これらの型は lib の公開 API の一部であり、後続のタスクの gRPC サーバーが
//! 読み取り系 API の応答を組み立てるために使います。

use chrono::{DateTime, Utc};

use crate::domain::{GoalId, InquiryId, PullRequestNumber, RegistrationRevision, RunSlug, TaskName};

/// ゴールの状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalStatus {
  Created,
  AwaitingMerge,
  Completed,
}

/// ゴールの実行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionStatus {
  Active,
  Paused,
  Aborted,
}

/// タスクの状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
  Pending,
  Running,
  Completed,
  Failed,
  Exceeded,
  PrFailed,
}

/// アクティビティの出来事の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventKind {
  GoalCreated,
  RunStarted,
  TaskCompleted,
  Inquiry,
  AwaitingMerge,
  Paused,
  Blocked,
  ConnectionLost,
}

impl EventKind {
  /// SQLite に保存するときの種類の文字列。
  pub fn as_str(self) -> &'static str {
    match self {
      | EventKind::GoalCreated => "goal_created",
      | EventKind::RunStarted => "run_started",
      | EventKind::TaskCompleted => "task_completed",
      | EventKind::Inquiry => "inquiry",
      | EventKind::AwaitingMerge => "awaiting_merge",
      | EventKind::Paused => "paused",
      | EventKind::Blocked => "blocked",
      | EventKind::ConnectionLost => "connection_lost",
    }
  }

  /// 保存済みの種類の文字列から復元する。
  pub fn from_stored(value: &str) -> Option<Self> {
    match value {
      | "goal_created" => Some(EventKind::GoalCreated),
      | "run_started" => Some(EventKind::RunStarted),
      | "task_completed" => Some(EventKind::TaskCompleted),
      | "inquiry" => Some(EventKind::Inquiry),
      | "awaiting_merge" => Some(EventKind::AwaitingMerge),
      | "paused" => Some(EventKind::Paused),
      | "blocked" => Some(EventKind::Blocked),
      | "connection_lost" => Some(EventKind::ConnectionLost),
      | _ => None,
    }
  }
}

/// 判断待ちの種類。未回答の問い合わせ・人のマージ待ちのゴール・ゴール登録の承認待ちを使います。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingDecisionType {
  Inquiry,
  AwaitingMerge,
  /// ゴール登録の承認待ち。
  GoalRegistration,
}

/// 判断待ちの問い合わせの種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InquiryKind {
  Question,
  OperationRequest,
}

/// ゴールの一覧に含まれるゴール。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalEntry {
  pub goal_id:             GoalId,
  pub status:              GoalStatus,
  pub execution_status:    ExecutionStatus,
  pub objective:           String,
  pub integration_branch:  String,
  pub pull_request_number: Option<PullRequestNumber>,
  pub pull_request_url:    Option<String>,
  /// takt_get_goal の通知（path=notifications）から得た、awaiting_merge の種類の通知の記録時刻。
  pub awaiting_merge_at:   Option<DateTime<Utc>>,
  /// 一時停止の理由。実行状態が paused で理由が報告されたときだけ持ちます。
  pub paused_reason:       Option<String>,
}

/// タスクの一覧に含まれるタスク。配列順は tasks.yaml の順を保ちます。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskEntry {
  pub name:         TaskName,
  pub goal_id:      Option<GoalId>,
  pub status:       TaskStatus,
  pub started_at:   Option<DateTime<Utc>>,
  pub run_slug:     Option<RunSlug>,
  pub current_step: Option<String>,
  /// ゴールの仕上げタスクかどうか。
  pub merge_task:   bool,
}

/// ゴールに属する問い合わせ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InquiryEntry {
  pub inquiry_id: InquiryId,
  pub goal_id:    Option<GoalId>,
  pub kind:       InquiryKind,
  pub body:       String,
  pub options:    Vec<String>,
  pub answered:   bool,
  pub created_at: DateTime<Utc>,
}

/// takt_get_goal の通知（path=notifications）から得た通知。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationEntry {
  pub notification_id: String,
  /// 通知を読んだゴール。道具エラー時の前回値保持で、ゴール単位に引き継ぐために使います。
  pub goal_id:         GoalId,
  pub kind:            String,
  pub body:            String,
  pub recorded_at:     DateTime<Utc>,
}

/// run の状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
  Running,
  Completed,
  Aborted,
  Failed,
}

impl TryFrom<&str> for RunStatus {
  type Error = crate::domain::DomainError;

  fn try_from(value: &str) -> Result<Self, Self::Error> {
    match value {
      | "running" => Ok(RunStatus::Running),
      | "completed" => Ok(RunStatus::Completed),
      | "aborted" => Ok(RunStatus::Aborted),
      | "failed" => Ok(RunStatus::Failed),
      | _ => Err(crate::domain::DomainError::Semantics("run の状態")),
    }
  }
}

/// run の実行で通ったステップ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunStepEntry {
  pub step_name:  String,
  pub model:      String,
  pub started_at: DateTime<Utc>,
  /// 終了時刻。継続中のステップは持ちません。
  pub ended_at:   Option<DateTime<Utc>>,
}

/// run のレポートの要約。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportEntry {
  pub filename:   String,
  pub size_bytes: i64,
}

/// takt_get_run から得た run の情報。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunEntry {
  pub run_slug:                String,
  pub session_id:              String,
  pub task_name:               TaskName,
  pub goal_id:                 Option<GoalId>,
  pub summary:                 String,
  pub status:                  RunStatus,
  pub current_step:            Option<String>,
  pub started_at:              DateTime<Utc>,
  pub current_step_started_at: Option<DateTime<Utc>>,
  pub last_log_at:             DateTime<Utc>,
  pub steps:                   Vec<RunStepEntry>,
  pub reports:                 Vec<ReportEntry>,
}

/// takt_get_run から得た、run 詳細の本文。一覧の投影では使わない詳細情報です。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunDetail {
  pub run_slug: RunSlug,
  pub session_id: String,
  pub task_name: Option<TaskName>,
  pub goal_id: Option<GoalId>,
  pub task_text: String,
  pub summary: String,
  pub workflow: String,
  pub status: RunStatus,
  pub started_at: DateTime<Utc>,
  pub current_step: Option<String>,
  pub current_iteration: Option<i32>,
  pub current_step_started_at: Option<DateTime<Utc>>,
  pub current_step_execution_count: Option<i32>,
  pub phase: Option<i32>,
  pub step_logs: Vec<StepLogEntry>,
  pub log_tail: String,
  pub reports: Vec<RunReportEntry>,
  pub instructions: Vec<RunInstructionEntry>,
  pub last_log_at: DateTime<Utc>,
}

/// run のステップの記録。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepLogEntry {
  pub step_name: String,
  pub persona:   String,
  pub status:    String,
  pub content:   String,
  pub timestamp: DateTime<Utc>,
  pub iteration: i32,
}

/// run のレポートの一覧項目と本文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunReportEntry {
  pub filename:   String,
  pub content:    String,
  pub size_bytes: i64,
}

/// run への指示書。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunInstructionEntry {
  pub instruction_id: i32,
  pub issued_at:      DateTime<Utc>,
  pub state:          String,
  pub content:        String,
}

/// 1回の読み取りで得た、1プロジェクト分の状態。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectSnapshot {
  pub goals:         Vec<GoalEntry>,
  pub tasks:         Vec<TaskEntry>,
  pub inquiries:     Vec<InquiryEntry>,
  pub runs:          Vec<RunEntry>,
  pub notifications: Vec<NotificationEntry>,
  /// ゴール登録の承認待ち。承認待ちが無ければ空です。
  pub proposals:     Vec<RegistrationProposalEntry>,
}

impl ProjectSnapshot {
  /// 空の状態。対象プロジェクトにゴールもタスクもない場合に使います。
  pub fn empty() -> Self {
    ProjectSnapshot {
      goals:         Vec::new(),
      tasks:         Vec::new(),
      inquiries:     Vec::new(),
      runs:          Vec::new(),
      notifications: Vec::new(),
      proposals:     Vec::new(),
    }
  }
}

/// 検出した出来事。SQLite へ記録する単位です。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityEvent {
  pub occurred_at: DateTime<Utc>,
  pub server_id:   String,
  pub project:     Option<String>,
  pub kind:        EventKind,
  pub body:        String,
  pub run_slug:    Option<RunSlug>,
}

/// 判断待ちの項目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingDecision {
  pub decision_type:       PendingDecisionType,
  pub server_id:           String,
  pub project:             String,
  pub goal_id:             Option<String>,
  pub inquiry_kind:        Option<InquiryKind>,
  pub created_at:          DateTime<Utc>,
  pub body:                String,
  pub options:             Vec<String>,
  pub pull_request_number: Option<PullRequestNumber>,
  pub pull_request_url:    Option<String>,
  pub stale:               bool,
  /// 問い合わせの識別子。マージ待ちと承認待ちは問い合わせではないため持ちません。
  pub inquiry_id:          Option<InquiryId>,
  /// ゴール登録の承認待ちの revision。それ以外の種別は持ちません。
  pub revision:            Option<RegistrationRevision>,
}

/// ゴール登録の承認待ち。提案そのものの保持と射影に使います。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationProposalEntry {
  pub revision:             RegistrationRevision,
  pub repository_directory: String,
  /// 提案の要約。判断待ちの本文として表示します。
  pub summary:              String,
}

/// 判断待ちの一覧の排他的な見方。問い合わせと人のマージ待ちを別々に取り出せます。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingDecisionFilter {
  All,
  Inquiry,
  AwaitingMerge,
}

/// 状態ストアが返す、1プロジェクト分の投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectView {
  pub name:         String,
  pub about:        String,
  pub server_id:    String,
  pub connected:    bool,
  /// 最後に読めた時刻。応答がない間も保持します。
  pub last_read_at: Option<DateTime<Utc>>,
  pub goals:        Vec<GoalEntry>,
  pub tasks:        Vec<TaskEntry>,
  pub runs:         Vec<RunEntry>,
  /// 状態の元になった snapshot。プロジェクトがまだ一度も読めていない場合は None。
  pub snapshot:     Option<ProjectSnapshot>,
}

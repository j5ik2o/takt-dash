//! takt-mcp から集約した内部状態の型。
//!
//! これらの型は lib の公開 API の一部であり、後続のタスクの gRPC サーバーが
//! 読み取り系 API の応答を組み立てるために使います。

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};

use crate::domain::{GoalId, InquiryId, ProjectName, PullRequestNumber, RegistrationRevision, RunSlug, TaskName};

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

/// プロジェクトの接続状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
  /// 最後の読み取りに成功している。
  Connected,
  /// 直近の読み取りに失敗したが、遅延のしきい値には達していない。
  Delayed,
  /// 連続失敗と経過時間のしきい値を超えて応答がない。
  Unresponsive,
}

/// 接続状態の遷移。出来事の検出と投影に使います。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectionTransition {
  /// 遷移前の状態。まだ一度も読めていない場合は None。
  pub previous: Option<ConnectionState>,
  /// 遷移後の状態。スロットが無い場合は None。
  pub next:     Option<ConnectionState>,
  /// 応答なしから接続中へ戻ったときの、応答なしだった時間。
  pub outage:   Option<chrono::Duration>,
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
  ConnectionRestored,
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
      | EventKind::ConnectionRestored => "connection_restored",
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
      | "connection_restored" => Some(EventKind::ConnectionRestored),
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
  /// 受信前に objective の続きが省略されているか。
  pub objective_truncated: bool,
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
  pub name:            TaskName,
  pub goal_id:         Option<GoalId>,
  pub status:          TaskStatus,
  pub started_at:      Option<DateTime<Utc>>,
  pub run_slug:        Option<RunSlug>,
  /// takt_list_tasks の sourceRunSlug。完了タスクが元 run を指すときに持ちます。
  pub source_run_slug: Option<String>,
  pub current_step:    Option<String>,
  /// ゴールの仕上げタスクかどうか。
  pub merge_task:      bool,
}

impl GoalEntry {
  pub(crate) fn objective_display(&self) -> String {
    if self.objective_truncated {
      format!("{}…", self.objective)
    } else {
      self.objective.clone()
    }
  }
}

/// takt_get_goal から得たゴールの詳細。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalDetail {
  pub goal_id:             GoalId,
  pub status:              GoalStatus,
  pub execution_status:    ExecutionStatus,
  pub objective:           String,
  pub out_of_scope:        Vec<String>,
  pub acceptance_criteria: Vec<String>,
  pub constraints:         Vec<String>,
  pub specifications:      BTreeMap<String, String>,
  pub branch:              String,
  pub integration_branch:  String,
  pub work_log:            Vec<GoalWorkLogEntry>,
  pub questions:           Vec<GoalQuestionEntry>,
  pub tasks:               Vec<GoalTaskEntry>,
}

/// ゴールの作業記録。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalWorkLogEntry {
  pub work_log_id: String,
  pub recorded_at: DateTime<Utc>,
  pub summary:     String,
}

/// ゴールに対する manager の質問と回答。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalQuestionEntry {
  pub question_id:     String,
  pub kind:            String,
  pub status:          String,
  pub body:            String,
  pub options:         Vec<String>,
  pub answer_text:     Option<String>,
  pub answer_decision: Option<String>,
  pub answer_report:   Option<String>,
  pub created_at:      Option<DateTime<Utc>>,
  pub answered_at:     Option<DateTime<Utc>>,
}

/// ゴールに紐づくタスク。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalTaskEntry {
  pub task_name: TaskName,
  pub status:    TaskStatus,
  pub run_slug:  Option<RunSlug>,
}

/// takt_list_tasks から得たタスクの詳細。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskDetail {
  pub project:             ProjectName,
  pub task_name:           TaskName,
  pub goal_id:             Option<GoalId>,
  pub status:              TaskStatus,
  pub summary:             String,
  pub created_at:          Option<DateTime<Utc>>,
  pub started_at:          Option<DateTime<Utc>>,
  pub completed_at:        Option<DateTime<Utc>>,
  pub run_slug:            Option<RunSlug>,
  pub linked_run_slugs:    Vec<RunSlug>,
  pub pull_request_number: Option<PullRequestNumber>,
  pub branch:              Option<String>,
  pub pull_request_url:    Option<String>,
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

/// 実行中の run の情報。状態とステップの記録は run の保存場所から読みます。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunEntry {
  pub run_slug:                String,
  pub session_id:              String,
  pub task_name:               TaskName,
  pub goal_id:                 Option<GoalId>,
  pub summary:                 String,
  pub status:                  RunStatus,
  pub current_step:            Option<String>,
  pub current_step_number:     Option<i32>,
  pub total_steps:             Option<i32>,
  pub current_iteration:       Option<i32>,
  pub started_at:              Option<DateTime<Utc>>,
  pub current_step_started_at: Option<DateTime<Utc>>,
  pub last_log_at:             Option<DateTime<Utc>>,
  pub steps:                   Vec<RunStepEntry>,
  pub reports:                 Vec<ReportEntry>,
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

/// 1回の読み取りで得た、1プロジェクト分の状態。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectSnapshot {
  pub goals:          Vec<GoalEntry>,
  pub tasks:          Vec<TaskEntry>,
  pub inquiries:      Vec<InquiryEntry>,
  pub runs:           Vec<RunEntry>,
  pub notifications:  Vec<NotificationEntry>,
  /// ゴール登録の承認待ち。承認待ちが無ければ空です。
  pub proposals:      Vec<RegistrationProposalEntry>,
  /// 終端状態のゴールで、補助読み取りに成功した項目。
  pub terminal_reads: TerminalReads,
}

/// 終端状態（完了または中止）のゴールのうち、その状態で補助読み取りに成功したゴール。
///
/// 状態が変わらない終端のゴールは、ここに載っている項目だけ読み取りを省きます。終端状態で読み取りに
/// 失敗した項目は載せず、次の周期で読み直します。質問と通知は別々に扱います。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TerminalReads {
  /// 質問（takt_list_goal_questions）を読めたゴール。
  pub questions:     BTreeSet<GoalId>,
  /// 通知（takt_get_goal の notifications）を読めたゴール。
  pub notifications: BTreeSet<GoalId>,
}

impl ProjectSnapshot {
  /// 空の状態。対象プロジェクトにゴールもタスクもない場合に使います。
  pub fn empty() -> Self {
    ProjectSnapshot {
      goals:          Vec::new(),
      tasks:          Vec::new(),
      inquiries:      Vec::new(),
      runs:           Vec::new(),
      notifications:  Vec::new(),
      proposals:      Vec::new(),
      terminal_reads: TerminalReads::default(),
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
  /// マージ待ちのゴールの仕上げタスクの最後の状態。それ以外の種別は持ちません。
  pub merge_task_last:     Option<TaskStatus>,
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
  pub name:             String,
  pub about:            String,
  pub server_id:        String,
  /// 同時実行の上限（プロジェクトの .takt/config.yaml 由来）。
  pub concurrency:      i32,
  pub connected:        bool,
  /// 接続状態。まだ一度も読めていない場合は None。
  pub connection_state: Option<ConnectionState>,
  /// 最後に読めた時刻。応答がない間も保持します。
  pub last_read_at:     Option<DateTime<Utc>>,
  pub goals:            Vec<GoalEntry>,
  pub tasks:            Vec<TaskEntry>,
  pub runs:             Vec<RunEntry>,
  /// 状態の元になった snapshot。プロジェクトがまだ一度も読めていない場合は None。
  pub snapshot:         Option<ProjectSnapshot>,
}

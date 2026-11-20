//! 読み取り専用と書き込み専用の道具のまとまり（公開する型）。
//!
//! 実装本体は [`super::ToolSetCore`] にあり、ここでは用途ごとに公開するメソッドを
//! 絞った型だけを定義します。読み取りレーンの接続からは書き込みの道具を、書き込み
//! レーンの接続からは読み取りの道具を呼べないことを、型で担保します。

use std::path::PathBuf;

use serde_json::Value;

use super::{run::RemoteRun, ApproveGoalRegistrationResult, MergeGoalPrResult, ToolError, ToolSetCore};
use crate::{
  domain::{
    GoalId, InquiryId, InstructionId, MergeUntil, ProjectName, PullRequestNumber, RegistrationRevision, ReportFilename,
    RunSlug, TaskName,
  },
  mcp::{client::ToolCaller, metrics::ToolCallMetrics},
  model::{GoalDetail, ProjectSnapshot, RegistrationProposalEntry, TaskDetail},
  run_log::{Instruction, RunContent},
  run_progress::RunLogHandle,
};

/// 読み取り専用の道具のまとまり。読み取りレーン（定期読み取り・画面読み取り）の接続で使います。
///
/// 書き込みの道具メソッドを持たないため、読み取りレーンのコードから takt の状態を
/// 変更する道具を呼ぶことはできません。
pub struct ReadToolSet<'a> {
  core: ToolSetCore<'a>,
}

impl<'a> ReadToolSet<'a> {
  pub fn new(client: &'a mut dyn ToolCaller, cwd: PathBuf) -> Self {
    ReadToolSet { core: ToolSetCore::new(client, cwd) }
  }

  /// この道具のまとまりが記録した呼び出しの集計を取り出す。
  pub(crate) fn take_metrics(&mut self) -> ToolCallMetrics {
    self.core.take_metrics()
  }

  /// プロジェクトの現在の状態を1回分読み取る。poller の定期読み取りが使います。
  pub async fn read_snapshot(
    &mut self,
    previous: Option<&ProjectSnapshot>,
    run_logs: &RunLogHandle,
  ) -> Result<ProjectSnapshot, ToolError> {
    self.core.read_snapshot(previous, run_logs).await
  }

  /// takt_get_run で run の状態を読む。画面の読み取りのフォールバックが使います。
  pub async fn read_remote_run(&mut self, run_slug: &RunSlug) -> Result<RemoteRun, ToolError> {
    self.core.read_remote_run(run_slug).await
  }

  /// ゴール詳細の読み取り（画面用）。
  pub async fn read_goal_detail(&mut self, goal_id: &GoalId) -> Result<GoalDetail, ToolError> {
    self.core.read_goal_detail(goal_id).await
  }

  /// タスク詳細の読み取り（画面用）。
  pub async fn read_task_detail(
    &mut self,
    project: &ProjectName,
    task_name: &TaskName,
    goal_id: Option<&GoalId>,
  ) -> Result<Option<TaskDetail>, ToolError> {
    self.core.read_task_detail(project, task_name, goal_id).await
  }

  /// ゴール登録案の一覧を読む。
  pub async fn list_goal_registration_proposals(&mut self) -> Result<Vec<RegistrationProposalEntry>, ToolError> {
    self.core.list_goal_registration_proposals().await
  }

  /// takt_get_run の view report からレポートの本文を読む（画面用）。
  pub async fn read_remote_report(
    &mut self,
    run_slug: &RunSlug,
    filename: &ReportFilename,
    max_bytes: u64,
  ) -> Result<RunContent, ToolError> {
    self.core.read_remote_report(run_slug, filename, max_bytes).await
  }

  /// takt_get_run の view instructions から指示書の一覧を読む（画面用）。
  pub async fn read_remote_instructions(&mut self, run_slug: &RunSlug) -> Result<Vec<Instruction>, ToolError> {
    self.core.read_remote_instructions(run_slug).await
  }

  /// takt_get_run の view task からタスク本文を読む（画面用）。
  pub async fn read_remote_task_text(&mut self, run_slug: &RunSlug) -> Result<String, ToolError> {
    self.core.read_remote_task_text(run_slug).await
  }

  /// takt_get_run の view instructions から指示書の本文を読む（画面用）。
  pub async fn read_remote_instruction(
    &mut self,
    run_slug: &RunSlug,
    instruction_id: InstructionId,
    max_bytes: u64,
  ) -> Result<RunContent, ToolError> {
    self.core.read_remote_instruction(run_slug, instruction_id, max_bytes).await
  }
}

/// 書き込み専用の道具のまとまり。書き込みレーンの接続で使います。
///
/// 読み取りの道具メソッドを持たないため、書き込みレーンのコードから `read_snapshot`
/// のような重い読み取りを呼ぶことはできません。
pub struct WriteToolSet<'a> {
  core: ToolSetCore<'a>,
}

impl<'a> WriteToolSet<'a> {
  pub fn new(client: &'a mut dyn ToolCaller, cwd: PathBuf) -> Self {
    WriteToolSet { core: ToolSetCore::new(client, cwd) }
  }

  /// managerからの質問へ回答する。
  pub async fn answer_goal_question(
    &mut self,
    goal_id: &GoalId,
    question_id: &InquiryId,
    answer: Value,
  ) -> Result<InquiryId, ToolError> {
    self.core.answer_goal_question(goal_id, question_id, answer).await
  }

  /// ゴール登録案を承認する。
  pub async fn approve_goal_registration(
    &mut self,
    revision: RegistrationRevision,
  ) -> Result<ApproveGoalRegistrationResult, ToolError> {
    self.core.approve_goal_registration(revision).await
  }

  /// ゴール登録案を却下する。
  pub async fn reject_goal_registration(
    &mut self,
    revision: RegistrationRevision,
  ) -> Result<RegistrationRevision, ToolError> {
    self.core.reject_goal_registration(revision).await
  }

  /// ゴールのプルリクエストをマージする。
  pub async fn merge_goal_pr(
    &mut self,
    goal_id: &GoalId,
    pr_number: PullRequestNumber,
    until: MergeUntil,
  ) -> Result<MergeGoalPrResult, ToolError> {
    self.core.merge_goal_pr(goal_id, pr_number, until).await
  }
}

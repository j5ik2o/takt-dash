//! 画面の要求時の読み取りと書き込みに使う、プロジェクトの接続先の解決。
//!
//! 設定から対象プロジェクトの読み取り接続先（`server`）と書き込み接続先
//! （`write_server`）を解決し、読み取り元（常駐 HTTP サーバーか stdio の
//! 子プロセスか）を組み立てる。

use std::path::PathBuf;

use tonic::Status;

use super::{TaktDashServiceImpl, READ_TIMEOUT};
use crate::{
  config::{load_config, Config, McpConnection},
  domain::{
    GoalId, InquiryId, MergeUntil, ProjectName, PullRequestNumber, RegistrationRevision, RunSlug, ScreenReadLanes,
    ServerId, TaskName,
  },
  mcp::{
    ApproveGoalRegistrationResult, ClientHandle, HttpReadHandle, MergeGoalPrResult, ReadToolSet, RemoteRun, ToolError,
    WriteToolSet,
  },
  model::{GoalDetail, ProjectView, TaskDetail},
};

/// 設定から解決した、対象サーバーの接続方法とプロジェクトの cwd。
pub(super) struct ResolvedProject {
  pub(super) server_id:         ServerId,
  pub(super) write_server_id:   ServerId,
  pub(super) project:           ProjectName,
  /// 読み取り接続先（`server` の接続方法）。
  pub(super) connection:        McpConnection,
  /// 書き込み接続先（`write_server` の接続方法）。
  pub(super) write_connection:  McpConnection,
  pub(super) cwd:               PathBuf,
  /// 画面読み取りレーンの本数。設定から解決します。
  pub(super) screen_read_lanes: ScreenReadLanes,
}

/// 状態ストアから解決した run のプロジェクトと、設定から解決したその cwd。
pub(super) struct RunTarget {
  pub(super) view:     ProjectView,
  pub(super) resolved: ResolvedProject,
}

/// 読み込み済みの設定から、`server_id` と `project` の接続方法と cwd を解決する。
///
/// 設定に無いサーバー・プロジェクトは `unavailable` で返し、takt-mcp を起動しません。
/// 読み取りは `server`、書き込みは `write_server` の接続方法を使います。
pub(super) fn resolve_project_in(
  config: &Config,
  server_id: &ServerId,
  project: &ProjectName,
) -> Result<ResolvedProject, Status> {
  let entry = config
    .projects
    .iter()
    .find(|entry| entry.name == project.as_str() && entry.server == server_id.as_str())
    .ok_or_else(|| Status::unavailable(format!("プロジェクト {} の cwd が設定にありません。", project.as_str())))?;
  let server = config
    .servers
    .get(entry.server.as_str())
    .ok_or_else(|| Status::unavailable(format!("サーバー {} の起動方法が設定にありません。", entry.server)))?
    .connection();
  let write_connection = config
    .servers
    .get(entry.write_server.as_str())
    .ok_or_else(|| {
      Status::unavailable(format!("書き込み用サーバー {} の起動方法が設定にありません。", entry.write_server))
    })?
    .connection();
  let write_server_id = ServerId::try_from(entry.write_server.as_str())
    .map_err(|error| Status::internal(format!("設定の書き込み用サーバー ID が不正です: {error}")))?;
  Ok(ResolvedProject {
    server_id: server_id.clone(),
    write_server_id,
    project: project.clone(),
    connection: server,
    write_connection,
    cwd: entry.cwd.clone(),
    screen_read_lanes: config.screen_read_lanes,
  })
}

impl TaktDashServiceImpl {
  /// 設定を読み、`server_id` と `project` から対象サーバーの起動方法と cwd を解決する。
  ///
  /// 設定に無いサーバー・プロジェクトは `unavailable` で返し、takt-mcp を起動しません。
  pub(super) fn resolve_project(&self, server_id: &ServerId, project: &ProjectName) -> Result<ResolvedProject, Status> {
    let config = load_config(&self.config_path)
      .map_err(|error| Status::unavailable(format!("設定を読み込めませんでした: {error}")))?;
    resolve_project_in(&config, server_id, project)
  }

  /// 書き込み RPC の共通の入口。型で検証済みの対象を解決する。
  pub(super) fn resolve_write_target(
    &self,
    server_id: &ServerId,
    project: &ProjectName,
  ) -> Result<ResolvedProject, Status> {
    self.resolve_project(server_id, project)
  }

  /// 状態ストアで run のプロジェクトを探し、設定からその cwd を解決する。takt-mcp は起動しません。
  pub(super) fn resolve_run_target(&self, run_slug: &RunSlug) -> Result<RunTarget, Status> {
    let view = self
      .store
      .find_run_project(run_slug)
      .ok_or_else(|| Status::not_found(format!("runSlug {} は状態ストアにありません。", run_slug.as_str())))?;
    let server_id = ServerId::try_from(view.server_id.as_str())
      .map_err(|error| Status::internal(format!("状態ストアの server_id が不正です: {error}")))?;
    let project = ProjectName::try_from(view.name.as_str())
      .map_err(|error| Status::internal(format!("状態ストアの project が不正です: {error}")))?;
    let resolved = self.resolve_project(&server_id, &project)?;
    Ok(RunTarget { view, resolved })
  }

  /// 読み取り RPC 用の読み取り元を作る。HTTP 接続先は常駐サーバーのハンドル、stdio 接続先は
  /// 画面読み取りレーンのクライアントを使います。
  pub(super) fn read_source(&self, resolved: &ResolvedProject) -> ReadSource {
    match &resolved.connection {
      | McpConnection::Http(_) => {
        ReadSource::Http(Box::new(HttpReadHandle::new(resolved.connection.clone(), READ_TIMEOUT)))
      },
      | McpConnection::Stdio(_) => ReadSource::Stdio(self.screen_handle(resolved)),
    }
  }

  /// 書き込み RPC 用の書き込み元を作る。HTTP 接続先は常駐サーバーのハンドル、stdio 接続先は
  /// 書き込みレーンのクライアントを使います。
  pub(super) fn write_source(&self, resolved: &ResolvedProject) -> WriteSource {
    match &resolved.write_connection {
      | McpConnection::Http(_) => {
        WriteSource::Http(Box::new(HttpReadHandle::new_for_write(resolved.write_connection.clone(), READ_TIMEOUT)))
      },
      | McpConnection::Stdio(_) => WriteSource::Stdio(self.write_handle(resolved)),
    }
  }
}

/// 画面の要求時の読み取り用の読み取り元。
///
/// HTTP 接続先は常駐サーバーのハンドルから読み、stdio 接続先は画面読み取りレーンの
/// 子プロセスから読みます。どちらの接続でも、読み取りの道具のまとまりは同じです。
pub(super) enum ReadSource {
  Http(Box<HttpReadHandle>),
  Stdio(ClientHandle),
}

impl ReadSource {
  /// takt_get_run で run の状態を読む。
  pub(super) async fn read_remote_run(
    &mut self,
    cwd: &std::path::Path,
    run_slug: &RunSlug,
  ) -> Result<RemoteRun, ToolError> {
    match self {
      | ReadSource::Http(handle) => handle.read_remote_run(cwd, run_slug).await,
      | ReadSource::Stdio(handle) => {
        let mut client = handle.lock().await;
        ReadToolSet::new(&mut *client, cwd.to_path_buf()).read_remote_run(run_slug).await
      },
    }
  }

  /// 詳細用に、指示書の一覧とタスク本文まで takt_get_run から読む。
  pub(super) async fn read_remote_run_detail(
    &mut self,
    cwd: &std::path::Path,
    run_slug: &RunSlug,
  ) -> Result<RemoteRun, ToolError> {
    match self {
      | ReadSource::Http(handle) => handle.read_remote_run_detail(cwd, run_slug).await,
      | ReadSource::Stdio(handle) => {
        let mut client = handle.lock().await;
        let mut toolset = ReadToolSet::new(&mut *client, cwd.to_path_buf());
        let mut remote = toolset.read_remote_run(run_slug).await?;
        remote.instructions = toolset.read_remote_instructions(run_slug).await?;
        if remote.task_text.is_empty() {
          remote.task_text = toolset.read_remote_task_text(run_slug).await?;
        }
        Ok(remote)
      },
    }
  }

  /// takt_get_run の view report からレポートの本文を読む。
  pub(super) async fn read_remote_report(
    &mut self,
    cwd: &std::path::Path,
    run_slug: &RunSlug,
    filename: &crate::domain::ReportFilename,
    max_bytes: u64,
  ) -> Result<crate::run_log::RunContent, ToolError> {
    match self {
      | ReadSource::Http(handle) => handle.read_remote_report(cwd, run_slug, filename, max_bytes).await,
      | ReadSource::Stdio(handle) => {
        let mut client = handle.lock().await;
        ReadToolSet::new(&mut *client, cwd.to_path_buf()).read_remote_report(run_slug, filename, max_bytes).await
      },
    }
  }

  /// takt_get_run の view instructions から指示書の本文を読む。
  pub(super) async fn read_remote_instruction(
    &mut self,
    cwd: &std::path::Path,
    run_slug: &RunSlug,
    instruction_id: crate::domain::InstructionId,
    max_bytes: u64,
  ) -> Result<crate::run_log::RunContent, ToolError> {
    match self {
      | ReadSource::Http(handle) => handle.read_remote_instruction(cwd, run_slug, instruction_id, max_bytes).await,
      | ReadSource::Stdio(handle) => {
        let mut client = handle.lock().await;
        ReadToolSet::new(&mut *client, cwd.to_path_buf())
          .read_remote_instruction(run_slug, instruction_id, max_bytes)
          .await
      },
    }
  }

  /// ゴールの詳細を読む。
  pub(super) async fn read_goal_detail(
    &mut self,
    cwd: &std::path::Path,
    goal_id: &GoalId,
  ) -> Result<GoalDetail, ToolError> {
    match self {
      | ReadSource::Http(handle) => handle.read_goal_detail(cwd, goal_id).await,
      | ReadSource::Stdio(handle) => {
        let mut client = handle.lock().await;
        ReadToolSet::new(&mut *client, cwd.to_path_buf()).read_goal_detail(goal_id).await
      },
    }
  }

  /// タスクの詳細を読む。
  pub(super) async fn read_task_detail(
    &mut self,
    cwd: &std::path::Path,
    project: &ProjectName,
    task_name: &TaskName,
    goal_id: Option<&GoalId>,
  ) -> Result<Option<TaskDetail>, ToolError> {
    match self {
      | ReadSource::Http(handle) => handle.read_task_detail(cwd, project, task_name, goal_id).await,
      | ReadSource::Stdio(handle) => {
        let mut client = handle.lock().await;
        ReadToolSet::new(&mut *client, cwd.to_path_buf()).read_task_detail(project, task_name, goal_id).await
      },
    }
  }
}

/// 画面の要求時の書き込み用の書き込み元。
///
/// HTTP 接続先は常駐サーバーのハンドルへ書き、stdio 接続先は書き込みレーンの
/// 子プロセスへ書きます。どちらの接続でも、書き込みの道具のまとまりは同じです。
pub(super) enum WriteSource {
  Http(Box<HttpReadHandle>),
  Stdio(ClientHandle),
}

impl WriteSource {
  /// takt_answer_goal_question で managerからの質問へ回答する。
  pub(super) async fn answer_goal_question(
    &mut self,
    cwd: &std::path::Path,
    goal_id: &GoalId,
    question_id: &InquiryId,
    answer: serde_json::Value,
  ) -> Result<InquiryId, ToolError> {
    match self {
      | WriteSource::Http(handle) => {
        WriteToolSet::new(&mut **handle, cwd.to_path_buf()).answer_goal_question(goal_id, question_id, answer).await
      },
      | WriteSource::Stdio(handle) => {
        let mut client = handle.lock().await;
        WriteToolSet::new(&mut *client, cwd.to_path_buf()).answer_goal_question(goal_id, question_id, answer).await
      },
    }
  }

  /// takt_approve_goal_registration でゴール登録案を承認する。
  pub(super) async fn approve_goal_registration(
    &mut self,
    cwd: &std::path::Path,
    revision: RegistrationRevision,
  ) -> Result<ApproveGoalRegistrationResult, ToolError> {
    match self {
      | WriteSource::Http(handle) => {
        WriteToolSet::new(&mut **handle, cwd.to_path_buf()).approve_goal_registration(revision).await
      },
      | WriteSource::Stdio(handle) => {
        let mut client = handle.lock().await;
        WriteToolSet::new(&mut *client, cwd.to_path_buf()).approve_goal_registration(revision).await
      },
    }
  }

  /// takt_reject_goal_registration でゴール登録案を却下する。
  pub(super) async fn reject_goal_registration(
    &mut self,
    cwd: &std::path::Path,
    revision: RegistrationRevision,
  ) -> Result<RegistrationRevision, ToolError> {
    match self {
      | WriteSource::Http(handle) => {
        WriteToolSet::new(&mut **handle, cwd.to_path_buf()).reject_goal_registration(revision).await
      },
      | WriteSource::Stdio(handle) => {
        let mut client = handle.lock().await;
        WriteToolSet::new(&mut *client, cwd.to_path_buf()).reject_goal_registration(revision).await
      },
    }
  }

  /// takt_merge_goal_pr でゴールのプルリクエストをマージする。
  pub(super) async fn merge_goal_pr(
    &mut self,
    cwd: &std::path::Path,
    goal_id: &GoalId,
    pr_number: PullRequestNumber,
    until: MergeUntil,
  ) -> Result<MergeGoalPrResult, ToolError> {
    match self {
      | WriteSource::Http(handle) => {
        WriteToolSet::new(&mut **handle, cwd.to_path_buf()).merge_goal_pr(goal_id, pr_number, until).await
      },
      | WriteSource::Stdio(handle) => {
        let mut client = handle.lock().await;
        WriteToolSet::new(&mut *client, cwd.to_path_buf()).merge_goal_pr(goal_id, pr_number, until).await
      },
    }
  }
}

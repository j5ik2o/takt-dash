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
  domain::{GoalId, ProjectName, RunSlug, ScreenReadLanes, ServerId, TaskName},
  mcp::{client_config, ClientHandle, HttpReadHandle, ReadToolSet, RemoteRun, ToolError},
  model::{GoalDetail, ProjectView, TaskDetail},
};

/// 設定から解決した、対象サーバーの接続方法とプロジェクトの cwd。
pub(super) struct ResolvedProject {
  pub(super) server_id:         ServerId,
  pub(super) write_server_id:   ServerId,
  pub(super) project:           ProjectName,
  /// 読み取り接続先（`server` の接続方法）。
  pub(super) connection:        McpConnection,
  /// 書き込み接続先（`write_server` の接続方法）。書き込みは stdio に限ります。
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

/// 書き込み RPC 用の stdio 起動設定を取り出す。書き込みは stdio の子プロセスだけを使います。
pub(super) fn write_client_config(
  connection: &McpConnection,
  cwd: &std::path::Path,
) -> Result<crate::mcp::McpClientConfig, Status> {
  match connection {
    | McpConnection::Stdio(server) => Ok(client_config(server, cwd)),
    | McpConnection::Http(_) => {
      Err(Status::unavailable("このサーバーは読み取り専用の HTTP 接続先のため、書き込みには使えません。"))
    },
  }
}

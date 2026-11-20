//! 画面の要求時の読み取りと書き込みに使う、プロジェクトの接続先の解決。
//!
//! 設定から対象プロジェクトの読み取り接続先（`server`）と書き込み接続先
//! （`write_server`）を解決し、読み取り元（常駐 HTTP サーバーか stdio の
//! 子プロセスか）を組み立てる。

use std::{path::PathBuf, sync::Arc};

use tonic::Status;

use super::{TaktDashServiceImpl, READ_TIMEOUT};
use crate::{
  config::{load_config, Config, McpConnection},
  domain::{ProjectName, RunSlug, ServerId},
  mcp::{client_config, HttpReadHandle, ToolError, ToolSet},
};

/// 設定から解決した、対象サーバーの接続方法とプロジェクトの cwd。
pub(super) struct ResolvedProject {
  pub(super) server_id:        ServerId,
  pub(super) write_server_id:  ServerId,
  pub(super) project:          ProjectName,
  pub(super) connection:       McpConnection,
  pub(super) write_connection: McpConnection,
  pub(super) cwd:              PathBuf,
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

  /// 読み取り RPC 用の読み取り元を作る。HTTP 接続先は常駐サーバーのハンドル、stdio 接続先は
  /// レジストリで共有する子プロセスのクライアントを使います。
  pub(super) fn read_source(&self, resolved: &ResolvedProject) -> ReadSource {
    match &resolved.connection {
      | McpConnection::Http(_) => {
        ReadSource::Http(Box::new(HttpReadHandle::new(resolved.connection.clone(), READ_TIMEOUT)))
      },
      | McpConnection::Stdio(server) => {
        let config = client_config(server, &resolved.cwd);
        let handle = self.registry.client(resolved.server_id.as_str(), resolved.project.as_str(), || config);
        ReadSource::Stdio(handle)
      },
    }
  }
}

/// 画面の要求時の読み取り用の読み取り元。
///
/// HTTP 接続先は常駐サーバーのハンドルから読み、stdio 接続先は設定どおり子プロセス（レジストリで
/// 共有）から読みます。新規の読み取りは HTTP 接続先で行い、stdio は既存の設定の互換として残します。
pub(super) enum ReadSource {
  Http(Box<HttpReadHandle>),
  Stdio(Arc<tokio::sync::Mutex<crate::mcp::McpClient>>),
}

impl ReadSource {
  pub(super) async fn read_run_detail(
    &mut self,
    cwd: &std::path::Path,
    run_slug: &RunSlug,
  ) -> Result<crate::model::RunDetail, ToolError> {
    match self {
      | ReadSource::Http(handle) => handle.read_run_detail(cwd, run_slug).await,
      | ReadSource::Stdio(client) => {
        let mut client = client.lock().await;
        ToolSet::new(&mut *client, cwd.to_path_buf()).read_run_detail(run_slug).await
      },
    }
  }

  pub(super) async fn read_goal_detail(
    &mut self,
    cwd: &std::path::Path,
    goal_id: &str,
  ) -> Result<crate::model::GoalDetail, ToolError> {
    match self {
      | ReadSource::Http(handle) => handle.read_goal_detail(cwd, goal_id).await,
      | ReadSource::Stdio(client) => {
        let mut client = client.lock().await;
        ToolSet::new(&mut *client, cwd.to_path_buf()).read_goal_detail(goal_id).await
      },
    }
  }

  pub(super) async fn read_task_detail(
    &mut self,
    cwd: &std::path::Path,
    project: &str,
    task_name: &str,
    goal_id: Option<&str>,
  ) -> Result<Option<crate::model::TaskDetail>, ToolError> {
    match self {
      | ReadSource::Http(handle) => handle.read_task_detail(cwd, project, task_name, goal_id).await,
      | ReadSource::Stdio(client) => {
        let mut client = client.lock().await;
        ToolSet::new(&mut *client, cwd.to_path_buf()).read_task_detail(project, task_name, goal_id).await
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

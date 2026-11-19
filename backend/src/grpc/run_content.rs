//! GetRunContent の本文を、run の保存場所か takt-mcp から読む。
//!
//! SSH 接続の本文は対象サーバーから読み、同名のローカル保存場所を使いません。
//! ローカルの対応表に無い run と対応表を読めない run も、GetRunDetail と同じ条件で takt_get_run の
//! view report（レポート）か view instructions の instructionId 指定（指示書）から読みます。
//! ローカルで読める run では takt-mcp を起動しません。

use super::ResolvedProject;
use crate::{
  domain::{InstructionId, ReportFilename, RunSlug},
  mcp::{McpClient, ToolError, ToolSet},
  run_log::{ContentRead, RunContent},
  run_progress::{self, read_report_content, RunReadFailure},
};

/// run のレポート `filename` の本文を、先頭から `max_bytes` まで読む。
///
/// takt_get_run でも読めなければ、保存場所の読み取りの失敗を返します。
pub(super) async fn read_report_body(
  resolved: &ResolvedProject,
  run_slug: &RunSlug,
  filename: &ReportFilename,
  max_bytes: u64,
) -> Result<ContentRead, RunReadFailure> {
  let local = if resolved.client_config.is_ssh() {
    Err(RunReadFailure::NotListed)
  } else {
    read_report_content(resolved.cwd.clone(), run_slug.clone(), filename.clone(), max_bytes).await
  };
  match local {
    | Err(failure @ (RunReadFailure::NotListed | RunReadFailure::Unreadable(_))) => {
      match read_remote_report(resolved, run_slug, filename, max_bytes).await {
        | Ok(content) => Ok(ContentRead::Found(content)),
        | Err(error) => {
          tracing::warn!(run_slug = %run_slug.as_str(), report = %filename.as_str(), local = %failure, error = %error, "レポート本文を保存場所からも takt_get_run からも読めませんでした。");
          Err(failure)
        },
      }
    },
    | read => read,
  }
}

/// 対象サーバーの takt-mcp を起動して takt_get_run からレポート本文を読み、子プロセスを終了する。
async fn read_remote_report(
  resolved: &ResolvedProject,
  run_slug: &RunSlug,
  filename: &ReportFilename,
  max_bytes: u64,
) -> Result<RunContent, ToolError> {
  let mut client = McpClient::new(resolved.client_config.clone());
  let result = ToolSet::new(&mut client, resolved.cwd.clone()).read_remote_report(run_slug, filename, max_bytes).await;
  client.shutdown().await;
  result
}

/// run の指示書 `instruction_id` の本文を、先頭から `max_bytes` まで読む。
///
/// takt_get_run でも読めなければ、保存場所の読み取りの失敗を返します。
pub(super) async fn read_instruction_body(
  resolved: &ResolvedProject,
  run_slug: &RunSlug,
  instruction_id: InstructionId,
  max_bytes: u64,
) -> Result<ContentRead, RunReadFailure> {
  let local = if resolved.client_config.is_ssh() {
    Err(RunReadFailure::NotListed)
  } else {
    run_progress::read_instruction_body(resolved.cwd.clone(), run_slug.clone(), instruction_id, max_bytes).await
  };
  match local {
    | Err(failure @ (RunReadFailure::NotListed | RunReadFailure::Unreadable(_))) => {
      match read_remote_instruction(resolved, run_slug, instruction_id, max_bytes).await {
        | Ok(content) => Ok(ContentRead::Found(content)),
        | Err(error) => {
          tracing::warn!(run_slug = %run_slug.as_str(), instruction_id = instruction_id.get(), local = %failure, error = %error, "指示書の本文を保存場所からも takt_get_run からも読めませんでした。");
          Err(failure)
        },
      }
    },
    | read => read,
  }
}

/// 対象サーバーの takt-mcp を起動して takt_get_run から指示書の本文を読み、子プロセスを終了する。
async fn read_remote_instruction(
  resolved: &ResolvedProject,
  run_slug: &RunSlug,
  instruction_id: InstructionId,
  max_bytes: u64,
) -> Result<RunContent, ToolError> {
  let mut client = McpClient::new(resolved.client_config.clone());
  let result =
    ToolSet::new(&mut client, resolved.cwd.clone()).read_remote_instruction(run_slug, instruction_id, max_bytes).await;
  client.shutdown().await;
  result
}

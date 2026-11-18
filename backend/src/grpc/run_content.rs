//! GetRunContent の本文を、run の保存場所か takt-mcp から読む。
//!
//! 本文は run の保存場所から読みます。保存場所の対応表に無い run と対応表を読めない run（SSH 越しに
//! 扱うリモートのサーバーのプロジェクトなど）だけ、GetRunDetail と同じ条件で takt_get_run の
//! view report（レポート）か view instructions の instructionId 指定（指示書）から読みます。
//! ローカルで読める run では takt-mcp を起動しません。

use super::ResolvedProject;
use crate::{
  domain::{InstructionId, ReportFilename, RunSlug},
  mcp::{McpClientRegistry, ReadToolSet, ToolError},
  run_log::{ContentRead, RunContent},
  run_progress::{self, read_report_content, RunReadFailure},
};

/// run のレポート `filename` の本文を、先頭から `max_bytes` まで読む。
///
/// takt_get_run でも読めなければ、保存場所の読み取りの失敗を返します。
pub(super) async fn read_report_body(
  registry: &McpClientRegistry,
  resolved: &ResolvedProject,
  run_slug: &RunSlug,
  filename: &ReportFilename,
  max_bytes: u64,
) -> Result<ContentRead, RunReadFailure> {
  match read_report_content(resolved.cwd.clone(), run_slug.clone(), filename.clone(), max_bytes).await {
    | Err(failure @ (RunReadFailure::NotListed | RunReadFailure::Unreadable(_))) => {
      match read_remote_report(registry, resolved, run_slug, filename, max_bytes).await {
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

/// 画面読み取りレーンの接続で takt_get_run からレポート本文を読む。
async fn read_remote_report(
  registry: &McpClientRegistry,
  resolved: &ResolvedProject,
  run_slug: &RunSlug,
  filename: &ReportFilename,
  max_bytes: u64,
) -> Result<RunContent, ToolError> {
  let handle =
    registry.screen_client(resolved.server_id.as_str(), resolved.project.as_str(), resolved.screen_read_lanes, || {
      resolved.client_config.clone()
    });
  let mut client = handle.lock().await;
  ReadToolSet::new(&mut client, resolved.cwd.clone()).read_remote_report(run_slug, filename, max_bytes).await
}

/// run の指示書 `instruction_id` の本文を、先頭から `max_bytes` まで読む。
///
/// takt_get_run でも読めなければ、保存場所の読み取りの失敗を返します。
pub(super) async fn read_instruction_body(
  registry: &McpClientRegistry,
  resolved: &ResolvedProject,
  run_slug: &RunSlug,
  instruction_id: InstructionId,
  max_bytes: u64,
) -> Result<ContentRead, RunReadFailure> {
  match run_progress::read_instruction_body(resolved.cwd.clone(), run_slug.clone(), instruction_id, max_bytes).await {
    | Err(failure @ (RunReadFailure::NotListed | RunReadFailure::Unreadable(_))) => {
      match read_remote_instruction(registry, resolved, run_slug, instruction_id, max_bytes).await {
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

/// 画面読み取りレーンの接続で takt_get_run から指示書の本文を読む。
async fn read_remote_instruction(
  registry: &McpClientRegistry,
  resolved: &ResolvedProject,
  run_slug: &RunSlug,
  instruction_id: InstructionId,
  max_bytes: u64,
) -> Result<RunContent, ToolError> {
  let handle =
    registry.screen_client(resolved.server_id.as_str(), resolved.project.as_str(), resolved.screen_read_lanes, || {
      resolved.client_config.clone()
    });
  let mut client = handle.lock().await;
  ReadToolSet::new(&mut client, resolved.cwd.clone()).read_remote_instruction(run_slug, instruction_id, max_bytes).await
}

//! GetRunContent の本文を、run の保存場所か takt-mcp から読む。
//!
//! 本文は run の保存場所から読みます。保存場所の対応表に無い run と対応表を読めない run（SSH 越しに
//! 扱うリモートのサーバーのプロジェクトなど）だけ、GetRunDetail と同じ条件で takt_get_run の
//! view report（レポート）か view instructions の instructionId 指定（指示書）から読みます。
//! ローカルで読める run では takt-mcp を起動しません。

use super::{resolve::ReadSource, ResolvedProject};
use crate::{
  domain::{InstructionId, ReportFilename, RunSlug},
  run_log::ContentRead,
  run_progress::{self, read_report_content, RunReadFailure},
};

/// run のレポート `filename` の本文を、先頭から `max_bytes` まで読む。
///
/// takt_get_run でも読めなければ、保存場所の読み取りの失敗を返します。
pub(super) async fn read_report_body(
  source: &mut ReadSource,
  resolved: &ResolvedProject,
  run_slug: &RunSlug,
  filename: &ReportFilename,
  max_bytes: u64,
) -> Result<ContentRead, RunReadFailure> {
  let local = if resolved.connection.is_ssh() {
    Err(RunReadFailure::NotListed)
  } else {
    read_report_content(resolved.cwd.clone(), run_slug.clone(), filename.clone(), max_bytes).await
  };
  match local {
    | Err(failure @ (RunReadFailure::NotListed | RunReadFailure::Unreadable(_))) => {
      match source.read_remote_report(&resolved.cwd, run_slug, filename, max_bytes).await {
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

/// run の指示書 `instruction_id` の本文を、先頭から `max_bytes` まで読む。
///
/// takt_get_run でも読めなければ、保存場所の読み取りの失敗を返します。
pub(super) async fn read_instruction_body(
  source: &mut ReadSource,
  resolved: &ResolvedProject,
  run_slug: &RunSlug,
  instruction_id: InstructionId,
  max_bytes: u64,
) -> Result<ContentRead, RunReadFailure> {
  let local = if resolved.connection.is_ssh() {
    Err(RunReadFailure::NotListed)
  } else {
    run_progress::read_instruction_body(resolved.cwd.clone(), run_slug.clone(), instruction_id, max_bytes).await
  };
  match local {
    | Err(failure @ (RunReadFailure::NotListed | RunReadFailure::Unreadable(_))) => {
      match source.read_remote_instruction(&resolved.cwd, run_slug, instruction_id, max_bytes).await {
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

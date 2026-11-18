//! GetRunContent のレポート本文を、run の保存場所か takt-mcp から読む。
//!
//! 本文は run の保存場所から読みます。保存場所の対応表に無い run と対応表を読めない run（SSH 越しに
//! 扱うリモートのサーバーのプロジェクトなど）だけ、GetRunDetail と同じ条件で takt_get_run の
//! view report から読みます。ローカルで読める run では takt-mcp を起動しません。

use super::ResolvedProject;
use crate::{
  domain::{ReportFilename, RunSlug},
  mcp::{McpClient, ToolError, ToolSet},
  run_log::{ContentRead, RunContent},
  run_progress::{read_report_content, RunReadFailure},
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
  match read_report_content(resolved.cwd.clone(), run_slug.clone(), filename.clone(), max_bytes).await {
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

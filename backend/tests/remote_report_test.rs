//! takt_get_run の view report からレポート本文を読む `ReadToolSet::read_remote_report` が、
//! ページをたどって本文を連結し、上限で文字の境界に切り詰め、不正な応答を解析エラーにすることを
//! 検証する。

mod common;

use std::path::Path;

use common::*;
use serde_json::{json, Value};
use takt_dash_backend::{
  domain::{ReportFilename, RunSlug},
  mcp::ToolError,
  run_log::RunContent,
};

/// mock の状態 `state` で takt_get_run の view report から run-1 のレポート `filename` を読む。
async fn read_report(state: Value, filename: &str, max_bytes: u64) -> Result<RunContent, ToolError> {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state_path = directory.path().join("state.json");
  std::fs::write(&state_path, state.to_string()).expect("状態ファイルを書けます");
  let mut client = mock_client(mock_config(directory.path(), &state_path, None));
  let result = tool_set(&mut client, Path::new(directory.path()))
    .read_remote_report(
      &RunSlug::try_from("run-1").expect("有効な runSlug です"),
      &ReportFilename::try_from(filename).expect("有効なレポートの名前です"),
      max_bytes,
    )
    .await;
  client.shutdown().await;
  result
}

fn report_state(filename: &str, content: &str) -> Value {
  json!({ "runs": { "run-1": { "runSlug": "run-1", "reports": [ { "filename": filename, "content": content } ] } } })
}

#[tokio::test]
async fn read_remote_report_joins_every_page_without_breaking_characters() {
  let content = "い".repeat(2000);

  let report = read_report(report_state("plan.md", &content), "plan.md", 1024 * 1024).await.expect("本文を読めます");

  assert_eq!(report, RunContent { content: content.clone(), size_bytes: content.len() as u64, truncated: false });
}

#[tokio::test]
async fn read_remote_report_truncates_at_the_limit_on_a_character_boundary() {
  // 上限 10 バイトに対して3バイトの文字を並べると、9バイトで止まり truncated になる。
  let content = "あ".repeat(10);

  let report = read_report(report_state("plan.md", &content), "plan.md", 10).await.expect("本文を読めます");

  assert_eq!(report, RunContent { content: "あ".repeat(3), size_bytes: 30, truncated: true });
}

#[tokio::test]
async fn read_remote_report_returns_a_tool_error_for_a_missing_report() {
  let error =
    read_report(report_state("plan.md", "本文"), "review.md", 1024).await.expect_err("無いレポートは読めません");

  assert!(matches!(error, ToolError::Tool(_)), "無いレポートは道具エラーです: {error}");
}

#[tokio::test]
async fn read_remote_report_rejects_a_response_without_a_report_object() {
  let state = json!({ "responses": { "takt_get_run": { "runSlug": "run-1", "status": "running" } } });

  let error = read_report(state, "plan.md", 1024).await.expect_err("report の無い応答は解析できません");

  assert!(matches!(error, ToolError::Parse(_)), "report の無い応答は解析エラーです: {error}");
}

#[tokio::test]
async fn read_remote_report_rejects_a_next_offset_that_does_not_advance() {
  // 同じ応答を返し続けるサーバーでも、読み取りを終わらせて解析エラーにする。
  let state = json!({
    "responses": {
      "takt_get_run": {
        "runSlug": "run-1",
        "report": { "filename": "plan.md", "content": "", "totalBytes": 10, "offset": 0, "nextOffset": 0 }
      }
    }
  });

  let error = read_report(state, "plan.md", 1024).await.expect_err("進まない nextOffset は解析できません");

  assert!(matches!(error, ToolError::Parse(_)), "進まない nextOffset は解析エラーです: {error}");
}

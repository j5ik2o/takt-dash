//! takt_get_run の view instructions（instructionId 指定）から指示書本文を読む
//! `ReadToolSet::read_remote_instruction` が、ページをたどって本文を連結し、上限で文字の境界に
//! 切り詰め、不正な応答を解析エラーにすることを検証する。

mod common;

use std::path::Path;

use common::*;
use serde_json::{json, Value};
use takt_dash_backend::{
  domain::{InstructionId, RunSlug},
  mcp::ToolError,
  run_log::RunContent,
};

/// mock の状態 `state` で takt_get_run の view instructions から run-1 の指示書
/// `instruction_id` の本文を読む。
async fn read_instruction(state: Value, instruction_id: i32, max_bytes: u64) -> Result<RunContent, ToolError> {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state_path = directory.path().join("state.json");
  std::fs::write(&state_path, state.to_string()).expect("状態ファイルを書けます");
  let mut client = mock_client(mock_config(directory.path(), &state_path, None));
  let result = tool_set(&mut client, Path::new(directory.path()))
    .read_remote_instruction(
      &RunSlug::try_from("run-1").expect("有効な runSlug です"),
      InstructionId::try_from(instruction_id).expect("有効な指示書 ID です"),
      max_bytes,
    )
    .await;
  client.shutdown().await;
  result
}

fn instruction_state(content: &str) -> Value {
  json!({
    "runs": {
      "run-1": {
        "runSlug": "run-1",
        "instructions": [
          { "instructionId": 1, "issuedAt": "2026-11-13T08:50:00Z", "state": "pending", "content": content }
        ]
      }
    }
  })
}

#[tokio::test]
async fn read_remote_instruction_joins_every_page_without_breaking_characters() {
  let content = "い".repeat(2000);

  let instruction = read_instruction(instruction_state(&content), 1, 1024 * 1024).await.expect("本文を読めます");

  assert_eq!(instruction, RunContent {
    content:    content.clone(),
    size_bytes: content.len() as u64,
    truncated:  false,
  });
}

#[tokio::test]
async fn read_remote_instruction_truncates_at_the_limit_on_a_character_boundary() {
  // 上限 10 バイトに対して3バイトの文字を並べると、9バイトで止まり truncated になる。
  let content = "あ".repeat(10);

  let instruction = read_instruction(instruction_state(&content), 1, 10).await.expect("本文を読めます");

  assert_eq!(instruction, RunContent { content: "あ".repeat(3), size_bytes: 30, truncated: true });
}

#[tokio::test]
async fn read_remote_instruction_returns_a_tool_error_for_a_missing_instruction() {
  let error = read_instruction(instruction_state("本文"), 2, 1024).await.expect_err("無い指示書は読めません");

  assert!(matches!(error, ToolError::Tool(_)), "無い指示書は道具エラーです: {error}");
}

#[tokio::test]
async fn read_remote_instruction_rejects_a_response_without_an_instruction_object() {
  let state = json!({ "responses": { "takt_get_run": { "runSlug": "run-1", "status": "running" } } });

  let error = read_instruction(state, 1, 1024).await.expect_err("instruction の無い応答は解析できません");

  assert!(matches!(error, ToolError::Parse(_)), "instruction の無い応答は解析エラーです: {error}");
}

#[tokio::test]
async fn read_remote_instruction_rejects_a_next_offset_that_does_not_advance() {
  // 同じ応答を返し続けるサーバーでも、読み取りを終わらせて解析エラーにする。
  let state = json!({
    "responses": {
      "takt_get_run": {
        "runSlug": "run-1",
        "instruction": { "instructionId": 1, "content": "", "totalBytes": 10, "offset": 0, "nextOffset": 0 }
      }
    }
  });

  let error = read_instruction(state, 1, 1024).await.expect_err("進まない nextOffset は解析できません");

  assert!(matches!(error, ToolError::Parse(_)), "進まない nextOffset は解析エラーです: {error}");
}

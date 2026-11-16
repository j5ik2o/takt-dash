//! takt 応答の runSlug が run
//! ディレクトリ名として不正なとき、状態の読み取りが解析エラーになることを検証する。

mod common;

use std::io::Write;

use common::*;
use serde_json::json;
use takt_dash_backend::mcp::ToolError;

fn write_state(directory: &std::path::Path, state: serde_json::Value) -> std::path::PathBuf {
  let path = directory.join("state.json");
  let mut file = std::fs::File::create(&path).expect("状態ファイルを作れます");
  file.write_all(state.to_string().as_bytes()).expect("状態ファイルへ書けます");
  path
}

#[tokio::test]
async fn a_run_slug_that_is_not_a_single_directory_name_is_a_parse_error() {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(
    directory.path(),
    json!({
      "tasks": [ { "name": "a", "status": "running", "runSlug": "r1", "startedAt": "2026-11-13T10:00:00Z" } ],
      "runs": {
        "r1": {
          "runSlug": "../escape",
          "status": "running",
          "startedAt": "2026-11-13T10:00:00Z",
          "stepLogs": []
        }
      }
    }),
  );
  let mut client = mock_client(mock_config(directory.path(), &state, None));

  let error = tool_set(&mut client, directory.path())
    .read_snapshot(None)
    .await
    .expect_err("run ディレクトリ名にならない runSlug は解析エラーです");

  assert!(matches!(error, ToolError::Parse(_)), "run ディレクトリ名にならない runSlug は拒否します: {error}");
  client.shutdown().await;
}

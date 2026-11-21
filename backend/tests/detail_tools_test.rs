//! ゴール詳細の要求時読み取りが、応答のない takt-mcp を有限の deadline でタイムアウトさせる
//! ことを検証する。

mod common;

use common::*;
use serde_json::json;
use takt_dash_backend::{
  domain::GoalId,
  mcp::{ToolError, TransportError},
};

#[tokio::test]
async fn read_goal_detail_times_out_when_takt_mcp_does_not_respond() {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state_path = directory.path().join("state.json");
  std::fs::write(&state_path, json!({}).to_string()).expect("状態ファイルを書けます");
  let mut client = mock_client(mock_config_with_timeout(directory.path(), &state_path, Some("hang"), 1));

  let error = tool_set(&mut client, directory.path())
    .read_goal_detail(&GoalId::try_from("g1").expect("有効なゴール ID です"))
    .await
    .expect_err("応答のない takt-mcp はタイムアウトします");

  match error {
    | ToolError::Transport(TransportError::Timeout(_)) => {},
    | other => panic!("タイムアウトの輸送エラーを期待しました: {other}"),
  }
  client.shutdown().await;
}

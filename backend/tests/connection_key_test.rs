//! 詳細の summary を使い回す鍵（ToolCaller::connection_key）に、接続の秘密が入らないことと、
//! 接続先が変わると鍵も変わることを検証する。

mod common;

use std::{path::Path, time::Duration};

use common::*;
use takt_dash_backend::mcp::{HttpReadHandle, McpClient, ToolCaller};

const SECRET: &str = "secret-token-do-not-leak";

#[test]
fn the_http_connection_key_has_the_endpoint_but_not_the_token() {
  let handle = HttpReadHandle::new(http_connection("http://127.0.0.1:4100/", SECRET), Duration::from_secs(1));
  let other_port = HttpReadHandle::new(http_connection("http://127.0.0.1:4101/", SECRET), Duration::from_secs(1));

  let key = handle.connection_key();

  assert!(!key.contains(SECRET), "鍵にトークンを入れません: {key}");
  assert_ne!(key, other_port.connection_key(), "接続先が変われば鍵も変わります");
}

#[test]
fn the_stdio_connection_key_hides_environment_values_but_changes_with_them() {
  let directory = Path::new("/abs/takt-dash");
  let mut config = mock_config(directory, &directory.join("state.json"), None);
  config.env.insert("TAKT_TOKEN".to_owned(), SECRET.to_owned());
  let mut other = config.clone();
  other.env.insert("TAKT_TOKEN".to_owned(), "another-token".to_owned());

  let key = McpClient::new(config).connection_key();

  assert!(!key.contains(SECRET), "鍵に環境変数の値を入れません: {key}");
  assert_ne!(key, McpClient::new(other).connection_key(), "環境変数の値が変われば鍵も変わります");
}

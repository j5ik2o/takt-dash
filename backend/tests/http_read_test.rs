//! 読み取り専用の常駐 takt-mcp HTTP サーバーへ backend の読み取り経路を向ける契約を検証する。
//!
//! mock HTTP MCP サーバー（`mock_mcp_http_server.mjs`）は実機 dist の HTTP transport と同じく、
//! `Authorization` の完全一致・`127.0.0.1` の待ち受け・`Mcp-Session-Id` の採番と再利用・
//! 未知セッションの 404 を再現する。ここでは HTTP 読み取りハンドル単体の往復・認証・
//! セッションの再利用と復旧・失敗の分類を観測する。

mod common;

use common::*;
use serde_json::json;
use takt_dash_backend::{
  mcp::{HttpReadHandle, ToolError},
  model::TaskStatus,
};

fn write_state(directory: &std::path::Path, state: serde_json::Value) -> std::path::PathBuf {
  let path = directory.join("state.json");
  std::fs::write(&path, state.to_string()).expect("状態ファイルを書けます");
  path
}

fn project_dir(directory: &std::path::Path) -> std::path::PathBuf {
  let path = directory.join("project");
  std::fs::create_dir_all(&path).expect("プロジェクトの cwd を作れます");
  std::fs::canonicalize(&path).expect("プロジェクトの cwd の実パスを解決できます")
}

#[tokio::test]
async fn http_handle_reads_tools_over_the_resident_server() {
  // 常駐サーバーへ tools/call を送り、応答の本文を道具の結果として読める。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(
    directory.path(),
    json!({
      "goals": [ { "id": "g1", "status": "created", "executionStatus": "active", "objective": "ゴール g1", "integrationBranch": "takt/g1" } ],
      "tasks": [ { "name": "implement", "status": "running", "runSlug": "r1", "startedAt": "2026-11-13T09:00:00Z" } ],
      "runs": { "r1": { "runSlug": "r1", "status": "running", "startedAt": "2026-11-13T09:00:00Z" } }
    }),
  );
  let server = HttpMockServer::start(&state, None);
  let cwd = project_dir(directory.path());
  let mut handle = HttpReadHandle::new(server.connection(), std::time::Duration::from_secs(10));

  let snapshot = handle.read_snapshot(&cwd, None).await.expect("読み取れます");

  assert_eq!(snapshot.goals.len(), 1, "HTTP 経由でゴールを読めます");
  assert_eq!(snapshot.tasks.first().expect("タスクがあります").status, TaskStatus::Running);
  let calls = server.recorded_calls();
  assert!(
    calls.iter().any(|call| call["name"] == "takt_list_goals"),
    "HTTP サーバーへ takt_list_goals を送ります: {calls:?}"
  );
  assert_eq!(handle.session_count(), 1, "1 つのハンドルはサーバー上に 1 セッションだけ作ります");
}

#[tokio::test]
async fn http_handle_sends_the_authorization_header_from_the_connection() {
  // 接続設定の Authorization を全要求に付ける。mock は不一致を 403 で返すため、
  // 別トークンで設定したハンドルは読み取りに失敗する。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(directory.path(), json!({ "goals": [] }));
  let server = HttpMockServer::start(&state, None);
  let cwd = project_dir(directory.path());

  let mut wrong = HttpReadHandle::new(http_connection(server.url(), "wrong-token"), std::time::Duration::from_secs(10));
  let error = wrong.read_snapshot(&cwd, None).await.expect_err("トークン不一致は読み取り失敗です");
  assert!(matches!(error, ToolError::Transport(_)), "認証失敗は輸送エラーの分類で表面化します: {error}");

  let mut correct =
    HttpReadHandle::new(http_connection(server.url(), server.token()), std::time::Duration::from_secs(10));
  correct.read_snapshot(&cwd, None).await.expect("正しいトークンでは読み取れます");
}

#[tokio::test]
async fn http_handle_reuses_one_session_across_calls() {
  // 同じハンドルが読み取りを繰り返しても initialize は 1 回だけで、同じセッションを再利用する。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(directory.path(), json!({ "goals": [] }));
  let server = HttpMockServer::start(&state, None);
  let cwd = project_dir(directory.path());
  let mut handle = HttpReadHandle::new(server.connection(), std::time::Duration::from_secs(10));

  handle.read_snapshot(&cwd, None).await.expect("1 回目を読み取れます");
  handle.read_snapshot(&cwd, None).await.expect("2 回目を読み取れます");

  let events = server.session_events();
  assert_eq!(
    events.iter().filter(|event| event.starts_with("new ")).count(),
    1,
    "セッション採番は 1 回だけです: {events:?}"
  );
  assert_eq!(handle.session_count(), 1, "ハンドルはセッションを 1 つだけ保持します");
}

#[tokio::test]
async fn unknown_session_is_recovered_by_creating_a_new_session() {
  // サーバー再起動などでセッションが消えると未知の ID は 404 になる。未知セッションだけは
  // セッションを作り直して 1 回だけ再試行し、次の読み取りでは成功する。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(directory.path(), json!({ "goals": [] }));
  let cwd = project_dir(directory.path());
  let healthy = HttpMockServer::start(&state, None);
  let mut handle = HttpReadHandle::new(healthy.connection(), std::time::Duration::from_secs(10));

  // 1 回目はセッションを作り、以降は同じセッションを再利用する。
  handle.read_snapshot(&cwd, None).await.expect("初回の読み取りは成功します");
  assert_eq!(handle.session_count(), 1, "初回に 1 セッションだけ作ります");

  // サーバーを再起動してセッションを失わせる。同じ接続先・同じハンドルのまま次の読み取りを行う。
  // 旧プロセスを止めてポートを空けてから、同じポートで新しいプロセスを立ち上げる。
  let port = healthy.port();
  drop(healthy);
  let restarted = HttpMockServer::start_on_port(&state, None, port);
  handle.read_snapshot(&cwd, None).await.expect("セッション消失後の読み取りは新セッションで回復します");
  assert_eq!(handle.session_count(), 1, "再試行後もハンドルが保持するセッションは 1 つだけです");
  let events = restarted.session_events();
  assert!(
    events.iter().filter(|event| event.starts_with("new ")).count() >= 1,
    "再起動後のサーバーは新しいセッションを採番します: {events:?}"
  );
}

#[tokio::test]
async fn a_server_that_keeps_returning_404_fails_instead_of_looping() {
  // 404 を返し続けるサーバーでは、再試行を無限に繰り返さず読み取り失敗として表面化する。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(directory.path(), json!({ "goals": [] }));
  let server = HttpMockServer::start(&state, Some("drop-session"));
  let cwd = project_dir(directory.path());
  let mut handle = HttpReadHandle::new(server.connection(), std::time::Duration::from_secs(2));

  let error = handle.read_snapshot(&cwd, None).await.expect_err("404 が続くサーバーは読み取り失敗です");
  assert!(matches!(error, ToolError::Transport(_)), "セッション消失の失敗は輸送エラーです: {error}");
}

#[tokio::test]
async fn connection_refused_is_a_transport_error_not_an_empty_snapshot() {
  // 常駐サーバーが落ちている（接続拒否）ときは、空 snapshot を成功として返さず輸送エラーにする。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let cwd = project_dir(directory.path());
  // 待ち受けていないポートへ接続する。ポートは立ててすぐ落として確実に空ける。
  let server = {
    let state = write_state(directory.path(), json!({ "goals": [] }));
    let server = HttpMockServer::start(&state, None);
    let port = server.port();
    drop(server);
    port
  };
  let url = format!("http://127.0.0.1:{server}/");
  let mut handle = HttpReadHandle::new(http_connection(&url, "test-token"), std::time::Duration::from_secs(2));

  let error = handle.read_snapshot(&cwd, None).await.expect_err("接続拒否は読み取り失敗です");
  assert!(
    matches!(error, ToolError::Transport(_)),
    "接続拒否は輸送エラーに分類します（空 snapshot に縮退しません）: {error}"
  );
}

#[tokio::test]
async fn tool_error_is_distinguished_from_transport_failure() {
  // 道具が isError を返す場合は道具エラーで、接続断の輸送エラーとは区別する。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(directory.path(), json!({ "goals": [], "isError": ["takt_list_goals"] }));
  let server = HttpMockServer::start(&state, None);
  let cwd = project_dir(directory.path());
  let mut handle = HttpReadHandle::new(server.connection(), std::time::Duration::from_secs(10));

  let error = handle.read_snapshot(&cwd, None).await.expect_err("道具エラーは読み取り失敗です");
  assert!(matches!(error, ToolError::Tool(_)), "道具の isError は道具エラーに分類します: {error}");
}

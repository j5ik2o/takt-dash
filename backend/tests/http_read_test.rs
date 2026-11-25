//! 常駐 takt-mcp HTTP サーバーへ backend
//! の読み取り経路を向ける契約と、書き込み用ハンドルの再試行の契約を検証する。
//!
//! mock HTTP MCP サーバー（`mock_mcp_http_server.mjs`）は実機 dist の HTTP transport と同じく、
//! `Authorization` の完全一致・`127.0.0.1` の待ち受け・`Mcp-Session-Id` の採番と再利用・
//! 未知セッションの 404 を再現する。ここでは HTTP 読み取りハンドル単体の往復・認証・
//! セッションの再利用と復旧・失敗の分類を観測する。

mod common;

use common::*;
use serde_json::json;
use takt_dash_backend::{
  mcp::{HttpReadHandle, HttpSession, ToolError, TransportError},
  model::TaskStatus,
};

fn write_state(directory: &std::path::Path, state: serde_json::Value) -> std::path::PathBuf {
  let path = directory.join("state.json");
  std::fs::write(&path, state.to_string()).expect("状態ファイルを書けます");
  path
}

/// snapshot の読み取りに渡す run ログの取っ手。テストのプロジェクトの cwd は空のため、
/// ローカルな run ログの補完は何も見つけません。
fn run_logs(cwd: &std::path::Path) -> takt_dash_backend::run_progress::RunLogHandle {
  let project = takt_dash_backend::domain::ProjectName::try_from("http-test").expect("プロジェクト名を作れます");
  let mut handles = takt_dash_backend::run_progress::RunLogHandles::default();
  handles.handle_for(&project, cwd)
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

  let snapshot = handle.read_snapshot(&cwd, None, &run_logs(&cwd)).await.expect("読み取れます");

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
  let error = wrong.read_snapshot(&cwd, None, &run_logs(&cwd)).await.expect_err("トークン不一致は読み取り失敗です");
  assert!(matches!(error, ToolError::Transport(_)), "認証失敗は輸送エラーの分類で表面化します: {error}");

  let mut correct =
    HttpReadHandle::new(http_connection(server.url(), server.token()), std::time::Duration::from_secs(10));
  correct.read_snapshot(&cwd, None, &run_logs(&cwd)).await.expect("正しいトークンでは読み取れます");
}

#[tokio::test]
async fn http_handle_reuses_one_session_across_calls() {
  // 同じハンドルが読み取りを繰り返しても initialize は 1 回だけで、同じセッションを再利用する。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(directory.path(), json!({ "goals": [] }));
  let server = HttpMockServer::start(&state, None);
  let cwd = project_dir(directory.path());
  let mut handle = HttpReadHandle::new(server.connection(), std::time::Duration::from_secs(10));

  handle.read_snapshot(&cwd, None, &run_logs(&cwd)).await.expect("1 回目を読み取れます");
  handle.read_snapshot(&cwd, None, &run_logs(&cwd)).await.expect("2 回目を読み取れます");

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
  handle.read_snapshot(&cwd, None, &run_logs(&cwd)).await.expect("初回の読み取りは成功します");
  assert_eq!(handle.session_count(), 1, "初回に 1 セッションだけ作ります");

  // サーバーを再起動してセッションを失わせる。同じ接続先・同じハンドルのまま次の読み取りを行う。
  // 旧プロセスを止めてポートを空けてから、同じポートで新しいプロセスを立ち上げる。
  let port = healthy.port();
  drop(healthy);
  let restarted = HttpMockServer::start_on_port(&state, None, port);
  handle
    .read_snapshot(&cwd, None, &run_logs(&cwd))
    .await
    .expect("セッション消失後の読み取りは新セッションで回復します");
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

  let error =
    handle.read_snapshot(&cwd, None, &run_logs(&cwd)).await.expect_err("404 が続くサーバーは読み取り失敗です");
  assert!(matches!(error, ToolError::Transport(_)), "セッション消失の失敗は輸送エラーです: {error}");
}

#[tokio::test]
async fn session_loss_on_tools_call_retries_once_then_surfaces_failure() {
  // initialize は成功するが tools/call の時点でセッションが消えているサーバーでは、
  // セッションを作り直して 1 回だけ再試行し、それでも 404 なら失敗として表面化する。
  // initialize の回数と tools/call の回数を数え、再試行が 1 回で止まることを確認する。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(directory.path(), json!({ "goals": [] }));
  let server = HttpMockServer::start(&state, Some("drop-session-on-call"));
  let cwd = project_dir(directory.path());
  let mut handle = HttpReadHandle::new(server.connection(), std::time::Duration::from_secs(10));

  let error = handle
    .read_snapshot(&cwd, None, &run_logs(&cwd))
    .await
    .expect_err("tools/call が 404 を返し続ける場合は読み取り失敗です");
  assert!(matches!(error, ToolError::Transport(_)), "セッション消失の失敗は輸送エラーです: {error}");

  let events = server.session_events();
  assert_eq!(
    events.iter().filter(|event| event.starts_with("new ")).count(),
    2,
    "initialize は初回と再試行の 2 回だけです: {events:?}"
  );
  let calls = server.recorded_calls();
  assert_eq!(calls.len(), 2, "tools/call は初回と再試行の 2 回だけです: {calls:?}");
  assert!(calls.iter().all(|call| call["name"] == calls[0]["name"]), "再試行は同じ道具を送り直します: {calls:?}");
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

  let error = handle.read_snapshot(&cwd, None, &run_logs(&cwd)).await.expect_err("接続拒否は読み取り失敗です");
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

  let error = handle.read_snapshot(&cwd, None, &run_logs(&cwd)).await.expect_err("道具エラーは読み取り失敗です");
  assert!(matches!(error, ToolError::Tool(_)), "道具の isError は道具エラーに分類します: {error}");
}

#[tokio::test]
async fn read_handle_retries_once_when_the_connection_drops_during_a_call() {
  // 読み取りでは、tools/call の応答が届く前に接続が切れても、セッションを作り直して
  // 1 回だけ再試行し、読み取りを成功させる。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(directory.path(), json!({ "goals": [] }));
  let server = HttpMockServer::start(&state, Some("drop-connection-once"));
  let cwd = project_dir(directory.path());
  let mut handle = HttpReadHandle::new(server.connection(), std::time::Duration::from_secs(10));

  let snapshot = handle.read_snapshot(&cwd, None, &run_logs(&cwd)).await.expect("接続断から再試行して読めます");

  assert_eq!(snapshot.goals.len(), 0, "再試行した読み取りの結果を返します");
  let calls = server.recorded_calls();
  let first = calls.first().expect("呼び出しが記録されます");
  assert_eq!(
    calls.iter().filter(|call| call["name"] == first["name"]).count(),
    2,
    "切れた呼び出しは 1 回だけ送り直されます: {calls:?}"
  );
}

#[tokio::test]
async fn write_handle_never_retries_a_write_when_the_connection_drops_during_a_call() {
  // 書き込みでは、tools/call の応答が届く前に接続が切れた場合、サーバーが書き込みを
  // 適用した後かもしれないため再試行しない。失敗として表面化し、書き込み道具は
  // 1 回だけサーバーへ渡る。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(directory.path(), json!({}));
  let server = HttpMockServer::start(&state, Some("drop-connection-once"));
  let cwd = project_dir(directory.path());
  let mut handle = HttpReadHandle::new_for_write(server.connection(), std::time::Duration::from_secs(10));
  let goal_id =
    takt_dash_backend::domain::GoalId::try_from("11111111-1111-4111-8111-111111111111").expect("ゴール ID を作れます");
  let question_id =
    takt_dash_backend::domain::InquiryId::try_from("8f14e45f-ceea-4f6a-9a3c-0f1a2b3c4d5e").expect("質問 ID を作れます");

  let error = takt_dash_backend::mcp::WriteToolSet::new(&mut handle, cwd.clone())
    .answer_goal_question(&goal_id, &question_id, json!({ "text": "はい" }))
    .await
    .expect_err("接続断した書き込みは失敗として表面化します");

  assert!(matches!(error, ToolError::Transport(_)), "輸送エラーです: {error}");
  let calls = server.recorded_calls();
  assert_eq!(
    calls.iter().filter(|call| call["name"] == "takt_answer_goal_question").count(),
    1,
    "書き込み道具は 1 回だけサーバーへ渡ります: {calls:?}"
  );
}

#[tokio::test]
async fn dropping_the_last_handle_closes_its_session_on_the_server() {
  // セッションを使い終えたら（最後の持ち主が消えたら）DELETE で閉じる。接続先の設定が
  // 変わって作り直した読み取りハンドルの古いセッションを、サーバーに残さない。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(directory.path(), json!({ "goals": [] }));
  let server = HttpMockServer::start(&state, None);
  let cwd = project_dir(directory.path());
  let mut handle = HttpReadHandle::new(server.connection(), std::time::Duration::from_secs(10));
  handle.read_snapshot(&cwd, None, &run_logs(&cwd)).await.expect("読み取れます");
  assert_eq!(server.live_session_count(), 1, "読み取りの間はセッションが 1 つ生きています");

  drop(handle);

  server.wait_for_live_sessions(0).await;
}

#[tokio::test]
async fn handles_sharing_a_session_recover_together_after_a_server_restart() {
  // 画面の読み取りと書き込みは、サーバーごとの 1 つのセッションを共有する。サーバーの再起動で
  // そのセッションが消えても、共有するハンドルは自動で作り直し、作り直すのは 1 つだけ。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(directory.path(), json!({ "goals": [] }));
  let cwd = project_dir(directory.path());
  let healthy = HttpMockServer::start(&state, None);
  let session = HttpSession::new(healthy.connection());
  let timeout = std::time::Duration::from_secs(10);
  let mut first = HttpReadHandle::with_session(session.clone(), timeout);
  let mut second = HttpReadHandle::with_session(session.clone(), timeout);

  first.read_snapshot(&cwd, None, &run_logs(&cwd)).await.expect("1 つ目のハンドルで読み取れます");
  second.read_snapshot(&cwd, None, &run_logs(&cwd)).await.expect("2 つ目のハンドルで読み取れます");
  assert_eq!(healthy.created_session_count(), 1, "2 つのハンドルは 1 つのセッションを共有します");

  let port = healthy.port();
  drop(healthy);
  let restarted = HttpMockServer::start_on_port(&state, None, port);
  let logs = run_logs(&cwd);
  let (first_read, second_read) =
    tokio::join!(first.read_snapshot(&cwd, None, &logs), second.read_snapshot(&cwd, None, &logs));
  first_read.expect("再起動後、1 つ目のハンドルは自動で再接続します");
  second_read.expect("再起動後、2 つ目のハンドルは自動で再接続します");

  assert_eq!(
    restarted.created_session_count(),
    1,
    "再起動後に作り直すセッションは 1 つだけです: {:?}",
    restarted.session_events()
  );
}

#[tokio::test]
async fn closing_a_session_deletes_it_and_the_next_call_creates_a_new_one() {
  // backend の停止時は、DELETE の応答を待ってセッションを閉じる。閉じたあとの要求は、
  // セッションを作り直して通る。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(directory.path(), json!({ "goals": [] }));
  let server = HttpMockServer::start(&state, None);
  let cwd = project_dir(directory.path());
  let mut handle = HttpReadHandle::new(server.connection(), std::time::Duration::from_secs(10));
  handle.read_snapshot(&cwd, None, &run_logs(&cwd)).await.expect("読み取れます");

  handle.close().await;

  assert_eq!(server.live_session_count(), 0, "閉じたセッションはサーバーに残りません: {:?}", server.session_events());
  assert_eq!(handle.session_count(), 0, "閉じたハンドルはセッションを保持しません");
  handle.read_snapshot(&cwd, None, &run_logs(&cwd)).await.expect("閉じたあとの読み取りは作り直して通ります");
  assert_eq!(server.live_session_count(), 1, "作り直したセッションが 1 つ生きています");
}

#[tokio::test]
async fn a_session_whose_initialization_fails_midway_is_closed() {
  // initialize は成功したが notifications/initialized が失敗した場合、サーバーはセッションを
  // 作り終えている。保存しないセッションを残さないよう、DELETE で閉じる。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(directory.path(), json!({ "goals": [] }));
  let server = HttpMockServer::start(&state, Some("fail-initialized"));
  let mut handle = HttpReadHandle::new(server.connection(), std::time::Duration::from_secs(10));

  handle.connect().await.expect_err("初期化の通知が失敗した接続は失敗です");

  assert_eq!(handle.session_count(), 0, "確立に失敗したセッションは保持しません");
  server.wait_for_live_sessions(0).await;
  assert_eq!(server.created_session_count(), 1, "サーバーはセッションを 1 つ作っていました");
}

fn answer_ids() -> (takt_dash_backend::domain::GoalId, takt_dash_backend::domain::InquiryId) {
  let goal_id =
    takt_dash_backend::domain::GoalId::try_from("11111111-1111-4111-8111-111111111111").expect("ゴール ID を作れます");
  let question_id =
    takt_dash_backend::domain::InquiryId::try_from("8f14e45f-ceea-4f6a-9a3c-0f1a2b3c4d5e").expect("質問 ID を作れます");
  (goal_id, question_id)
}

#[tokio::test]
async fn a_write_that_cannot_connect_on_an_established_session_is_reported_as_not_sent() {
  // 共有セッションが確立済みだと、書き込みの前の connect() は何もせずに通る。そのあとで
  // takt-mcp が止まっていると、道具を送る段で接続を張れない。要求は届いていないので、
  // 結果が不明な失敗（I/O エラー）ではなく、送る前の失敗として分けて返す。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(directory.path(), json!({}));
  let server = HttpMockServer::start(&state, None);
  let cwd = project_dir(directory.path());
  let session = HttpSession::new(server.connection());
  let timeout = std::time::Duration::from_secs(10);
  let mut reader = HttpReadHandle::with_session(session.clone(), timeout);
  reader.connect().await.expect("共有セッションを確立できます");
  drop(server);

  let mut writer = HttpReadHandle::with_session_for_write(session, timeout);
  writer.connect().await.expect("確立済みのセッションでは、接続の確認は何も送らずに通ります");
  let (goal_id, question_id) = answer_ids();
  let error = takt_dash_backend::mcp::WriteToolSet::new(&mut writer, cwd)
    .answer_goal_question(&goal_id, &question_id, json!({ "text": "はい" }))
    .await
    .expect_err("止まっているサーバーへの書き込みは失敗です");

  assert!(
    matches!(error, ToolError::Transport(TransportError::NotSent(_))),
    "接続を張れなかった書き込みは、送る前の失敗です: {error:?}"
  );
}

#[tokio::test]
async fn a_write_whose_lost_session_cannot_be_recreated_is_reported_as_not_sent() {
  // 未知セッション（404）を受けた書き込みは処理されていない。セッションの作り直しに失敗すれば
  // 再試行も送っていないので、送る前の失敗として返す。書き込み道具が処理された記録は無い。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(directory.path(), json!({}));
  let server = HttpMockServer::start(&state, Some("drop-session-then-refuse-initialize"));
  let cwd = project_dir(directory.path());
  let mut writer = HttpReadHandle::new_for_write(server.connection(), std::time::Duration::from_secs(10));
  writer.connect().await.expect("最初のセッションは確立できます");
  let (goal_id, question_id) = answer_ids();

  let error = takt_dash_backend::mcp::WriteToolSet::new(&mut writer, cwd)
    .answer_goal_question(&goal_id, &question_id, json!({ "text": "はい" }))
    .await
    .expect_err("セッションを作り直せない書き込みは失敗です");

  assert!(
    matches!(error, ToolError::Transport(TransportError::NotSent(_))),
    "作り直しに失敗した書き込みは、送る前の失敗です: {error:?}"
  );
  assert_eq!(server.created_session_count(), 1, "作り直しは成功していません");
}

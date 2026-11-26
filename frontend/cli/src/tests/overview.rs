//! 概要の取得と表示、接続不能時の異常終了。

use tonic::{Code, Status};

use super::{
  fixtures::{sample_overview, ts, INQUIRY_KIND_OPERATION_REQUEST},
  support::{reserve_closed_addr, run_cli, spawn_accept_then_close, start_dummy_server, start_overview_error_server},
};
use crate::{
  is_connection_failure,
  proto::v1::{DashboardState, PendingDecision, PendingDecisionType},
  render_overview, run_with, EXIT_FAILURE, EXIT_OK,
};

// ----------------------------------------------------------------------------
// CLI-OVERVIEW-RENDER: 概要の取得と表示（要件 1, 2, 4(a)）
// ----------------------------------------------------------------------------

#[test]
fn overview_command_labels_a_write_only_server_as_write_only() {
  let runtime =
    tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("テスト用の実行環境を作れませんでした");
  let mut state = sample_overview();
  state.servers[1].write_only = true;
  let server = runtime.block_on(start_dummy_server(state));

  let (code, stdout, stderr) = run_cli(&["overview".to_owned()], server.addr);
  runtime.block_on(server.shutdown());

  assert_eq!(code, EXIT_OK, "閲覧コマンドは成功する必要があります。stderr: {stderr}");
  assert!(
    stdout.contains("j5ik2o-mac-studio（Mac Studio、書き込み専用）"),
    "書き込み専用のサーバーは書き込み専用と表示する必要があります: {stdout}"
  );
  assert!(
    !stdout.contains("j5ik2o-mac-studio（Mac Studio、応答なし）"),
    "書き込み専用のサーバーを応答なしと表示してはいけません: {stdout}"
  );
}

#[test]
fn overview_command_prints_fetched_summary_to_stdout() {
  let runtime =
    tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("テスト用の実行環境を作れませんでした");
  let server = runtime.block_on(start_dummy_server(sample_overview()));

  let (code, stdout, stderr) = run_cli(&["overview".to_owned()], server.addr);
  let seen_calls = server.seen_calls();
  runtime.block_on(server.shutdown());

  assert_eq!(code, EXIT_OK, "閲覧コマンドは成功する必要があります。stderr: {stderr}");
  assert_eq!(seen_calls, vec!["GetOverview".to_owned()], "閲覧は概要の取得だけを呼ぶ必要があります");
  assert!(stderr.is_empty(), "成功時に標準エラーへ出してはいけません: {stderr}");
  assert!(stdout.contains("サーバー: 2台"), "サーバーの台数: {stdout}");
  assert!(stdout.contains("j5ik2o-mac-mini（Mac mini、接続中）"), "接続中のサーバー: {stdout}");
  assert!(stdout.contains("j5ik2o-mac-studio（Mac Studio、応答なし）"), "応答なしのサーバー: {stdout}");
  assert!(stdout.contains("taktdash-cli（taktDash の概要を表示する閲覧コマンド）"), "プロジェクト: {stdout}");
  assert!(
    stdout.contains("taktdash-cli/goal-cli「CLI に閲覧コマンドを足す」完了2件・失敗1件（未着手1件・実行中0件）"),
    "ゴールとタスクの件数: {stdout}"
  );
  assert!(stdout.contains("実行中の run: 1件"), "実行中の run の件数: {stdout}");
  assert!(stdout.contains("taktdash-cli/閲覧コマンドの実装（概要を表示する実装を進めています）"), "run: {stdout}");
  assert!(
    stdout.contains("taktdash-cli/終わったタスク（完了済みの run）"),
    "実行中でない run も一覧に出す必要があります: {stdout}"
  );
  assert!(
    stdout.contains("判断待ち: 3件（未回答の問い合わせ 1件、人のマージ待ち 1件、ゴール登録の承認待ち 1件）"),
    "判断待ちの件数と内訳: {stdout}"
  );
  assert!(stdout.contains("問い合わせ（質問）"), "問い合わせの種類: {stdout}");
  assert!(
    stdout.contains("taktdash-cli/mini: 承認待ち 新しいゴールを登録してください。"),
    "承認待ちは独立の種別で表示する必要があります: {stdout}"
  );
  assert!(
    !stdout.contains("問い合わせ（質問） 新しいゴールを登録してください。"),
    "承認待ちを問い合わせ（質問）と表示してはいけません: {stdout}"
  );
  assert!(stdout.contains("異常判定の閾値: 判断待ちの滞留 240分、ログの無音 10分"), "異常判定の閾値: {stdout}");
}

#[test]
fn overview_rendering_distinguishes_operation_request_inquiry() {
  let mut state = sample_overview();
  state.pending_decisions = vec![PendingDecision {
    r#type:          PendingDecisionType::Inquiry as i32,
    server_id:       "mini".to_owned(),
    project:         "taktdash-cli".to_owned(),
    goal_id:         None,
    inquiry_kind:    INQUIRY_KIND_OPERATION_REQUEST,
    created_at:      Some(ts(1_731_300_000)),
    body:            "ログを取得してください。".to_owned(),
    options:         Vec::new(),
    pull_request:    None,
    stale:           false,
    inquiry_id:      String::new(),
    revision:        None,
    merge_task_last: None,
    body_truncated:  false,
  }];

  let rendered = render_overview(&state).expect("有効です");

  assert!(
    rendered.contains("問い合わせ（作業の依頼）"),
    "作業の依頼は質問と区別して表示する必要があります: {rendered}"
  );
  assert!(
    rendered.contains("判断待ち: 1件（未回答の問い合わせ 1件、人のマージ待ち 0件、ゴール登録の承認待ち 0件）"),
    "作業の依頼は未回答の問い合わせに数える必要があります: {rendered}"
  );
  assert!(!rendered.contains("人のマージ待ち 1件"), "マージ待ちの件数を混同しないこと: {rendered}");
  assert!(rendered.contains("ゴール登録の承認待ち 0件"), "承認待ちの区分を出す必要があります: {rendered}");
  assert!(!rendered.contains("承認待ち ログ"), "作業の依頼を承認待ちと表示しないこと: {rendered}");
}

#[test]
fn overview_of_empty_state_renders_empty_markers() {
  let runtime =
    tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("テスト用の実行環境を作れませんでした");
  let server = runtime.block_on(start_dummy_server(DashboardState::default()));

  let (code, stdout, stderr) = run_cli(&[], server.addr);
  runtime.block_on(server.shutdown());

  // 位置引数を省略しても overview として動作する。
  assert_eq!(code, EXIT_OK, "位置引数を省略しても成功する必要があります。stderr: {stderr}");
  assert!(stdout.contains("サーバー: 0台"), "サーバーの台数: {stdout}");
  assert!(stdout.contains("実行中の run: 0件"), "実行中の run: {stdout}");
  assert!(
    stdout.contains("判断待ち: 0件（未回答の問い合わせ 0件、人のマージ待ち 0件、ゴール登録の承認待ち 0件）"),
    "判断待ち: {stdout}"
  );
  assert!(stdout.contains("（なし）"), "空の一覧は空であることが分かる表示にする必要があります: {stdout}");
}

#[test]
fn overview_rejects_unknown_run_status_from_backend() {
  // 未知の run status を既知値や件数へ丸めず、応答の不正として失敗させる。
  let mut state = sample_overview();
  state.runs[0].status = 999;

  let runtime =
    tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("テスト用の実行環境を作れませんでした");
  let server = runtime.block_on(start_dummy_server(state));

  let (code, stdout, stderr) = run_cli(&["overview".to_owned()], server.addr);
  runtime.block_on(server.shutdown());

  assert_eq!(code, EXIT_FAILURE, "未知の run status は取得の失敗として終わる必要があります: {stderr}");
  assert!(stdout.is_empty(), "不正な応答を表示してはいけません: {stdout}");
  assert!(stderr.contains("不正な値"), "応答が不正であることを示す必要があります: {stderr}");
  assert!(
    !stderr.contains("backend に接続できません。"),
    "backend が応答した不正値を接続不能として表示してはいけません: {stderr}"
  );
}

// ----------------------------------------------------------------------------
// CLI-UNAVAILABLE: 接続不能時の異常終了（要件 6, 7, 8, 4(b)）
// ----------------------------------------------------------------------------

#[test]
fn overview_fails_with_nonzero_code_when_backend_is_down() {
  let addr = reserve_closed_addr();
  let mut stdout = Vec::new();
  let mut stderr = Vec::new();

  let code = run_with(&["overview".to_owned()], Ok(format!("{addr}")), &mut stdout, &mut stderr);

  let stdout = String::from_utf8(stdout).expect("標準出力は UTF-8 である必要があります");
  let stderr = String::from_utf8(stderr).expect("標準エラーは UTF-8 である必要があります");
  assert_ne!(code, EXIT_OK, "接続できないときは 0 以外で終わる必要があります");
  assert_eq!(code, EXIT_FAILURE, "接続不能は取得の失敗として終わる必要があります");
  assert!(
    stderr.contains("backend に接続できません。"),
    "接続できない旨を標準エラーへ日本語で出す必要があります: {stderr}"
  );
  assert!(stderr.contains(&addr.to_string()), "接続先を標準エラーで示す必要があります: {stderr}");
  assert!(!stdout.contains("サーバー:"), "接続できないときは概要を標準出力へ出してはいけません: {stdout}");
}

#[test]
fn overview_treats_non_grpc_listener_as_connection_failure() {
  // 待ち受けはあるが gRPC を話さない相手（accept 後即時 close）でも、待ち受けが無い場合と
  // 同じ分類・文言・終了コードで異常終了する。
  let addr = spawn_accept_then_close();
  let mut stdout = Vec::new();
  let mut stderr = Vec::new();

  let code = run_with(&["overview".to_owned()], Ok(format!("{addr}")), &mut stdout, &mut stderr);

  let stdout = String::from_utf8(stdout).expect("標準出力は UTF-8 である必要があります");
  let stderr = String::from_utf8(stderr).expect("標準エラーは UTF-8 である必要があります");
  assert_eq!(code, EXIT_FAILURE, "gRPC を話さない相手は接続不能として終わる必要があります: {stderr}");
  assert!(
    stderr.starts_with("backend に接続できません。"),
    "待ち受けあり・gRPC を話さない相手でも同じ文言で始まる必要があります: {stderr}"
  );
  assert!(stderr.contains(&addr.to_string()), "接続先を標準エラーで示す必要があります: {stderr}");
  assert!(!stdout.contains("サーバー:"), "接続できないときは概要を標準出力へ出してはいけません: {stdout}");
}

#[test]
fn overview_reports_backend_failure_without_treating_it_as_connection_failure() {
  let runtime =
    tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("テスト用の実行環境を作れませんでした");
  let server = runtime.block_on(start_overview_error_server(Status::internal("概要の射影に失敗しました。")));

  let (code, stdout, stderr) = run_cli(&["overview".to_owned()], server.addr);
  runtime.block_on(server.shutdown());

  assert_eq!(code, EXIT_FAILURE, "backend が応答した失敗は取得の失敗として終わる必要があります");
  assert!(
    stderr.contains("backend から概要を取得できませんでした"),
    "backend の失敗を概要の取得失敗として表示する必要があります: {stderr}"
  );
  assert!(stderr.contains("概要の射影に失敗しました。"), "backend の失敗理由を表示する必要があります: {stderr}");
  assert!(
    !stderr.contains("backend に接続できません。"),
    "backend が応答した失敗を接続不能として表示してはいけません: {stderr}"
  );
  assert!(!stdout.contains("概要"), "取得に失敗したときは概要を表示してはいけません: {stdout}");
}

#[test]
fn connection_failure_classification_covers_cancelled_and_unknown() {
  // 実測で gRPC を話さない相手は Cancelled/Unknown を返す。backend が応答したうえでの失敗
  // （例如: NotFound）は接続不能にしない。
  assert!(is_connection_failure(Code::Unavailable));
  assert!(is_connection_failure(Code::DeadlineExceeded));
  assert!(is_connection_failure(Code::Cancelled));
  assert!(is_connection_failure(Code::Unknown));
  assert!(!is_connection_failure(Code::NotFound));
  assert!(!is_connection_failure(Code::PermissionDenied));
  assert!(!is_connection_failure(Code::Internal));
}

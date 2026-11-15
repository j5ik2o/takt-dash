//! 完了契約を、本番コードと同じ関数・同じ生成クライアント・ダミー gRPC サーバーで観測する。
//!
//! 表示の組み立ては `render_overview` を、引数と接続先の解決と終了コードは `run_with` を、
//! 実際の gRPC 通信は生成クライアントと本番と同じサービス定義のダミーサーバーを通して確かめる。

use std::{
  net::{Ipv4Addr, SocketAddr, TcpListener},
  sync::{Arc, Mutex},
  time::Duration,
};

use prost_types::Timestamp;
use proto::v1::{
  self, takt_dash_service_server::TaktDashServiceServer, DashboardState, GetOverviewRequest, PendingDecision,
  PendingDecisionType, Project, Server as ServerInfo, TaskStatusCounts,
};
use tonic::{transport::Server, Code, Request, Response, Status};

use super::{
  is_connection_failure, parse_arguments, proto, render_overview, resolve_backend_addr, run_with, AddrError,
  AddrSources, Invocation, OverviewOptions, UsageError, DEFAULT_BACKEND_ADDR, EXIT_FAILURE, EXIT_OK, EXIT_USAGE,
};

// proto の列挙値は生成コードの数値をそのまま使う。
const GOAL_STATUS_AWAITING_MERGE: i32 = v1::GoalStatus::AwaitingMerge as i32;
const EXECUTION_STATUS_ACTIVE: i32 = v1::ExecutionStatus::Active as i32;
const RUN_STATUS_RUNNING: i32 = v1::RunStatus::Running as i32;
const INQUIRY_KIND_QUESTION: i32 = v1::InquiryKind::Question as i32;
const INQUIRY_KIND_OPERATION_REQUEST: i32 = v1::InquiryKind::OperationRequest as i32;

/// タイムスタンプを組み立てる。`secs` は UNIX 時刻の秒。
fn ts(secs: i64) -> Timestamp {
  Timestamp { seconds: secs, nanos: 0 }
}

// ----------------------------------------------------------------------------
// ダミー gRPC サーバー
// ----------------------------------------------------------------------------

/// `start_dummy_server` が起動したダミーサーバーの起動結果。
struct DummyServer {
  addr:          SocketAddr,
  seen:          Arc<Mutex<Vec<String>>>,
  shutdown_tx:   tokio::sync::oneshot::Sender<()>,
  server_handle: tokio::task::JoinHandle<()>,
}

impl DummyServer {
  /// 記録した RPC 名の一覧を返す。
  fn seen_calls(&self) -> Vec<String> {
    self.seen.lock().expect("RPC 名の記録を読めませんでした").clone()
  }

  /// サーバーを止め、待ち受けを解放する。
  ///
  /// グレースフルシャットダウンを待ってからタスクの終了を確認する。停止を待たずにタスクごと
  /// 打ち切ると、進行中の RPC が応答を欠いたまま切断され、次の観測の前提を崩すため。
  async fn shutdown(self) {
    let _ = self.shutdown_tx.send(());
    tokio::time::timeout(Duration::from_secs(5), self.server_handle)
      .await
      .expect("ダミーサーバーは5秒以内に停止する必要があります")
      .expect("ダミーサーバーのタスクが異常終了しました");
  }
}

/// 概要を固定値で返すダミーサーバーを `127.0.0.1:0` で起動する。
async fn start_dummy_server(overview: DashboardState) -> DummyServer {
  let seen = Arc::new(Mutex::new(Vec::new()));
  let service = DummyTaktDashService { overview: Arc::new(overview), seen: seen.clone() };
  let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
    .await
    .expect("ダミーサーバーは空きポートで待ち受けできる必要があります");
  let addr = listener.local_addr().expect("待ち受けアドレスを取得できませんでした");
  let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
  let server_handle = tokio::spawn(async move {
    let _ = Server::builder()
      .add_service(TaktDashServiceServer::new(service))
      .serve_with_incoming_shutdown(tokio_stream::wrappers::TcpListenerStream::new(listener), async {
        let _ = shutdown_rx.await;
      })
      .await;
  });
  DummyServer { addr, seen, shutdown_tx, server_handle }
}

/// 本番と同じ生成サービス定義を実装するテスト用サーバー。
///
/// 閲覧は概要の取得だけを担うため、`GetOverview` だけを意味のある形で実装し、それ以外の
/// 読み取り RPC と状態ストリーミングは Unimplemented を返す。
struct DummyTaktDashService {
  overview: Arc<DashboardState>,
  seen:     Arc<Mutex<Vec<String>>>,
}

impl DummyTaktDashService {
  fn record(&self, name: &str) {
    self.seen.lock().expect("RPC 名の記録を書けませんでした").push(name.to_owned());
  }
}

#[tonic::async_trait]
impl v1::takt_dash_service_server::TaktDashService for DummyTaktDashService {
  type WatchStateStream = tokio_stream::wrappers::ReceiverStream<Result<DashboardState, Status>>;

  async fn get_overview(&self, _request: Request<GetOverviewRequest>) -> Result<Response<DashboardState>, Status> {
    self.record("GetOverview");
    Ok(Response::new((*self.overview).clone()))
  }

  async fn get_projects(
    &self,
    _request: Request<v1::GetProjectsRequest>,
  ) -> Result<Response<v1::GetProjectsResponse>, Status> {
    self.record("GetProjects");
    Err(Status::unimplemented("このダミーサーバーは GetProjects を提供しません。"))
  }

  async fn get_pending_decisions(
    &self,
    _request: Request<v1::GetPendingDecisionsRequest>,
  ) -> Result<Response<v1::GetPendingDecisionsResponse>, Status> {
    self.record("GetPendingDecisions");
    Err(Status::unimplemented("このダミーサーバーは GetPendingDecisions を提供しません。"))
  }

  async fn get_activity(
    &self,
    _request: Request<v1::GetActivityRequest>,
  ) -> Result<Response<v1::GetActivityResponse>, Status> {
    self.record("GetActivity");
    Err(Status::unimplemented("このダミーサーバーは GetActivity を提供しません。"))
  }

  async fn get_run_detail(
    &self,
    _request: Request<v1::GetRunDetailRequest>,
  ) -> Result<Response<v1::GetRunDetailResponse>, Status> {
    self.record("GetRunDetail");
    Err(Status::unimplemented("このダミーサーバーは GetRunDetail を提供しません。"))
  }

  async fn watch_state(
    &self,
    _request: Request<v1::WatchStateRequest>,
  ) -> Result<Response<Self::WatchStateStream>, Status> {
    self.record("WatchState");
    Err(Status::unimplemented("このダミーサーバーは WatchState を提供しません。"))
  }

  async fn answer_inquiry(
    &self,
    _request: Request<v1::AnswerInquiryRequest>,
  ) -> Result<Response<v1::AnswerInquiryResponse>, Status> {
    self.record("AnswerInquiry");
    Err(Status::unimplemented("このダミーサーバーは AnswerInquiry を提供しません。"))
  }

  async fn approve_goal_merge(
    &self,
    _request: Request<v1::ApproveGoalMergeRequest>,
  ) -> Result<Response<v1::ApproveGoalMergeResponse>, Status> {
    self.record("ApproveGoalMerge");
    Err(Status::unimplemented("このダミーサーバーは ApproveGoalMerge を提供しません。"))
  }

  async fn approve_goal_registration(
    &self,
    _request: Request<v1::ApproveGoalRegistrationRequest>,
  ) -> Result<Response<v1::ApproveGoalRegistrationResponse>, Status> {
    self.record("ApproveGoalRegistration");
    Err(Status::unimplemented("このダミーサーバーは ApproveGoalRegistration を提供しません。"))
  }

  async fn reject_goal_registration(
    &self,
    _request: Request<v1::RejectGoalRegistrationRequest>,
  ) -> Result<Response<v1::RejectGoalRegistrationResponse>, Status> {
    self.record("RejectGoalRegistration");
    Err(Status::unimplemented("このダミーサーバーは RejectGoalRegistration を提供しません。"))
  }

  async fn list_goal_registration_proposals(
    &self,
    _request: Request<v1::ListGoalRegistrationProposalsRequest>,
  ) -> Result<Response<v1::ListGoalRegistrationProposalsResponse>, Status> {
    self.record("ListGoalRegistrationProposals");
    Err(Status::unimplemented("このダミーサーバーは ListGoalRegistrationProposals を提供しません。"))
  }
}

/// 空きポートを確保して即座に閉じる。接続を拒否するアドレスを作る。
fn reserve_closed_addr() -> SocketAddr {
  let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("空きポートを確保できませんでした");
  let addr = listener.local_addr().expect("待ち受けアドレスを取得できませんでした");
  drop(listener);
  addr
}

/// 待ち受けはあるが gRPC を話さない相手を作る。接続を accept して即座に閉じる。
///
/// accept のループを回すスレッドはデーモンとして動かし、テストの終了とともにプロセスが終わる
/// ことで解放します。
fn spawn_accept_then_close() -> SocketAddr {
  let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("空きポートを確保できませんでした");
  let addr = listener.local_addr().expect("待ち受けアドレスを取得できませんでした");
  std::thread::spawn(move || {
    // 接続のたびに accept して即座に閉じる。gRPC の応答は返さない。
    while let Ok((socket, _)) = listener.accept() {
      drop(socket);
    }
  });
  addr
}

/// 一時ディレクトリに設定ファイルを書いて、そのパスを返す。
///
/// ユーザーの既存の設定を読まないよう、テストごとに一時ディレクトリへ隔離する。
fn write_config(dir: &tempfile::TempDir, contents: &str) -> std::path::PathBuf {
  let path = dir.path().join("taktdash.config.json");
  std::fs::write(&path, contents).expect("テスト用の設定ファイルを書けませんでした");
  path
}

/// `run_with` を、ダミーサーバーのアドレスと空の環境変数で実行する。
fn run_cli(args: &[String], addr: SocketAddr) -> (i32, String, String) {
  let mut stdout = Vec::new();
  let mut stderr = Vec::new();
  let env_addr = Ok(format!("{addr}"));
  let code = run_with(args, env_addr, &mut stdout, &mut stderr);
  (
    code,
    String::from_utf8(stdout).expect("標準出力は UTF-8 である必要があります"),
    String::from_utf8(stderr).expect("標準エラーは UTF-8 である必要があります"),
  )
}

// ----------------------------------------------------------------------------
// ダミーサーバーが返す固定の概要
// ----------------------------------------------------------------------------

fn sample_overview() -> DashboardState {
  DashboardState {
    read_at:           Some(ts(1_731_600_000)),
    servers:           vec![
      ServerInfo {
        server_id:    "mini".to_owned(),
        name:         "j5ik2o-mac-mini".to_owned(),
        kind:         "Mac mini".to_owned(),
        connected:    true,
        last_read_at: Some(ts(1_731_599_000)),
        concurrency:  4,
      },
      ServerInfo {
        server_id:    "studio".to_owned(),
        name:         "j5ik2o-mac-studio".to_owned(),
        kind:         "Mac Studio".to_owned(),
        connected:    false,
        last_read_at: None,
        concurrency:  8,
      },
    ],
    projects:          vec![Project {
      name:                "taktdash-cli".to_owned(),
      about:               "taktDash の概要を表示する閲覧コマンド".to_owned(),
      server_id:           "mini".to_owned(),
      completed_tasks_24h: 3,
    }],
    goals:             vec![v1::Goal {
      goal_id:           "goal-cli".to_owned(),
      server_id:         "mini".to_owned(),
      project:           "taktdash-cli".to_owned(),
      name:              "CLI に閲覧コマンドを足す".to_owned(),
      status:            GOAL_STATUS_AWAITING_MERGE,
      execution_status:  EXECUTION_STATUS_ACTIVE,
      task_counts:       Some(TaskStatusCounts {
        pending: 1,
        running: 0,
        completed: 2,
        failed: 1,
        ..Default::default()
      }),
      paused_reason:     None,
      pull_request:      Some(v1::PullRequest { number: 42, url: "https://example.test/pr/42".to_owned() }),
      awaiting_merge_at: Some(ts(1_731_500_000)),
      merge_task_last:   None,
    }],
    runs:              vec![
      v1::Run {
        run_slug:                "run-1".to_owned(),
        session_id:              "session-1".to_owned(),
        server_id:               "mini".to_owned(),
        project:                 "taktdash-cli".to_owned(),
        goal_id:                 "goal-cli".to_owned(),
        task_name:               "閲覧コマンドの実装".to_owned(),
        summary:                 "概要を表示する実装を進めています".to_owned(),
        started_at:              Some(ts(1_731_590_000)),
        steps:                   Vec::new(),
        current_step_started_at: None,
        last_log_at:             Some(ts(1_731_599_500)),
        reports:                 Vec::new(),
        status:                  RUN_STATUS_RUNNING,
        log_silent:              false,
      },
      // 実行中でない run を混ぜ、実行中の件数が全 run の件数と取り違えられないようにする。
      v1::Run {
        run_slug:                "run-2".to_owned(),
        session_id:              "session-2".to_owned(),
        server_id:               "studio".to_owned(),
        project:                 "taktdash-cli".to_owned(),
        goal_id:                 "goal-cli".to_owned(),
        task_name:               "終わったタスク".to_owned(),
        summary:                 "完了済みの run".to_owned(),
        started_at:              Some(ts(1_731_580_000)),
        steps:                   Vec::new(),
        current_step_started_at: None,
        last_log_at:             Some(ts(1_731_590_000)),
        reports:                 Vec::new(),
        status:                  v1::RunStatus::Completed as i32,
        log_silent:              false,
      },
    ],
    pending_decisions: vec![
      PendingDecision {
        r#type:       PendingDecisionType::Inquiry as i32,
        server_id:    "mini".to_owned(),
        project:      "taktdash-cli".to_owned(),
        goal_id:      Some("goal-cli".to_owned()),
        inquiry_kind: INQUIRY_KIND_QUESTION,
        created_at:   Some(ts(1_731_300_000)),
        body:         "表示の並び順はどれがよいですか。".to_owned(),
        options:      vec!["待ち時間の長い順".to_owned()],
        pull_request: None,
        stale:        false,
        inquiry_id:   String::new(),
        revision:     None,
      },
      PendingDecision {
        r#type:       PendingDecisionType::AwaitingMerge as i32,
        server_id:    "mini".to_owned(),
        project:      "taktdash-cli".to_owned(),
        goal_id:      Some("goal-cli".to_owned()),
        inquiry_kind: v1::InquiryKind::Unspecified as i32,
        created_at:   Some(ts(1_731_400_000)),
        body:         "プルリクエスト 42 をマージしてください。".to_owned(),
        options:      Vec::new(),
        pull_request: Some(v1::PullRequest { number: 42, url: "https://example.test/pr/42".to_owned() }),
        stale:        false,
        inquiry_id:   String::new(),
        revision:     None,
      },
      PendingDecision {
        r#type:       PendingDecisionType::GoalRegistration as i32,
        server_id:    "mini".to_owned(),
        project:      "taktdash-cli".to_owned(),
        goal_id:      None,
        inquiry_kind: v1::InquiryKind::Unspecified as i32,
        created_at:   Some(ts(1_731_500_000)),
        body:         "新しいゴールを登録してください。".to_owned(),
        options:      Vec::new(),
        pull_request: None,
        stale:        false,
        inquiry_id:   String::new(),
        revision:     Some(3),
      },
    ],
    thresholds:        Some(v1::AnomalyThresholds {
      pending_decision_stale_minutes: 240,
      run_log_silent_minutes:         10,
    }),
  }
}

// ----------------------------------------------------------------------------
// CLI-OVERVIEW-RENDER: 概要の取得と表示（要件 1, 2, 4(a)）
// ----------------------------------------------------------------------------

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
    r#type:       PendingDecisionType::Inquiry as i32,
    server_id:    "mini".to_owned(),
    project:      "taktdash-cli".to_owned(),
    goal_id:      None,
    inquiry_kind: INQUIRY_KIND_OPERATION_REQUEST,
    created_at:   Some(ts(1_731_300_000)),
    body:         "ログを取得してください。".to_owned(),
    options:      Vec::new(),
    pull_request: None,
    stale:        false,
    inquiry_id:   String::new(),
    revision:     None,
  }];

  let rendered = render_overview(&state);

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

// ----------------------------------------------------------------------------
// CLI-NO-WRITE: 書き込み操作のコマンドを持たない（要件 9）
// ----------------------------------------------------------------------------

#[test]
fn write_command_is_rejected_with_usage() {
  let mut stdout = Vec::new();
  let mut stderr = Vec::new();

  let code = run_with(&["answer".to_owned()], Ok(DEFAULT_BACKEND_ADDR.to_owned()), &mut stdout, &mut stderr);

  let stdout = String::from_utf8(stdout).expect("標準出力は UTF-8 である必要があります");
  let stderr = String::from_utf8(stderr).expect("標準エラーは UTF-8 である必要があります");
  assert_eq!(code, EXIT_USAGE, "未知のコマンドは使い方を出して異常終了する必要があります");
  assert!(stderr.contains("未知のコマンドです: answer"), "未知のコマンドであることを示す必要があります: {stderr}");
  assert!(stderr.contains("使い方: takt-dash-cli overview"), "使い方を示す必要があります: {stderr}");
  assert!(stdout.is_empty(), "使い方の誤りで標準出力へ出してはいけません: {stdout}");
}

#[test]
fn unknown_flag_is_rejected_with_usage() {
  let mut stdout = Vec::new();
  let mut stderr = Vec::new();

  let code = run_with(&["--merge".to_owned()], Ok(DEFAULT_BACKEND_ADDR.to_owned()), &mut stdout, &mut stderr);

  let stderr = String::from_utf8(stderr).expect("標準エラーは UTF-8 である必要があります");
  assert_eq!(code, EXIT_USAGE);
  assert!(stderr.contains("未知の引数です: --merge"), "未知の引数であることを示す必要があります: {stderr}");
}

#[test]
fn generated_client_exposes_the_read_and_write_calls() {
  // proto のサービス定義は読み取りの 6 つと書き込みの 5 つを持つ。CLI はこの版から
  // 書き込みを画面と共有する API として使うため、生成クライアントの型に両方が現れる。
  let _request_types = (
    std::marker::PhantomData::<proto::v1::GetOverviewRequest>,
    std::marker::PhantomData::<proto::v1::GetProjectsRequest>,
    std::marker::PhantomData::<proto::v1::GetPendingDecisionsRequest>,
    std::marker::PhantomData::<proto::v1::GetActivityRequest>,
    std::marker::PhantomData::<proto::v1::GetRunDetailRequest>,
    std::marker::PhantomData::<proto::v1::WatchStateRequest>,
    std::marker::PhantomData::<proto::v1::AnswerInquiryRequest>,
    std::marker::PhantomData::<proto::v1::ApproveGoalMergeRequest>,
    std::marker::PhantomData::<proto::v1::ApproveGoalRegistrationRequest>,
    std::marker::PhantomData::<proto::v1::RejectGoalRegistrationRequest>,
    std::marker::PhantomData::<proto::v1::ListGoalRegistrationProposalsRequest>,
  );
  let _response_types = (
    std::marker::PhantomData::<proto::v1::AnswerInquiryResponse>,
    std::marker::PhantomData::<proto::v1::ApproveGoalMergeResponse>,
    std::marker::PhantomData::<proto::v1::ApproveGoalRegistrationResponse>,
    std::marker::PhantomData::<proto::v1::RejectGoalRegistrationResponse>,
    std::marker::PhantomData::<proto::v1::ListGoalRegistrationProposalsResponse>,
  );
}

#[test]
fn generated_client_type_uses_the_transport_channel() {
  let _client: std::marker::PhantomData<
    proto::v1::takt_dash_service_client::TaktDashServiceClient<tonic::transport::Channel>,
  > = std::marker::PhantomData;
}

// ----------------------------------------------------------------------------
// CLI-ADDR-RESOLVE: 接続先の解決（要件 18, 19）
// ----------------------------------------------------------------------------

#[test]
fn explicit_addr_wins_over_env_and_config() {
  let sources = AddrSources {
    cli_addr:      Some("127.0.0.1:50101"),
    env_addr:      Ok("127.0.0.1:50202".to_owned()),
    config_listen: Some("127.0.0.1:50303"),
  };

  assert_eq!(resolve_backend_addr(&sources), Ok("127.0.0.1:50101".parse().unwrap()));
}

#[test]
fn env_wins_over_config_listen() {
  let sources = AddrSources {
    cli_addr:      None,
    env_addr:      Ok("127.0.0.1:50202".to_owned()),
    config_listen: Some("127.0.0.1:50303"),
  };

  assert_eq!(resolve_backend_addr(&sources), Ok("127.0.0.1:50202".parse().unwrap()));
}

#[test]
fn config_listen_used_when_addr_and_env_absent() {
  let sources = AddrSources {
    cli_addr:      None,
    env_addr:      Err(std::env::VarError::NotPresent),
    config_listen: Some("127.0.0.1:50303"),
  };

  assert_eq!(resolve_backend_addr(&sources), Ok("127.0.0.1:50303".parse().unwrap()));
}

#[test]
fn default_addr_used_when_nothing_configured() {
  let sources =
    AddrSources { cli_addr: None, env_addr: Err(std::env::VarError::NotPresent), config_listen: None };

  assert_eq!(resolve_backend_addr(&sources), Ok("127.0.0.1:50051".parse().unwrap()));
  assert_eq!(DEFAULT_BACKEND_ADDR, "127.0.0.1:50051");
}

#[test]
fn empty_values_fall_through_to_next_candidate() {
  let sources =
    AddrSources { cli_addr: Some(""), env_addr: Ok(String::new()), config_listen: Some("127.0.0.1:50303") };

  assert_eq!(resolve_backend_addr(&sources), Ok("127.0.0.1:50303".parse().unwrap()));
}

#[test]
fn invalid_addr_is_not_replaced_with_default() {
  let sources = AddrSources {
    cli_addr:      Some("not-an-address"),
    env_addr:      Err(std::env::VarError::NotPresent),
    config_listen: None,
  };

  assert_eq!(
    resolve_backend_addr(&sources),
    Err(AddrError::Invalid { source: "--addr", value: "not-an-address".to_owned() })
  );
}

#[test]
fn invalid_addr_exits_with_usage_error() {
  let mut stdout = Vec::new();
  let mut stderr = Vec::new();

  let code = run_with(
    &["overview".to_owned(), "--addr".to_owned(), "not-an-address".to_owned()],
    Err(std::env::VarError::NotPresent),
    &mut stdout,
    &mut stderr,
  );

  let stderr = String::from_utf8(stderr).expect("標準エラーは UTF-8 である必要があります");
  assert_eq!(code, EXIT_USAGE, "--addr の不正値は既定値へ置き換えず異常終了する必要があります");
  assert!(stderr.contains("not-an-address"), "不正な値を示す必要があります: {stderr}");
}

#[test]
fn parse_arguments_accepts_overview_and_flags() {
  let args = [
    "overview".to_owned(),
    "--addr".to_owned(),
    "127.0.0.1:50052".to_owned(),
    "--config".to_owned(),
    "/tmp/a.json".to_owned(),
  ];

  assert_eq!(
    parse_arguments(&args),
    Ok(Invocation::Overview(OverviewOptions {
      addr:   Some("127.0.0.1:50052".to_owned()),
      config: Some(std::path::PathBuf::from("/tmp/a.json")),
    }))
  );
}

#[test]
fn parse_arguments_defaults_to_overview_without_position() {
  assert_eq!(parse_arguments(&[]), Ok(Invocation::Overview(OverviewOptions::default())));
}

#[test]
fn parse_arguments_reports_help() {
  assert_eq!(parse_arguments(&["--help".to_owned()]), Ok(Invocation::Help));
}

#[test]
fn parse_arguments_requires_flag_value() {
  assert_eq!(parse_arguments(&["--addr".to_owned()]), Err(UsageError("--addr には値が必要です。".to_owned())));
}

#[test]
fn config_listen_overrides_default_for_connection() {
  let dir = tempfile::tempdir().expect("一時ディレクトリを作れませんでした");
  let config = write_config(&dir, r#"{"listen":"127.0.0.1:59999"}"#);

  let mut stdout = Vec::new();
  let mut stderr = Vec::new();
  let code = run_with(
    &["--config".to_owned(), config.display().to_string()],
    Err(std::env::VarError::NotPresent),
    &mut stdout,
    &mut stderr,
  );

  let stderr = String::from_utf8(stderr).expect("標準エラーは UTF-8 である必要があります");
  // 設定の listen が使われ（既定値ではない）、待ち受けが無いため接続不能で終わる。
  assert_eq!(code, EXIT_FAILURE, "設定の listen に待ち受けが無いため失敗する必要があります");
  assert!(stderr.contains("127.0.0.1:59999"), "設定の listen を接続先に使う必要があります: {stderr}");
  assert!(!stderr.contains(DEFAULT_BACKEND_ADDR), "既定値へ戻ってはいけません: {stderr}");
}

#[test]
fn missing_config_file_falls_through_to_default() {
  let dir = tempfile::tempdir().expect("一時ディレクトリを作れませんでした");
  let missing = dir.path().join("does-not-exist.json");
  let mut stdout = Vec::new();
  let mut stderr = Vec::new();

  let code = run_with(
    &["--config".to_owned(), missing.display().to_string()],
    Err(std::env::VarError::NotPresent),
    &mut stdout,
    &mut stderr,
  );

  let stderr = String::from_utf8(stderr).expect("標準エラーは UTF-8 である必要があります");
  assert_eq!(code, EXIT_FAILURE);
  assert!(stderr.contains(DEFAULT_BACKEND_ADDR), "設定ファイルが無いときは既定値へ進む必要があります: {stderr}");
}

// ----------------------------------------------------------------------------
// 到達経路: 引数解析・接続先解決・表示が一つの入口からつながる
// ----------------------------------------------------------------------------

#[test]
fn cli_end_to_end_from_arguments_to_stdout() {
  let runtime =
    tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("テスト用の実行環境を作れませんでした");
  let server = runtime.block_on(start_dummy_server(sample_overview()));

  // 環境変数ではなく --addr で接続先を渡す。表示は標準出力へ出る。
  let (code, stdout, stderr) = {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let args = ["overview".to_owned(), "--addr".to_owned(), format!("{}", server.addr)];
    let code = run_with(&args, Ok("127.0.0.1:1".to_owned()), &mut stdout, &mut stderr);
    (code, String::from_utf8(stdout).unwrap(), String::from_utf8(stderr).unwrap())
  };
  let seen_calls = server.seen_calls();
  runtime.block_on(server.shutdown());

  assert_eq!(code, EXIT_OK, "stderr: {stderr}");
  assert_eq!(seen_calls, vec!["GetOverview".to_owned()], "取得は GetOverview だけを呼ぶ必要があります");
  assert!(stdout.contains("概要"), "--addr の接続先から取得した概要を表示する必要があります: {stdout}");
  assert!(stdout.contains("j5ik2o-mac-mini（Mac mini、接続中）"), "{stdout}");
}

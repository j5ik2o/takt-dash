//! 完了契約を、本番コードと同じ関数・同じ生成クライアント・ダミー gRPC サーバーで観測する。
//!
//! 表示の組み立ては `render_overview` を、引数と接続先の解決と終了コードは `run_with` を、
//! 実際の gRPC 通信は生成クライアントと本番と同じサービス定義のダミーサーバーを通して確かめる。

use std::{
  collections::VecDeque,
  net::{Ipv4Addr, SocketAddr, TcpListener},
  sync::{Arc, Mutex},
  time::Duration,
};

use prost_types::Timestamp;
use proto::v1::{
  self, takt_dash_service_server::TaktDashServiceServer, ConversationMessage, ConversationSkipped, DashboardState,
  GetConversationRequest, GetConversationResponse, GetOverviewRequest, PendingDecision, PendingDecisionType, Project,
  Server as ServerInfo, TaskStatusCounts,
};
use tonic::{transport::Server, Code, Request, Response, Status};

use super::{
  is_connection_failure, parse_arguments, proto, read_config_listen, render_conversation, render_overview,
  resolve_backend_addr, resolve_overview_addr, run_with, AddrError, AddrSources, Answer, ConversationOptions,
  Invocation, OverviewOptions, UsageError, WriteCommand, WriteInvocation, ANSWER_COMMAND, BACKEND_ADDR_ENV,
  DEFAULT_BACKEND_ADDR, EXIT_FAILURE, EXIT_OK, EXIT_USAGE,
};
use crate::domain::{GoalId, InquiryId, MergeUntil, ProjectName, PullRequestNumber, ServerId};

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
  addr:                  SocketAddr,
  seen:                  Arc<Mutex<Vec<String>>>,
  conversation_requests: Arc<Mutex<Vec<GetConversationRequest>>>,
  shutdown_tx:           tokio::sync::oneshot::Sender<()>,
  server_handle:         tokio::task::JoinHandle<()>,
}

impl DummyServer {
  /// 記録した RPC 名の一覧を返す。
  fn seen_calls(&self) -> Vec<String> {
    self.seen.lock().expect("RPC 名の記録を読めませんでした").clone()
  }

  /// 受け取った `GetConversation` の要求を到着順に返す。
  fn conversation_requests(&self) -> Vec<GetConversationRequest> {
    self.conversation_requests.lock().expect("会話の要求の記録を読めませんでした").clone()
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
  start_dummy_server_with_conversation(overview, Vec::new()).await
}

/// 会話のページを呼び出しごとに1件ずつ返すダミーサーバーを起動する。
async fn start_conversation_server(
  overview: DashboardState,
  conversation_pages: Vec<GetConversationResponse>,
) -> DummyServer {
  start_dummy_server_with_conversation(overview, conversation_pages).await
}

/// 概要の取得を指定した失敗で返すダミーサーバーを起動する。
async fn start_overview_error_server(status: Status) -> DummyServer {
  start_dummy_server_with(Some(status), DashboardState::default(), Vec::new()).await
}

/// 概要と会話の固定応答を持つダミーサーバーを `127.0.0.1:0` で起動する。
async fn start_dummy_server_with_conversation(
  overview: DashboardState,
  conversation_pages: Vec<GetConversationResponse>,
) -> DummyServer {
  start_dummy_server_with(None, overview, conversation_pages).await
}

/// 概要の失敗と会話の固定応答を受け取ってダミーサーバーを `127.0.0.1:0` で起動する。
async fn start_dummy_server_with(
  overview_error: Option<Status>,
  overview: DashboardState,
  conversation_pages: Vec<GetConversationResponse>,
) -> DummyServer {
  let seen = Arc::new(Mutex::new(Vec::new()));
  let conversation_requests = Arc::new(Mutex::new(Vec::new()));
  let service = DummyTaktDashService {
    overview: Arc::new(overview),
    overview_error,
    seen: seen.clone(),
    conversation_pages: Arc::new(Mutex::new(VecDeque::from(conversation_pages))),
    conversation_requests: conversation_requests.clone(),
  };
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
  DummyServer { addr, seen, conversation_requests, shutdown_tx, server_handle }
}

/// 本番と同じ生成サービス定義を実装するテスト用サーバー。
///
/// 閲覧は概要と会話の取得だけを担うため、`GetOverview` と `GetConversation` だけを意味の
/// ある形で実装し、それ以外の読み取り RPC と状態ストリーミングは Unimplemented を返す。
struct DummyTaktDashService {
  overview:              Arc<DashboardState>,
  overview_error:        Option<Status>,
  seen:                  Arc<Mutex<Vec<String>>>,
  conversation_pages:    Arc<Mutex<VecDeque<GetConversationResponse>>>,
  conversation_requests: Arc<Mutex<Vec<GetConversationRequest>>>,
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
    match &self.overview_error {
      | Some(status) => Err(status.clone()),
      | None => Ok(Response::new((*self.overview).clone())),
    }
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

  async fn get_conversation(
    &self,
    request: Request<v1::GetConversationRequest>,
  ) -> Result<Response<v1::GetConversationResponse>, Status> {
    self.record("GetConversation");
    let request = request.into_inner();
    self.conversation_requests.lock().expect("会話の要求の記録を書けませんでした").push(request);
    let page = self.conversation_pages.lock().expect("会話のページを読めませんでした").pop_front();
    match page {
      | Some(page) => Ok(Response::new(page)),
      | None => Err(Status::not_found("このダミーサーバーには会話のページが残っていません。")),
    }
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
      connected:           true,
      last_read_at:        Some(ts(1_731_599_000)),
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
// ダミーサーバーが返す固定の会話
// ----------------------------------------------------------------------------

/// 会話の1件を組み立てる。`secs` は UNIX 時刻の秒。
fn conversation_message(seq: i64, secs: i64, speaker: v1::ConversationSpeaker, text: &str) -> ConversationMessage {
  ConversationMessage { seq, occurred_at: Some(ts(secs)), speaker: speaker as i32, text: text.to_owned() }
}

/// 会話の1ページを組み立てる。
fn conversation_page(
  project: &str,
  session_id: &str,
  messages: Vec<ConversationMessage>,
  next_after_seq: i64,
  has_more: bool,
) -> GetConversationResponse {
  GetConversationResponse {
    project: project.to_owned(),
    session_id: session_id.to_owned(),
    provider: "opencode".to_owned(),
    messages,
    next_after_seq,
    has_more,
    skipped: Vec::new(),
    unavailable_reason: None,
  }
}

/// 表示に使う UTC の日時表記を、本番と同じ書式で組み立てる。
fn formatted_time(secs: i64) -> String {
  chrono::DateTime::from_timestamp(secs, 0)
    .expect("テストの秒は有効な日時である必要があります")
    .format("%Y-%m-%d %H:%M:%S UTC")
    .to_string()
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

// ----------------------------------------------------------------------------
// CLI-USAGE: 書き込みコマンドの必須引数が欠けたときの使い方エラー
// ----------------------------------------------------------------------------

#[test]
fn write_command_missing_required_flag_is_rejected_with_usage() {
  let mut stdout = Vec::new();
  let mut stderr = Vec::new();

  // answer は --server/--project/--question と --text か --report が必須。欠けたら backend
  // を呼ばず使い方を出す。
  let code = run_with(&[ANSWER_COMMAND.to_owned()], Ok(DEFAULT_BACKEND_ADDR.to_owned()), &mut stdout, &mut stderr);

  let stdout = String::from_utf8(stdout).expect("標準出力は UTF-8 である必要があります");
  let stderr = String::from_utf8(stderr).expect("標準エラーは UTF-8 である必要があります");
  assert_eq!(code, EXIT_USAGE, "必須引数が欠けた書き込みコマンドは使い方を出して異常終了する必要があります");
  assert!(stderr.contains("answer には"), "欠けた引数を示す必要があります: {stderr}");
  assert!(stderr.contains("使い方: takt-dash-cli"), "使い方を示す必要があります: {stderr}");
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
fn parse_arguments_accepts_conversation_with_project_and_session() {
  let args = [
    "conversation",
    "corporate-site",
    "--session",
    "session-1",
    "--addr",
    "127.0.0.1:50052",
    "--config",
    "/tmp/a.json",
  ]
  .map(str::to_owned);

  assert_eq!(
    parse_arguments(&args),
    Ok(Invocation::Conversation(ConversationOptions {
      project: ProjectName::try_from("corporate-site").expect("有効です"),
      session: Some("session-1".to_owned()),
      addr:    Some("127.0.0.1:50052".to_owned()),
      config:  Some(std::path::PathBuf::from("/tmp/a.json")),
    }))
  );
}

#[test]
fn parse_arguments_requires_project_for_conversation() {
  let result = parse_arguments(&["conversation".to_owned()]);

  match result {
    | Err(UsageError(message)) => assert!(message.contains("プロジェクト"), "-- {message}"),
    | other => panic!("会話のプロジェクト未指定は使い方の誤りにする必要があります: {other:?}"),
  }
}

#[test]
fn parse_arguments_rejects_extra_project_position() {
  let args = ["conversation".to_owned(), "corporate-site".to_owned(), "esa".to_owned()];

  match parse_arguments(&args) {
    | Err(UsageError(message)) => assert!(message.contains("esa"), "-- {message}"),
    | other => panic!("会話の2個目の位置引数は使い方の誤りにする必要があります: {other:?}"),
  }
}

#[test]
fn parse_arguments_rejects_session_for_overview() {
  let args = ["overview".to_owned(), "--session".to_owned(), "session-1".to_owned()];

  match parse_arguments(&args) {
    | Err(UsageError(message)) => assert!(message.contains("--session"), "-- {message}"),
    | other => panic!("概要に --session は使い方の誤りにする必要があります: {other:?}"),
  }
}

// ----------------------------------------------------------------------------
// 書き込みコマンドの引数解釈
// ----------------------------------------------------------------------------

/// 書き込みコマンドの引数を組み立てるヘルパー。
fn write_args(parts: &[&str]) -> Vec<String> {
  parts.iter().map(|part| (*part).to_owned()).collect()
}

#[test]
fn parse_arguments_builds_answer_with_text() {
  let args = write_args(&[
    "answer",
    "--server",
    "mac-mini",
    "--project",
    "takt-dash",
    "--goal",
    "g-1",
    "--question",
    "q-1",
    "--text",
    "はい",
  ]);
  assert_eq!(
    parse_arguments(&args),
    Ok(Invocation::Write(WriteInvocation {
      command: WriteCommand::Answer {
        server_id:  ServerId::try_from("mac-mini").expect("有効です"),
        project:    ProjectName::try_from("takt-dash").expect("有効です"),
        goal_id:    Some(GoalId::try_from("g-1").expect("有効です")),
        inquiry_id: InquiryId::try_from("q-1").expect("有効です"),
        answer:     Answer::Text("はい".to_owned()),
      },
      addr:    None,
      config:  None,
    }))
  );
}

#[test]
fn parse_arguments_rejects_answer_with_text_and_report() {
  let args = write_args(&[
    "answer",
    "--server",
    "mac-mini",
    "--project",
    "takt-dash",
    "--question",
    "q-1",
    "--text",
    "はい",
    "--report",
    "completed",
  ]);
  assert!(parse_arguments(&args).is_err(), "--text と --report の同時指定は誤りです");
}

#[test]
fn parse_arguments_rejects_unknown_report_value() {
  let args =
    write_args(&["answer", "--server", "mac-mini", "--project", "takt-dash", "--question", "q-1", "--report", "done"]);
  assert!(parse_arguments(&args).is_err(), "--report は completed か unable だけを許します");
}

#[test]
fn parse_arguments_builds_merge_with_until() {
  let args = write_args(&[
    "merge",
    "--server",
    "mac-mini",
    "--project",
    "takt-dash",
    "--goal",
    "g-1",
    "--pr",
    "42",
    "--until",
    "merge",
  ]);
  assert_eq!(
    parse_arguments(&args),
    Ok(Invocation::Write(WriteInvocation {
      command: WriteCommand::Merge {
        server_id: ServerId::try_from("mac-mini").expect("有効です"),
        project:   ProjectName::try_from("takt-dash").expect("有効です"),
        goal_id:   GoalId::try_from("g-1").expect("有効です"),
        pr_number: PullRequestNumber::try_from(42).expect("有効です"),
        until:     MergeUntil::Merge,
      },
      addr:    None,
      config:  None,
    }))
  );
}

#[test]
fn parse_arguments_rejects_merge_with_unknown_until() {
  let args = write_args(&[
    "merge",
    "--server",
    "mac-mini",
    "--project",
    "takt-dash",
    "--goal",
    "g-1",
    "--pr",
    "42",
    "--until",
    "later",
  ]);
  assert!(parse_arguments(&args).is_err(), "--until は prepare か merge だけを許します");
}

#[test]
fn parse_arguments_rejects_a_flag_for_another_command() {
  // --pr は merge 専用。answer に付けたら未知の引数として拒否する。
  let args = write_args(&["answer", "--server", "mac-mini", "--project", "takt-dash", "--pr", "42"]);
  assert_eq!(parse_arguments(&args), Err(UsageError("未知の引数です: --pr".to_owned())));
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

  // 設定ファイルが無いときは listen を未設定として扱い、解決は既定値へ進む。
  let listen = read_config_listen(&missing).expect("存在しない設定ファイルは未設定として扱う必要があります");
  assert_eq!(listen, None, "存在しない設定ファイルは listen を返しません");
  let sources = AddrSources {
    cli_addr:      None,
    env_addr:      Err(std::env::VarError::NotPresent),
    config_listen: listen.as_deref(),
  };
  assert_eq!(
    resolve_backend_addr(&sources),
    Ok(DEFAULT_BACKEND_ADDR.parse().unwrap()),
    "設定ファイルが無いときは既定値へ進む必要があります"
  );

  // 接続結果は環境に依存するため、待ち受けの無い未使用ポートを明示して決定的に観測する。
  let closed = reserve_closed_addr();
  let mut stdout = Vec::new();
  let mut stderr = Vec::new();
  let code = run_with(
    &["--config".to_owned(), missing.display().to_string(), "--addr".to_owned(), closed.to_string()],
    Err(std::env::VarError::NotPresent),
    &mut stdout,
    &mut stderr,
  );

  let stderr = String::from_utf8(stderr).expect("標準エラーは UTF-8 である必要があります");
  assert_eq!(code, EXIT_FAILURE, "待ち受けの無い接続先では失敗する必要があります: {stderr}");
  assert!(stderr.contains(&closed.to_string()), "明示した接続先を使う必要があります: {stderr}");
  assert!(!stderr.contains(DEFAULT_BACKEND_ADDR), "明示した接続先があるときは既定値へ戻ってはいけません: {stderr}");
}

#[test]
fn explicit_addr_skips_config_read_even_if_config_is_invalid() {
  // 上位の接続先指定（--addr）がある場合、設定ファイルは読まない。壊れた設定を置いても
  // 解析エラーにならず、--addr のダミーサーバーから概要を取得できることを観測する。
  let runtime =
    tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("テスト用の実行環境を作れませんでした");
  let server = runtime.block_on(start_dummy_server(sample_overview()));
  let dir = tempfile::tempdir().expect("一時ディレクトリを作れませんでした");
  let config = write_config(&dir, "{ これは JSON ではありません }");
  let mut stdout = Vec::new();
  let mut stderr = Vec::new();
  let args = [
    "overview".to_owned(),
    "--addr".to_owned(),
    format!("{}", server.addr),
    "--config".to_owned(),
    config.display().to_string(),
  ];

  let code = run_with(&args, Err(std::env::VarError::NotPresent), &mut stdout, &mut stderr);
  let seen_calls = server.seen_calls();
  runtime.block_on(server.shutdown());

  let stdout = String::from_utf8(stdout).expect("標準出力は UTF-8 である必要があります");
  let stderr = String::from_utf8(stderr).expect("標準エラーは UTF-8 である必要があります");
  assert_eq!(code, EXIT_OK, "--addr があるときは設定ファイルの解析で止まってはいけません。stderr: {stderr}");
  assert!(stderr.is_empty(), "成功時に設定ファイルのエラーを出してはいけません: {stderr}");
  assert_eq!(seen_calls, vec!["GetOverview".to_owned()], "--addr の接続先から概要を取得する必要があります");
  assert!(stdout.contains("概要"), "--addr の接続先から取得した概要を表示する必要があります: {stdout}");
}

#[test]
fn env_addr_skips_config_read_even_if_config_is_invalid() {
  // 上位の接続先指定（環境変数）がある場合も設定ファイルは読まない。
  let runtime =
    tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("テスト用の実行環境を作れませんでした");
  let server = runtime.block_on(start_dummy_server(sample_overview()));
  let dir = tempfile::tempdir().expect("一時ディレクトリを作れませんでした");
  let config = write_config(&dir, "{ これは JSON ではありません }");
  let mut stdout = Vec::new();
  let mut stderr = Vec::new();
  let args = ["overview".to_owned(), "--config".to_owned(), config.display().to_string()];

  let code = run_with(&args, Ok(format!("{}", server.addr)), &mut stdout, &mut stderr);
  let seen_calls = server.seen_calls();
  runtime.block_on(server.shutdown());

  let stdout = String::from_utf8(stdout).expect("標準出力は UTF-8 である必要があります");
  let stderr = String::from_utf8(stderr).expect("標準エラーは UTF-8 である必要があります");
  assert_eq!(code, EXIT_OK, "環境変数があるときは設定ファイルの解析で止まってはいけません。stderr: {stderr}");
  assert!(stderr.is_empty(), "成功時に設定ファイルのエラーを出してはいけません: {stderr}");
  assert_eq!(seen_calls, vec!["GetOverview".to_owned()], "環境変数の接続先から概要を取得する必要があります");
  assert!(stdout.contains("概要"), "環境変数の接続先から取得した概要を表示する必要があります: {stdout}");
}

#[test]
fn invalid_config_fails_when_no_higher_addr_is_specified() {
  // 上位の指定が無い場合だけ設定ファイルを読み、解析できなければ接続前に失敗する。
  let dir = tempfile::tempdir().expect("一時ディレクトリを作れませんでした");
  let config = write_config(&dir, "{ これは JSON ではありません }");
  let mut stdout = Vec::new();
  let mut stderr = Vec::new();

  let code = run_with(
    &["overview".to_owned(), "--config".to_owned(), config.display().to_string()],
    Err(std::env::VarError::NotPresent),
    &mut stdout,
    &mut stderr,
  );

  let stdout = String::from_utf8(stdout).expect("標準出力は UTF-8 である必要があります");
  let stderr = String::from_utf8(stderr).expect("標準エラーは UTF-8 である必要があります");
  assert_eq!(code, EXIT_FAILURE, "上位の指定が無いときの設定エラーは失敗にする必要があります");
  assert!(stderr.contains("JSON を解析できませんでした"), "設定の解析失敗を示す必要があります: {stderr}");
  assert!(stdout.is_empty(), "設定エラーで概要を標準出力へ出してはいけません: {stdout}");
}

#[test]
fn empty_env_addr_reads_config_as_unset() {
  // 空の環境変数は「上位の指定なし」として扱い、設定ファイルを読む。
  let dir = tempfile::tempdir().expect("一時ディレクトリを作れませんでした");
  let config = write_config(&dir, "{ これは JSON ではありません }");
  let mut stdout = Vec::new();
  let mut stderr = Vec::new();

  let code = run_with(
    &["overview".to_owned(), "--config".to_owned(), config.display().to_string()],
    Ok(String::new()),
    &mut stdout,
    &mut stderr,
  );

  let stderr = String::from_utf8(stderr).expect("標準エラーは UTF-8 である必要があります");
  assert_eq!(code, EXIT_FAILURE, "空の環境変数は未設定として設定ファイルを読む必要があります");
  assert!(stderr.contains("JSON を解析できませんでした"), "設定の解析失敗を示す必要があります: {stderr}");
}

#[test]
fn invalid_env_addr_does_not_fall_through_to_config() {
  // 非空だが不正な環境変数を「未設定」と扱い、設定の listen へ落ちてはいけない。
  let dir = tempfile::tempdir().expect("一時ディレクトリを作れませんでした");
  let listen = reserve_closed_addr();
  let config = write_config(&dir, &format!(r#"{{"listen":"{listen}"}}"#));
  let mut stdout = Vec::new();
  let mut stderr = Vec::new();

  let code = run_with(
    &["overview".to_owned(), "--config".to_owned(), config.display().to_string()],
    Ok("not-an-address".to_owned()),
    &mut stdout,
    &mut stderr,
  );

  let stderr = String::from_utf8(stderr).expect("標準エラーは UTF-8 である必要があります");
  assert_eq!(code, EXIT_USAGE, "不正な環境変数は既定値や設定の listen で補ってはいけません");
  assert!(stderr.contains("not-an-address"), "不正な値を示す必要があります: {stderr}");
  assert!(stderr.contains(BACKEND_ADDR_ENV), "どの指定が不正かを示す必要があります: {stderr}");
}

#[test]
fn resolve_overview_addr_falls_back_to_default_when_config_file_is_missing() {
  // 所有者関数 resolve_overview_addr を通して、設定ファイルの欠落が「未設定」として
  // 既定値へ解決されることを、ソケットを開かずに観測する。
  let dir = tempfile::tempdir().expect("一時ディレクトリを作れませんでした");
  let missing = dir.path().join("does-not-exist.json");
  let options = OverviewOptions { addr: None, config: Some(missing) };

  assert_eq!(
    resolve_overview_addr(&options, Err(std::env::VarError::NotPresent)),
    Ok(DEFAULT_BACKEND_ADDR.parse::<SocketAddr>().unwrap()),
    "設定ファイルが無いときは所有者関数も既定値へ解決する必要があります"
  );
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

// ----------------------------------------------------------------------------
// CLI-CONVERSATION: 会話の取得と表示（AC38B-3/AC38B-4）
// ----------------------------------------------------------------------------

#[test]
fn conversation_rendering_marks_empty_conversation() {
  let rendered =
    render_conversation(&conversation_page("corporate-site", "session-1", Vec::new(), 0, false)).expect("有効です");

  assert!(rendered.contains("会話: corporate-site"), "{rendered}");
  assert!(
    rendered.contains("（会話はありません）"),
    "空の会話は空であることが分かる表示にする必要があります: {rendered}"
  );
}

#[test]
fn conversation_command_prints_messages_in_seq_order_and_follows_has_more() {
  let runtime =
    tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("テスト用の実行環境を作れませんでした");
  let first_secs = 1_731_600_000;
  let second_secs = 1_731_600_060;
  let third_secs = 1_731_600_120;
  let pages = vec![
    {
      let mut first_page = conversation_page(
        "corporate-site",
        "session-1",
        vec![
          conversation_message(1, first_secs, v1::ConversationSpeaker::User, "最初の発言"),
          conversation_message(2, second_secs, v1::ConversationSpeaker::Manager, "次の発言"),
        ],
        4,
        true,
      );
      first_page.skipped = vec![ConversationSkipped { reason: "解釈できない行".to_owned(), count: 1 }];
      first_page
    },
    conversation_page(
      "corporate-site",
      "session-other",
      vec![conversation_message(4, third_secs, v1::ConversationSpeaker::System, "最後の発言")],
      4,
      false,
    ),
  ];
  let server = runtime.block_on(start_conversation_server(sample_overview(), pages));

  let (code, stdout, stderr) = run_cli(&["conversation".to_owned(), "corporate-site".to_owned()], server.addr);
  let requests = server.conversation_requests();
  runtime.block_on(server.shutdown());

  assert_eq!(code, EXIT_OK, "会話の表示は成功する必要があります。stderr: {stderr}");
  assert!(stderr.is_empty(), "成功時に標準エラーへ出してはいけません: {stderr}");
  assert!(stdout.contains("会話: corporate-site"), "対象プロジェクト: {stdout}");
  assert!(
    stdout.contains("セッション: session-1"),
    "最初の応答の session_id を表示し、2ページ目の値で置き換えない: {stdout}"
  );
  assert!(stdout.contains("プロバイダ: opencode"), "プロバイダ: {stdout}");
  assert!(stdout.contains("ユーザー"), "ユーザーの発言: {stdout}");
  assert!(stdout.contains("manager"), "manager の発言: {stdout}");
  assert!(stdout.contains("システム"), "システムの発言: {stdout}");
  assert!(stdout.contains(&formatted_time(first_secs)), "発言の時刻: {stdout}");

  let first = stdout.find("最初の発言").expect("最初の発言を表示する必要があります");
  let second = stdout.find("次の発言").expect("次の発言を表示する必要があります");
  let third = stdout.find("最後の発言").expect("最後の発言を表示する必要があります");
  assert!(first < second && second < third, "会話は時系列で並ぶ必要があります: {stdout}");

  assert_eq!(requests.len(), 2, "has_more の間は続きを取得する必要があります");
  assert_eq!(requests[0].project, "corporate-site");
  assert!(requests[0].session_id.is_none(), "初回は session_id を省略する");
  assert_eq!(requests[0].after_seq, 0);
  assert_eq!(requests[0].limit, 200);
  assert_eq!(requests[1].session_id.as_deref(), Some("session-1"), "以降は最初の session_id を固定する");
  assert_eq!(requests[1].after_seq, 4, "続きは末尾メッセージの seq ではなく next_after_seq から取る");
}

#[test]
fn conversation_command_passes_session_id_to_backend() {
  let runtime =
    tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("テスト用の実行環境を作れませんでした");
  let server = runtime.block_on(start_conversation_server(sample_overview(), vec![conversation_page(
    "corporate-site",
    "session-9",
    Vec::new(),
    0,
    false,
  )]));

  let (code, _stdout, stderr) = run_cli(
    &["conversation".to_owned(), "corporate-site".to_owned(), "--session".to_owned(), "session-9".to_owned()],
    server.addr,
  );
  let requests = server.conversation_requests();
  runtime.block_on(server.shutdown());

  assert_eq!(code, EXIT_OK, "stderr: {stderr}");
  assert_eq!(requests.len(), 1);
  assert_eq!(requests[0].session_id.as_deref(), Some("session-9"));
}

#[test]
fn conversation_command_shows_unavailable_reason() {
  let runtime =
    tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("テスト用の実行環境を作れませんでした");
  let mut page = conversation_page("corporate-site", "session-1", Vec::new(), 0, false);
  page.unavailable_reason = Some("会話の保存先がありません。".to_owned());
  let server = runtime.block_on(start_conversation_server(sample_overview(), vec![page]));

  let (code, stdout, stderr) = run_cli(&["conversation".to_owned(), "corporate-site".to_owned()], server.addr);
  runtime.block_on(server.shutdown());

  assert_eq!(code, EXIT_OK, "unavailable_reason は取得の成功として扱う必要があります。stderr: {stderr}");
  assert!(
    stdout.contains("会話を表示できません: 会話の保存先がありません。"),
    "会話が無い理由を表示する必要があります: {stdout}"
  );
}

#[test]
fn conversation_command_shows_skipped_rows() {
  let runtime =
    tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("テスト用の実行環境を作れませんでした");
  let mut page = conversation_page("corporate-site", "session-1", Vec::new(), 0, false);
  page.skipped = vec![ConversationSkipped { reason: "解釈できない行".to_owned(), count: 2 }];
  let server = runtime.block_on(start_conversation_server(sample_overview(), vec![page]));

  let (code, stdout, stderr) = run_cli(&["conversation".to_owned(), "corporate-site".to_owned()], server.addr);
  runtime.block_on(server.shutdown());

  assert_eq!(code, EXIT_OK, "stderr: {stderr}");
  assert!(
    stdout.contains("スキップした行: 解釈できない行 2件"),
    "スキップした行の理由と件数を表示する必要があります: {stdout}"
  );
}

#[test]
fn conversation_command_fails_when_backend_is_down() {
  let addr = reserve_closed_addr();
  let mut stdout = Vec::new();
  let mut stderr = Vec::new();

  let code = run_with(
    &["conversation".to_owned(), "corporate-site".to_owned()],
    Ok(format!("{addr}")),
    &mut stdout,
    &mut stderr,
  );

  let stdout = String::from_utf8(stdout).expect("標準出力は UTF-8 である必要があります");
  let stderr = String::from_utf8(stderr).expect("標準エラーは UTF-8 である必要があります");
  assert_eq!(code, EXIT_FAILURE, "接続できないときは取得の失敗として終わる必要があります");
  assert!(stderr.contains("backend に接続できません。"), "接続できない旨を標準エラーへ出す必要があります: {stderr}");
  assert!(!stdout.contains("会話:"), "接続できないときは会話を表示してはいけません: {stdout}");
}

#[test]
fn conversation_reports_backend_failure_without_treating_it_as_connection_failure() {
  // 会話のページを持たないダミーサーバーは GetConversation に NotFound を返す。
  let runtime =
    tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("テスト用の実行環境を作れませんでした");
  let server = runtime.block_on(start_conversation_server(sample_overview(), Vec::new()));

  let (code, stdout, stderr) = run_cli(&["conversation".to_owned(), "corporate-site".to_owned()], server.addr);
  runtime.block_on(server.shutdown());

  assert_eq!(code, EXIT_FAILURE, "backend が応答した失敗は取得の失敗として終わる必要があります");
  assert!(
    stderr.contains("backend から会話を取得できませんでした"),
    "backend の失敗を会話の取得失敗として表示する必要があります: {stderr}"
  );
  assert!(
    stderr.contains("このダミーサーバーには会話のページが残っていません。"),
    "backend の失敗理由を表示する必要があります: {stderr}"
  );
  assert!(
    !stderr.contains("backend に接続できません。"),
    "backend が応答した失敗を接続不能として表示してはいけません: {stderr}"
  );
  assert!(!stdout.contains("会話:"), "取得に失敗したときは会話を表示してはいけません: {stdout}");
}

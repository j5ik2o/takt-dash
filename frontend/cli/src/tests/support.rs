//! ダミー gRPC サーバーと、CLI を実行する補助関数。

use std::{
  collections::VecDeque,
  net::{Ipv4Addr, SocketAddr, TcpListener},
  sync::{Arc, Mutex},
  time::Duration,
};

use tonic::{transport::Server, Request, Response, Status};

use crate::{
  proto::v1::{
    self, takt_dash_service_server::TaktDashServiceServer, DashboardState, GetConversationRequest,
    GetConversationResponse, GetOverviewRequest,
  },
  run_with,
};

// ----------------------------------------------------------------------------
// ダミー gRPC サーバー
// ----------------------------------------------------------------------------

/// `start_dummy_server` が起動したダミーサーバーの起動結果。
pub(super) struct DummyServer {
  pub(super) addr:       SocketAddr,
  seen:                  Arc<Mutex<Vec<String>>>,
  conversation_requests: Arc<Mutex<Vec<GetConversationRequest>>>,
  goal_detail_requests:  Arc<Mutex<Vec<v1::GetGoalDetailRequest>>>,
  timeouts:              Arc<Mutex<Vec<(String, String)>>>,
  shutdown_tx:           tokio::sync::oneshot::Sender<()>,
  server_handle:         tokio::task::JoinHandle<()>,
}

impl DummyServer {
  /// 記録した RPC 名の一覧を返す。
  pub(super) fn seen_calls(&self) -> Vec<String> {
    self.seen.lock().expect("RPC 名の記録を読めませんでした").clone()
  }

  /// 受け取った `GetConversation` の要求を到着順に返す。
  pub(super) fn conversation_requests(&self) -> Vec<GetConversationRequest> {
    self.conversation_requests.lock().expect("会話の要求の記録を読めませんでした").clone()
  }

  /// 受け取った `GetGoalDetail` の要求を到着順に返す。
  pub(super) fn goal_detail_requests(&self) -> Vec<v1::GetGoalDetailRequest> {
    self.goal_detail_requests.lock().expect("ゴール詳細の要求の記録を読めませんでした").clone()
  }

  /// 受け取った読み取り RPC の `grpc-timeout` を、RPC 名と到着順に返す。
  pub(super) fn timeouts(&self) -> Vec<(String, String)> {
    self.timeouts.lock().expect("grpc-timeout の記録を読めませんでした").clone()
  }

  /// サーバーを止め、待ち受けを解放する。
  ///
  /// グレースフルシャットダウンを待ってからタスクの終了を確認する。停止を待たずにタスクごと
  /// 打ち切ると、進行中の RPC が応答を欠いたまま切断され、次の観測の前提を崩すため。
  pub(super) async fn shutdown(self) {
    let _ = self.shutdown_tx.send(());
    tokio::time::timeout(Duration::from_secs(5), self.server_handle)
      .await
      .expect("ダミーサーバーは5秒以内に停止する必要があります")
      .expect("ダミーサーバーのタスクが異常終了しました");
  }
}

/// 概要を固定値で返すダミーサーバーを `127.0.0.1:0` で起動する。
pub(super) async fn start_dummy_server(overview: DashboardState) -> DummyServer {
  start_dummy_server_with_conversation(overview, Vec::new()).await
}

/// 会話のページを呼び出しごとに1件ずつ返すダミーサーバーを起動する。
pub(super) async fn start_conversation_server(
  overview: DashboardState,
  conversation_pages: Vec<GetConversationResponse>,
) -> DummyServer {
  start_dummy_server_with_conversation(overview, conversation_pages).await
}

/// 概要の取得を指定した失敗で返すダミーサーバーを起動する。
pub(super) async fn start_overview_error_server(status: Status) -> DummyServer {
  start_dummy_server_with(Some(status), DashboardState::default(), Vec::new()).await
}

/// 概要と会話の固定応答を持つダミーサーバーを `127.0.0.1:0` で起動する。
async fn start_dummy_server_with_conversation(
  overview: DashboardState,
  conversation_pages: Vec<GetConversationResponse>,
) -> DummyServer {
  start_dummy_server_with(None, overview, conversation_pages).await
}

/// ゴール一覧の固定応答を持つダミーサーバーを `127.0.0.1:0` で起動する。
pub(super) async fn start_goals_server(goals: Vec<v1::Goal>) -> DummyServer {
  start_dummy_server_with_goals(
    None,
    DashboardState::default(),
    Vec::new(),
    Some(Ok(v1::GetGoalsResponse { goals })),
    None,
  )
  .await
}

/// ゴール一覧の取得を指定した失敗で返すダミーサーバーを起動する。
pub(super) async fn start_goals_error_server(status: Status) -> DummyServer {
  start_dummy_server_with_goals(None, DashboardState::default(), Vec::new(), Some(Err(status)), None).await
}

/// ゴール詳細の固定応答を持つダミーサーバーを `127.0.0.1:0` で起動する。
pub(super) async fn start_goal_detail_server(detail: v1::GetGoalDetailResponse) -> DummyServer {
  start_dummy_server_with_goals(None, DashboardState::default(), Vec::new(), None, Some(Ok(detail))).await
}

/// ゴール詳細の取得を指定した失敗で返すダミーサーバーを起動する。
pub(super) async fn start_goal_detail_error_server(status: Status) -> DummyServer {
  start_dummy_server_with_goals(None, DashboardState::default(), Vec::new(), None, Some(Err(status))).await
}

/// 概要の失敗と会話の固定応答を受け取ってダミーサーバーを `127.0.0.1:0` で起動する。
async fn start_dummy_server_with(
  overview_error: Option<Status>,
  overview: DashboardState,
  conversation_pages: Vec<GetConversationResponse>,
) -> DummyServer {
  start_dummy_server_with_goals(overview_error, overview, conversation_pages, None, None).await
}

/// 概要・会話・ゴールの固定応答を受け取ってダミーサーバーを `127.0.0.1:0` で起動する。
pub(super) async fn start_dummy_server_with_goals(
  overview_error: Option<Status>,
  overview: DashboardState,
  conversation_pages: Vec<GetConversationResponse>,
  goals: Option<Result<v1::GetGoalsResponse, Status>>,
  goal_detail: Option<Result<v1::GetGoalDetailResponse, Status>>,
) -> DummyServer {
  let seen = Arc::new(Mutex::new(Vec::new()));
  let conversation_requests = Arc::new(Mutex::new(Vec::new()));
  let goal_detail_requests = Arc::new(Mutex::new(Vec::new()));
  let timeouts = Arc::new(Mutex::new(Vec::new()));
  let service = DummyTaktDashService {
    overview: Arc::new(overview),
    overview_error,
    seen: seen.clone(),
    conversation_pages: Arc::new(Mutex::new(VecDeque::from(conversation_pages))),
    conversation_requests: conversation_requests.clone(),
    goals,
    goal_detail,
    goal_detail_requests: goal_detail_requests.clone(),
    timeouts: timeouts.clone(),
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
  DummyServer { addr, seen, conversation_requests, goal_detail_requests, timeouts, shutdown_tx, server_handle }
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
  goals:                 Option<Result<v1::GetGoalsResponse, Status>>,
  goal_detail:           Option<Result<v1::GetGoalDetailResponse, Status>>,
  goal_detail_requests:  Arc<Mutex<Vec<v1::GetGoalDetailRequest>>>,
  timeouts:              Arc<Mutex<Vec<(String, String)>>>,
}

impl DummyTaktDashService {
  fn record(&self, name: &str) {
    self.seen.lock().expect("RPC 名の記録を書けませんでした").push(name.to_owned());
  }

  fn record_timeout<T>(&self, name: &str, request: &Request<T>) {
    if let Some(timeout) = request.metadata().get("grpc-timeout").and_then(|value| value.to_str().ok()) {
      self.timeouts.lock().expect("grpc-timeout の記録を書けませんでした").push((name.to_owned(), timeout.to_owned()));
    }
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

  async fn get_goals(&self, request: Request<v1::GetGoalsRequest>) -> Result<Response<v1::GetGoalsResponse>, Status> {
    self.record("GetGoals");
    self.record_timeout("GetGoals", &request);
    match &self.goals {
      | Some(Ok(goals)) => Ok(Response::new(goals.clone())),
      | Some(Err(status)) => Err(status.clone()),
      | None => Err(Status::unimplemented("このダミーサーバーは GetGoals を提供しません。")),
    }
  }

  async fn get_goal_detail(
    &self,
    request: Request<v1::GetGoalDetailRequest>,
  ) -> Result<Response<v1::GetGoalDetailResponse>, Status> {
    self.record("GetGoalDetail");
    self.record_timeout("GetGoalDetail", &request);
    let request = request.into_inner();
    self.goal_detail_requests.lock().expect("ゴール詳細の要求の記録を書けませんでした").push(request);
    match &self.goal_detail {
      | Some(Ok(detail)) => Ok(Response::new(detail.clone())),
      | Some(Err(status)) => Err(status.clone()),
      | None => Err(Status::unimplemented("このダミーサーバーは GetGoalDetail を提供しません。")),
    }
  }

  async fn get_task_detail(
    &self,
    _request: Request<v1::GetTaskDetailRequest>,
  ) -> Result<Response<v1::GetTaskDetailResponse>, Status> {
    self.record("GetTaskDetail");
    Err(Status::unimplemented("このダミーサーバーは GetTaskDetail を提供しません。"))
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
pub(super) fn reserve_closed_addr() -> SocketAddr {
  let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("空きポートを確保できませんでした");
  let addr = listener.local_addr().expect("待ち受けアドレスを取得できませんでした");
  drop(listener);
  addr
}

/// 待ち受けはあるが gRPC を話さない相手を作る。接続を accept して即座に閉じる。
///
/// accept のループを回すスレッドはデーモンとして動かし、テストの終了とともにプロセスが終わる
/// ことで解放します。
pub(super) fn spawn_accept_then_close() -> SocketAddr {
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
pub(super) fn write_config(dir: &tempfile::TempDir, contents: &str) -> std::path::PathBuf {
  let path = dir.path().join("taktdash.config.json");
  std::fs::write(&path, contents).expect("テスト用の設定ファイルを書けませんでした");
  path
}

/// `run_with` を、ダミーサーバーのアドレスと空の環境変数で実行する。
pub(super) fn run_cli(args: &[String], addr: SocketAddr) -> (i32, String, String) {
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

/// 書き込みコマンドの引数を組み立てるヘルパー。
pub(super) fn write_args(parts: &[&str]) -> Vec<String> {
  parts.iter().map(|part| (*part).to_owned()).collect()
}

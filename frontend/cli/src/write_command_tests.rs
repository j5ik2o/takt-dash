//! 書き込みコマンドの契約を、本番と同じ `run_with` と、本番と同じ生成サービス定義のダミー
//! gRPC サーバーで観測する。
//!
//! 目的は、問い合わせへの回答・マージ承認・ゴール登録の承認と却下・承認待ちの一覧が、
//! CLI の入口（`run_with`）から stdout / stderr / 終了コードへ一続きに届くことを確かめる
//! ことです。ダミーサーバーは受け取った要求を記録し、テストはその要求フィールドを直接
//! 観測します。`--addr` はダミーサーバーの空きポートを明示し、環境の既定アドレス
//! （実 backend が待ち受けていることがある 127.0.0.1:50051）には依存しません。

use std::{
  net::{Ipv4Addr, SocketAddr},
  sync::{Arc, Mutex},
  time::Duration,
};

use tonic::{transport::Server, Request, Response, Status};
use v1::takt_dash_service_server::TaktDashServiceServer;

use super::{proto::v1, run_with, EXIT_FAILURE, EXIT_OK, EXIT_USAGE};

// ----------------------------------------------------------------------------
// ダミー gRPC サーバー
// ----------------------------------------------------------------------------

/// 書き込み RPC が受け取った要求。テストは要求フィールドを観測します。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Recorded {
  Answer {
    server_id:  String,
    project:    String,
    goal_id:    Option<String>,
    inquiry_id: String,
    text:       Option<String>,
    report:     Option<i32>,
  },
  Merge {
    server_id: String,
    project:   String,
    goal_id:   String,
    pr_number: i64,
    until:     String,
  },
  ListProposals {
    server_id: String,
    project:   String,
  },
  ApproveRegistration {
    server_id: String,
    project:   String,
    revision:  i32,
  },
  RejectRegistration {
    server_id: String,
    project:   String,
    revision:  i32,
  },
}

/// ダミーサーバーの応答と、指定した RPC 名で返す失敗。
#[derive(Clone)]
struct DummyConfig {
  answer_inquiry_id: String,
  merge_task_name:   String,
  merge_workflow:    String,
  approve_goal_id:   String,
  proposals:         Vec<v1::GoalRegistrationProposal>,
  error:             Option<(&'static str, Status)>,
}

impl Default for DummyConfig {
  fn default() -> Self {
    DummyConfig {
      answer_inquiry_id: "8f14e45f-ceea-4f6a-9a3c-0f1a2b3c4d5e".to_owned(),
      merge_task_name:   "仕上げタスク".to_owned(),
      merge_workflow:    "goal-finish".to_owned(),
      approve_goal_id:   "3f9c1a54-2b41-4d1e-9d2e-2c1a5b8f7e11".to_owned(),
      proposals:         Vec::new(),
      error:             None,
    }
  }
}

impl DummyConfig {
  fn error_for(&self, method: &str) -> Option<Status> {
    self.error.as_ref().filter(|(name, _)| *name == method).map(|(_, status)| status.clone())
  }
}

struct DummyServer {
  addr:          SocketAddr,
  recorded:      Arc<Mutex<Vec<Recorded>>>,
  shutdown_tx:   tokio::sync::oneshot::Sender<()>,
  server_handle: tokio::task::JoinHandle<()>,
}

impl DummyServer {
  fn recorded(&self) -> Vec<Recorded> {
    self.recorded.lock().expect("要求の記録を読めませんでした").clone()
  }

  async fn shutdown(self) {
    let _ = self.shutdown_tx.send(());
    tokio::time::timeout(Duration::from_secs(5), self.server_handle)
      .await
      .expect("ダミーサーバーは5秒以内に停止する必要があります")
      .expect("ダミーサーバーのタスクが異常終了しました");
  }
}

async fn start_dummy_server(config: DummyConfig) -> DummyServer {
  let recorded = Arc::new(Mutex::new(Vec::new()));
  let service = DummyService { config: Arc::new(config), recorded: recorded.clone() };
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
  DummyServer { addr, recorded, shutdown_tx, server_handle }
}

struct DummyService {
  config:   Arc<DummyConfig>,
  recorded: Arc<Mutex<Vec<Recorded>>>,
}

impl DummyService {
  fn record(&self, call: Recorded) {
    self.recorded.lock().expect("要求の記録を書けませんでした").push(call);
  }
}

#[tonic::async_trait]
impl v1::takt_dash_service_server::TaktDashService for DummyService {
  type WatchStateStream = tokio_stream::wrappers::ReceiverStream<Result<v1::DashboardState, Status>>;

  async fn get_overview(
    &self,
    _request: Request<v1::GetOverviewRequest>,
  ) -> Result<Response<v1::DashboardState>, Status> {
    Err(Status::unimplemented("このダミーサーバーは GetOverview を提供しません。"))
  }

  async fn get_projects(
    &self,
    _request: Request<v1::GetProjectsRequest>,
  ) -> Result<Response<v1::GetProjectsResponse>, Status> {
    Err(Status::unimplemented("このダミーサーバーは GetProjects を提供しません。"))
  }

  async fn get_pending_decisions(
    &self,
    _request: Request<v1::GetPendingDecisionsRequest>,
  ) -> Result<Response<v1::GetPendingDecisionsResponse>, Status> {
    Err(Status::unimplemented("このダミーサーバーは GetPendingDecisions を提供しません。"))
  }

  async fn get_activity(
    &self,
    _request: Request<v1::GetActivityRequest>,
  ) -> Result<Response<v1::GetActivityResponse>, Status> {
    Err(Status::unimplemented("このダミーサーバーは GetActivity を提供しません。"))
  }

  async fn get_run_detail(
    &self,
    _request: Request<v1::GetRunDetailRequest>,
  ) -> Result<Response<v1::GetRunDetailResponse>, Status> {
    Err(Status::unimplemented("このダミーサーバーは GetRunDetail を提供しません。"))
  }

  async fn watch_state(
    &self,
    _request: Request<v1::WatchStateRequest>,
  ) -> Result<Response<Self::WatchStateStream>, Status> {
    Err(Status::unimplemented("このダミーサーバーは WatchState を提供しません。"))
  }

  async fn answer_inquiry(
    &self,
    request: Request<v1::AnswerInquiryRequest>,
  ) -> Result<Response<v1::AnswerInquiryResponse>, Status> {
    if let Some(status) = self.config.error_for("AnswerInquiry") {
      return Err(status);
    }
    let request = request.into_inner();
    self.record(Recorded::Answer {
      server_id:  request.server_id,
      project:    request.project,
      goal_id:    request.goal_id,
      inquiry_id: request.inquiry_id,
      text:       request.text,
      report:     request.report,
    });
    Ok(Response::new(v1::AnswerInquiryResponse { inquiry_id: self.config.answer_inquiry_id.clone() }))
  }

  async fn approve_goal_merge(
    &self,
    request: Request<v1::ApproveGoalMergeRequest>,
  ) -> Result<Response<v1::ApproveGoalMergeResponse>, Status> {
    if let Some(status) = self.config.error_for("ApproveGoalMerge") {
      return Err(status);
    }
    let request = request.into_inner();
    self.record(Recorded::Merge {
      server_id: request.server_id,
      project:   request.project,
      goal_id:   request.goal_id,
      pr_number: request.pr_number,
      until:     request.until,
    });
    Ok(Response::new(v1::ApproveGoalMergeResponse {
      task_name: self.config.merge_task_name.clone(),
      workflow:  self.config.merge_workflow.clone(),
    }))
  }

  async fn approve_goal_registration(
    &self,
    request: Request<v1::ApproveGoalRegistrationRequest>,
  ) -> Result<Response<v1::ApproveGoalRegistrationResponse>, Status> {
    if let Some(status) = self.config.error_for("ApproveGoalRegistration") {
      return Err(status);
    }
    let request = request.into_inner();
    self.record(Recorded::ApproveRegistration {
      server_id: request.server_id,
      project:   request.project,
      revision:  request.revision,
    });
    Ok(Response::new(v1::ApproveGoalRegistrationResponse {
      revision: request.revision,
      goal_id:  self.config.approve_goal_id.clone(),
    }))
  }

  async fn reject_goal_registration(
    &self,
    request: Request<v1::RejectGoalRegistrationRequest>,
  ) -> Result<Response<v1::RejectGoalRegistrationResponse>, Status> {
    if let Some(status) = self.config.error_for("RejectGoalRegistration") {
      return Err(status);
    }
    let request = request.into_inner();
    self.record(Recorded::RejectRegistration {
      server_id: request.server_id,
      project:   request.project,
      revision:  request.revision,
    });
    Ok(Response::new(v1::RejectGoalRegistrationResponse { revision: request.revision }))
  }

  async fn list_goal_registration_proposals(
    &self,
    request: Request<v1::ListGoalRegistrationProposalsRequest>,
  ) -> Result<Response<v1::ListGoalRegistrationProposalsResponse>, Status> {
    if let Some(status) = self.config.error_for("ListGoalRegistrationProposals") {
      return Err(status);
    }
    let request = request.into_inner();
    self.record(Recorded::ListProposals { server_id: request.server_id, project: request.project });
    Ok(Response::new(v1::ListGoalRegistrationProposalsResponse { proposals: self.config.proposals.clone() }))
  }
}

// ----------------------------------------------------------------------------
// 共通ヘルパー
// ----------------------------------------------------------------------------

/// `run_with` を、環境変数を持たない状態で `--addr` にダミーサーバーを指して実行する。
fn run_against(server: &DummyServer, args: &[&str]) -> (i32, String, String) {
  let mut full: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
  full.push("--addr".to_owned());
  full.push(format!("{}", server.addr));
  let mut stdout = Vec::new();
  let mut stderr = Vec::new();
  let code = run_with(&full, Err(std::env::VarError::NotPresent), &mut stdout, &mut stderr);
  (
    code,
    String::from_utf8(stdout).expect("標準出力は UTF-8 である必要があります"),
    String::from_utf8(stderr).expect("標準エラーは UTF-8 である必要があります"),
  )
}

/// テスト用の非同期実行環境を作る。
fn runtime() -> tokio::runtime::Runtime {
  tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("テスト用の実行環境を作れませんでした")
}

// ----------------------------------------------------------------------------
// answer --goal <ID> --question <ID> (--text | --report)
// ----------------------------------------------------------------------------

#[test]
fn answer_sends_free_text_with_server_project_goal_and_inquiry_id() {
  let rt = runtime();
  let server =
    rt.block_on(start_dummy_server(DummyConfig { answer_inquiry_id: "q-1".to_owned(), ..DummyConfig::default() }));

  let (code, stdout, stderr) = run_against(&server, &[
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
  let recorded = server.recorded();
  rt.block_on(server.shutdown());

  assert_eq!(code, EXIT_OK, "回答は成功する必要があります: {stderr}");
  assert_eq!(
    recorded,
    vec![Recorded::Answer {
      server_id:  "mac-mini".to_owned(),
      project:    "takt-dash".to_owned(),
      goal_id:    Some("g-1".to_owned()),
      inquiry_id: "q-1".to_owned(),
      text:       Some("はい".to_owned()),
      report:     None,
    }],
    "回答は server_id・project・goal_id・inquiry_id・text をそのまま送る必要があります"
  );
  assert!(stdout.contains("q-1"), "応答の問い合わせ識別子を表示する必要があります: {stdout}");
}

#[test]
fn answer_sends_completed_report_for_an_operation_request() {
  let rt = runtime();
  let server = rt.block_on(start_dummy_server(DummyConfig::default()));

  let (code, _stdout, stderr) = run_against(&server, &[
    "answer",
    "--server",
    "mac-mini",
    "--project",
    "takt-dash",
    "--goal",
    "g-1",
    "--question",
    "q-1",
    "--report",
    "completed",
  ]);
  let recorded = server.recorded();
  rt.block_on(server.shutdown());

  assert_eq!(code, EXIT_OK, "作業の依頼への回答は成功する必要があります: {stderr}");
  assert_eq!(
    recorded,
    vec![Recorded::Answer {
      server_id:  "mac-mini".to_owned(),
      project:    "takt-dash".to_owned(),
      goal_id:    Some("g-1".to_owned()),
      inquiry_id: "q-1".to_owned(),
      text:       None,
      report:     Some(v1::OperationAnswerReport::Completed as i32),
    }],
    "--report completed は OPERATION_ANSWER_REPORT_COMPLETED として送る必要があります"
  );
}

#[test]
fn answer_requires_text_or_report() {
  let rt = runtime();
  let server = rt.block_on(start_dummy_server(DummyConfig::default()));

  let (code, _stdout, _stderr) = run_against(&server, &[
    "answer",
    "--server",
    "mac-mini",
    "--project",
    "takt-dash",
    "--goal",
    "g-1",
    "--question",
    "q-1",
  ]);
  let recorded = server.recorded();
  rt.block_on(server.shutdown());

  assert_eq!(code, EXIT_USAGE, "--text も --report も無い回答は使い方の誤りで終わる必要があります");
  assert!(recorded.is_empty(), "使い方の誤りでは backend を呼んではいけません: {recorded:?}");
}

// ----------------------------------------------------------------------------
// merge --goal <ID> --pr <番号> [--until prepare|merge]
// ----------------------------------------------------------------------------

#[test]
fn merge_sends_goal_pr_number_and_until() {
  let rt = runtime();
  let server = rt.block_on(start_dummy_server(DummyConfig::default()));

  let (code, stdout, stderr) = run_against(&server, &[
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
  let recorded = server.recorded();
  rt.block_on(server.shutdown());

  assert_eq!(code, EXIT_OK, "マージ承認は成功する必要があります: {stderr}");
  assert_eq!(
    recorded,
    vec![Recorded::Merge {
      server_id: "mac-mini".to_owned(),
      project:   "takt-dash".to_owned(),
      goal_id:   "g-1".to_owned(),
      pr_number: 42,
      until:     "merge".to_owned(),
    }],
    "マージ承認は server_id・project・goal_id・pr_number・until を送る必要があります"
  );
  assert!(stdout.contains("仕上げタスク"), "依頼したタスク名を表示する必要があります: {stdout}");
}

#[test]
fn merge_rejects_a_non_positive_pr_number_without_calling_backend() {
  let rt = runtime();
  let server = rt.block_on(start_dummy_server(DummyConfig::default()));

  let (code, _stdout, _stderr) = run_against(&server, &[
    "merge",
    "--server",
    "mac-mini",
    "--project",
    "takt-dash",
    "--goal",
    "g-1",
    "--pr",
    "0",
    "--until",
    "merge",
  ]);
  let recorded = server.recorded();
  rt.block_on(server.shutdown());

  assert_eq!(code, EXIT_USAGE, "正でない PR 番号は使い方の誤りで終わる必要があります");
  assert!(recorded.is_empty(), "使い方の誤りでは backend を呼んではいけません: {recorded:?}");
}

// ----------------------------------------------------------------------------
// registrations / approve-registration / reject-registration
// ----------------------------------------------------------------------------

#[test]
fn registrations_lists_the_pending_proposals() {
  let rt = runtime();
  let server = rt.block_on(start_dummy_server(DummyConfig {
    proposals: vec![v1::GoalRegistrationProposal {
      revision:             7,
      server_id:            "mac-mini".to_owned(),
      project:              "takt-dash".to_owned(),
      body:                 "新しいゴールの提案".to_owned(),
      repository_directory: "/abs/takt-dash".to_owned(),
    }],
    ..DummyConfig::default()
  }));

  let (code, stdout, stderr) =
    run_against(&server, &["registrations", "--server", "mac-mini", "--project", "takt-dash"]);
  let recorded = server.recorded();
  rt.block_on(server.shutdown());

  assert_eq!(code, EXIT_OK, "承認待ちの一覧は成功する必要があります: {stderr}");
  assert_eq!(
    recorded,
    vec![Recorded::ListProposals { server_id: "mac-mini".to_owned(), project: "takt-dash".to_owned() }],
    "一覧は server_id と project を送る必要があります"
  );
  assert!(stdout.contains('7'), "revision を表示する必要があります: {stdout}");
  assert!(stdout.contains("新しいゴールの提案"), "提案の本文を表示する必要があります: {stdout}");
}

#[test]
fn approve_registration_sends_the_revision() {
  let rt = runtime();
  let server = rt.block_on(start_dummy_server(DummyConfig::default()));

  let (code, stdout, stderr) = run_against(&server, &[
    "approve-registration",
    "--server",
    "mac-mini",
    "--project",
    "takt-dash",
    "--revision",
    "7",
  ]);
  let recorded = server.recorded();
  rt.block_on(server.shutdown());

  assert_eq!(code, EXIT_OK, "承認は成功する必要があります: {stderr}");
  assert_eq!(
    recorded,
    vec![Recorded::ApproveRegistration {
      server_id: "mac-mini".to_owned(),
      project:   "takt-dash".to_owned(),
      revision:  7,
    }],
    "承認は server_id・project・revision を送る必要があります"
  );
  assert!(
    stdout.contains("3f9c1a54-2b41-4d1e-9d2e-2c1a5b8f7e11"),
    "登録されたゴール ID を表示する必要があります: {stdout}"
  );
}

#[test]
fn reject_registration_calls_reject_and_not_approve() {
  let rt = runtime();
  let server = rt.block_on(start_dummy_server(DummyConfig::default()));

  let (code, _stdout, stderr) =
    run_against(&server, &["reject-registration", "--server", "mac-mini", "--project", "takt-dash", "--revision", "7"]);
  let recorded = server.recorded();
  rt.block_on(server.shutdown());

  assert_eq!(code, EXIT_OK, "却下は成功する必要があります: {stderr}");
  assert_eq!(
    recorded,
    vec![Recorded::RejectRegistration {
      server_id: "mac-mini".to_owned(),
      project:   "takt-dash".to_owned(),
      revision:  7,
    }],
    "却下は RejectGoalRegistration だけを呼ぶ必要があります"
  );
}

// ----------------------------------------------------------------------------
// 失敗の分類と終了コード
// ----------------------------------------------------------------------------

#[test]
fn backend_error_is_reported_as_failure_with_nonzero_code() {
  let rt = runtime();
  let server = rt.block_on(start_dummy_server(DummyConfig {
    error: Some(("ApproveGoalMerge", Status::invalid_argument("goal_id が空です。"))),
    ..DummyConfig::default()
  }));

  let (code, _stdout, stderr) = run_against(&server, &[
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
  rt.block_on(server.shutdown());

  assert_eq!(code, EXIT_FAILURE, "backend の失敗は異常終了する必要があります");
  assert!(stderr.contains("goal_id が空です。"), "backend の失敗理由を表示する必要があります: {stderr}");
}

#[test]
fn connection_failure_is_reported_without_panicking() {
  // ダミーサーバーを停止してから呼び、接続不能の分類を観測する。
  let rt = runtime();
  let server = rt.block_on(start_dummy_server(DummyConfig::default()));
  let addr = server.addr;
  rt.block_on(server.shutdown());

  let mut stdout = Vec::new();
  let mut stderr = Vec::new();
  let code = run_with(
    &[
      "answer".to_owned(),
      "--server".to_owned(),
      "mac-mini".to_owned(),
      "--project".to_owned(),
      "takt-dash".to_owned(),
      "--goal".to_owned(),
      "g-1".to_owned(),
      "--question".to_owned(),
      "q-1".to_owned(),
      "--text".to_owned(),
      "はい".to_owned(),
      "--addr".to_owned(),
      format!("{addr}"),
    ],
    Err(std::env::VarError::NotPresent),
    &mut stdout,
    &mut stderr,
  );

  assert_eq!(code, EXIT_FAILURE, "接続できないときは異常終了する必要があります");
  assert!(String::from_utf8_lossy(&stderr).contains("接続できません"), "接続不能であることを表示する必要があります");
}

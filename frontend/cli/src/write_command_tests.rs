//! 書き込みコマンドの契約を、本番と同じ `run_with` と、本番と同じ生成サービス定義のダミー
//! gRPC サーバーで観測する。
//!
//! 目的は、問い合わせへの回答・マージ承認・ゴール登録の承認と却下・承認待ちの一覧が、
//! CLI の入口（`run_with`）から stdout / stderr / 終了コードへ一続きに届くことを確かめる
//! ことです。ダミーサーバーは受け取った要求を記録し、テストはその要求フィールドを直接
//! 観測します。`--addr` はダミーサーバーの空きポートを明示し、環境の既定アドレス
//! （実 backend が待ち受けていることがある 127.0.0.1:50051）には依存しません。

use std::{
  collections::BTreeMap,
  net::{Ipv4Addr, SocketAddr},
  sync::{Arc, Mutex},
  time::Duration,
};

use prost_types::Timestamp;
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
  pending:           Option<v1::DashboardState>,
  delays:            BTreeMap<&'static str, Duration>,
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
      pending:           None,
      delays:            BTreeMap::new(),
      error:             None,
    }
  }
}

impl DummyConfig {
  fn error_for(&self, method: &str) -> Option<Status> {
    self.error.as_ref().filter(|(name, _)| *name == method).map(|(_, status)| status.clone())
  }

  /// 指定した RPC 名の遅延を再現する。
  async fn delay_for(&self, method: &str) {
    if let Some(delay) = self.delays.get(method) {
      tokio::time::sleep(*delay).await;
    }
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
    tokio::time::timeout(Duration::from_secs(15), self.server_handle)
      .await
      .expect("ダミーサーバーは15秒以内に停止する必要があります")
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
    if let Some(status) = self.config.error_for("GetOverview") {
      return Err(status);
    }
    self.config.delay_for("GetOverview").await;
    Ok(Response::new(self.config.pending.clone().unwrap_or_default()))
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
    if let Some(status) = self.config.error_for("GetPendingDecisions") {
      return Err(status);
    }
    self.config.delay_for("GetPendingDecisions").await;
    Ok(Response::new(v1::GetPendingDecisionsResponse { state: self.config.pending.clone() }))
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
    self.config.delay_for("AnswerInquiry").await;
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

// ----------------------------------------------------------------------------
// decisions --server <名前> --project <名前>
// ----------------------------------------------------------------------------

/// 固定ラベルの行から値を取り出す。見つからなければテストを失敗させる。
fn value_line(stdout: &str, label: &str) -> String {
  stdout
    .lines()
    .find_map(|line| line.strip_prefix(label))
    .map(str::trim)
    .filter(|value| !value.is_empty())
    .map(str::to_owned)
    .unwrap_or_else(|| panic!("出力に {label} の行がありません: {stdout}"))
}

fn timestamp(seconds: i64) -> Timestamp {
  Timestamp { seconds, nanos: 0 }
}

/// 問い合わせ・マージ待ち・別プロジェクトの問い合わせを含む判断待ちの状態。
fn pending_state() -> v1::DashboardState {
  v1::DashboardState {
    read_at:           Some(timestamp(1_700_000_000)),
    servers:           Vec::new(),
    projects:          Vec::new(),
    goals:             Vec::new(),
    runs:              Vec::new(),
    pending_decisions: vec![
      v1::PendingDecision {
        r#type:       v1::PendingDecisionType::Inquiry as i32,
        server_id:    "mac-mini".to_owned(),
        project:      "takt-dash".to_owned(),
        goal_id:      Some("33333333-3333-4333-8333-333333333333".to_owned()),
        inquiry_kind: v1::InquiryKind::Question as i32,
        created_at:   Some(timestamp(1_700_000_000)),
        body:         "続けてよいですか".to_owned(),
        options:      vec!["yes".to_owned(), "no".to_owned()],
        pull_request: None,
        stale:        false,
        inquiry_id:   "q-1".to_owned(),
        revision:     None,
      },
      v1::PendingDecision {
        r#type:       v1::PendingDecisionType::AwaitingMerge as i32,
        server_id:    "mac-mini".to_owned(),
        project:      "takt-dash".to_owned(),
        goal_id:      Some("33333333-3333-4333-8333-333333333333".to_owned()),
        inquiry_kind: v1::InquiryKind::Unspecified as i32,
        created_at:   Some(timestamp(1_700_000_000)),
        body:         "マージ待ちのゴール".to_owned(),
        options:      Vec::new(),
        pull_request: Some(v1::PullRequest { number: 12, url: "https://example.test/pr/12".to_owned() }),
        stale:        false,
        inquiry_id:   String::new(),
        revision:     None,
      },
      v1::PendingDecision {
        r#type:       v1::PendingDecisionType::Inquiry as i32,
        server_id:    "other-server".to_owned(),
        project:      "other".to_owned(),
        goal_id:      Some("44444444-4444-4444-8444-444444444444".to_owned()),
        inquiry_kind: v1::InquiryKind::Question as i32,
        created_at:   Some(timestamp(1_700_000_000)),
        body:         "別プロジェクトの問い合わせ".to_owned(),
        options:      Vec::new(),
        pull_request: None,
        stale:        false,
        inquiry_id:   "q-other".to_owned(),
        revision:     None,
      },
    ],
    thresholds:        Some(v1::AnomalyThresholds {
      pending_decision_stale_minutes: 240,
      run_log_silent_minutes:         10,
    }),
  }
}

/// 判断待ち一覧の表示テスト用に、登録提案（承認待ち）を加えた状態を作る。
///
/// 共有の `pending_state()` は他のテストと共有しているため変更せず、承認待ちの種別表示と
/// リビジョンの描画を検証できるよう、登録提案を追加した複製を返します。
fn decisions_pending_state() -> v1::DashboardState {
  let mut state = pending_state();
  state.pending_decisions.push(v1::PendingDecision {
    r#type:       v1::PendingDecisionType::GoalRegistration as i32,
    server_id:    "mac-mini".to_owned(),
    project:      "takt-dash".to_owned(),
    goal_id:      None,
    inquiry_kind: v1::InquiryKind::Unspecified as i32,
    created_at:   Some(timestamp(1_700_000_000)),
    body:         "API を追加".to_owned(),
    options:      Vec::new(),
    pull_request: None,
    stale:        false,
    inquiry_id:   String::new(),
    revision:     Some(7),
  });
  state
}

/// 一覧のうち、指定した種別ラベルで始まる項目の表示ブロックを返す。
fn decision_block<'a>(stdout: &'a str, kind_label: &str) -> Vec<&'a str> {
  let header = format!("- {kind_label}: ");
  let mut block = Vec::new();
  let mut in_block = false;
  for line in stdout.lines() {
    if line.starts_with("- ") {
      if in_block {
        break;
      }
      in_block = line.starts_with(&header);
    }
    if in_block {
      block.push(line);
    }
  }
  if block.is_empty() {
    panic!("出力に {kind_label} の項目がありません: {stdout}");
  }
  block
}

#[test]
fn decisions_shows_ids_that_answer_and_merge_accept() {
  let rt = runtime();
  let server =
    rt.block_on(start_dummy_server(DummyConfig { pending: Some(decisions_pending_state()), ..DummyConfig::default() }));

  let (code, stdout, stderr) = run_against(&server, &["decisions", "--server", "mac-mini", "--project", "takt-dash"]);
  assert_eq!(code, EXIT_OK, "判断待ちの一覧は成功する必要があります: {stderr}");
  assert!(!stdout.contains("q-other"), "絞り込んだサーバー・プロジェクト以外の判断を表示しません: {stdout}");

  let inquiry_id = value_line(&stdout, "問い合わせID: ");
  let goal_id = value_line(&stdout, "ゴールID: ");
  let pr_number = value_line(&stdout, "プルリクエスト番号: ");
  assert_eq!(inquiry_id, "q-1", "問い合わせ ID を表示する必要があります: {stdout}");
  assert_eq!(goal_id, "33333333-3333-4333-8333-333333333333", "ゴール ID を表示する必要があります: {stdout}");
  assert_eq!(pr_number, "12", "プルリクエスト番号を表示する必要があります: {stdout}");

  let inquiry = decision_block(&stdout, "問い合わせ（質問）");
  assert!(inquiry.contains(&"選択肢: yes、no"), "問い合わせの選択肢を完全一致で表示する必要があります: {inquiry:?}");
  assert!(
    inquiry.contains(&"作成時刻: 2023-11-14 22:13:20 UTC"),
    "問い合わせの作成時刻を表示する必要があります: {inquiry:?}"
  );
  assert!(
    inquiry.contains(&"滞留期限: 2023-11-15 02:13:20 UTC"),
    "問い合わせの滞留期限を作成時刻と滞留分数から表示する必要があります: {inquiry:?}"
  );

  let approval = decision_block(&stdout, "承認待ち");
  assert!(approval.contains(&"リビジョン: 7"), "登録提案のリビジョンを表示する必要があります: {approval:?}");
  assert!(
    approval.iter().all(|line| !line.starts_with("作成時刻: ") && !line.starts_with("滞留期限: ")),
    "承認待ちに架空の期限行を出してはいけません: {approval:?}"
  );

  let (answer_code, _answer_stdout, answer_stderr) = run_against(&server, &[
    "answer",
    "--server",
    "mac-mini",
    "--project",
    "takt-dash",
    "--goal",
    goal_id.as_str(),
    "--question",
    inquiry_id.as_str(),
    "--text",
    "yes",
  ]);
  assert_eq!(answer_code, EXIT_OK, "表示した問い合わせ ID をそのまま回答に使える必要があります: {answer_stderr}");

  let (merge_code, _merge_stdout, merge_stderr) = run_against(&server, &[
    "merge",
    "--server",
    "mac-mini",
    "--project",
    "takt-dash",
    "--goal",
    goal_id.as_str(),
    "--pr",
    pr_number.as_str(),
    "--until",
    "merge",
  ]);
  assert_eq!(
    merge_code, EXIT_OK,
    "表示したゴール ID と PR 番号をそのままマージ承認に使える必要があります: {merge_stderr}"
  );

  let recorded = server.recorded();
  rt.block_on(server.shutdown());

  assert_eq!(
    recorded,
    vec![
      Recorded::Answer {
        server_id:  "mac-mini".to_owned(),
        project:    "takt-dash".to_owned(),
        goal_id:    Some(goal_id.clone()),
        inquiry_id: inquiry_id.clone(),
        text:       Some("yes".to_owned()),
        report:     None,
      },
      Recorded::Merge {
        server_id: "mac-mini".to_owned(),
        project:   "takt-dash".to_owned(),
        goal_id:   goal_id.clone(),
        pr_number: 12,
        until:     "merge".to_owned(),
      },
    ],
    "表示した値をそのまま書き込み要求へ渡す必要があります"
  );
}

#[test]
fn decisions_requires_server_and_project() {
  let rt = runtime();
  let server =
    rt.block_on(start_dummy_server(DummyConfig { pending: Some(pending_state()), ..DummyConfig::default() }));

  let (code, _stdout, _stderr) = run_against(&server, &["decisions"]);
  let recorded = server.recorded();
  rt.block_on(server.shutdown());

  assert_eq!(code, EXIT_USAGE, "--server と --project の無い decisions は使い方の誤りで終わる必要があります");
  assert!(recorded.is_empty(), "使い方の誤りでは backend を呼んではいけません: {recorded:?}");
}

// ----------------------------------------------------------------------------
// 読み取りと書き込みの期限
// ----------------------------------------------------------------------------

#[test]
fn answer_succeeds_after_connect_timeout_with_the_write_deadline() {
  // 接続期限は5秒のまま。書き込みは別の期限（60秒）で実行するため、6秒遅れても成功する。
  let rt = runtime();
  let mut delays = BTreeMap::new();
  delays.insert("AnswerInquiry", Duration::from_secs(6));
  let server = rt.block_on(start_dummy_server(DummyConfig { delays, ..DummyConfig::default() }));

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
    "--text",
    "はい",
  ]);
  let recorded = server.recorded();
  rt.block_on(server.shutdown());

  assert_eq!(code, EXIT_OK, "接続期限を超えても書き込み期限の内なら成功する必要があります: {stderr}");
  assert_eq!(recorded.len(), 1, "応答が遅れても回答は1回だけ送る必要があります: {recorded:?}");
}

#[test]
fn overview_keeps_the_read_deadline() {
  // 読み取りの期限は5秒のまま。6秒遅れる GetOverview は期限超過で失敗する。
  let rt = runtime();
  let mut delays = BTreeMap::new();
  delays.insert("GetOverview", Duration::from_secs(6));
  let server = rt.block_on(start_dummy_server(DummyConfig { delays, ..DummyConfig::default() }));

  let (code, _stdout, stderr) = run_against(&server, &["overview"]);
  rt.block_on(server.shutdown());

  assert_eq!(code, EXIT_FAILURE, "読み取りは5秒の期限を超えると失敗する必要があります: {stderr}");
}

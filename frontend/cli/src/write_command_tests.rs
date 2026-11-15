//! 書き込みコマンドの契約を、本番と同じ `run_with` と生成クライアントのダミー gRPC
//! サーバーで観測する。
//!
//! 引数解析・要求組み立て・成功と失敗の出力と終了コード・承認待ち一覧の種別表示を、
//! コマンドの入口から stdout / stderr / 終了コードまで一続きに確かめます。

use std::{
  net::{Ipv4Addr, SocketAddr, TcpListener},
  sync::{Arc, Mutex},
};

use tonic::{transport::Server, Code, Request, Response, Status};

use crate::{proto::v1, run_with, EXIT_FAILURE, EXIT_OK, EXIT_USAGE};

// ----------------------------------------------------------------------------
// ダミー gRPC サーバー
// ----------------------------------------------------------------------------

/// 書き込みコマンドが backend へ送った要求の記録。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Recorded {
  GetOverview,
  AnswerInquiry { goal_id: String, inquiry_id: String, text: String, report: i32 },
  ApproveMerge { goal_id: String },
  ListGoalRegistrationProposals { project: String },
  ApproveGoalRegistration { project: String, revision: i64 },
  RejectGoalRegistration { project: String, revision: i64 },
}

/// ダミーサーバーが返す値と、指定した RPC で返す失敗。
struct DummyConfig {
  overview:          v1::DashboardState,
  proposals:         Vec<v1::GoalRegistrationProposal>,
  merge_pr_number:   i64,
  merge_task_name:   String,
  approve_goal_id:   String,
  approve_objective: String,
  reject_notice:     String,
  error:             Option<(&'static str, Status)>,
}

impl Default for DummyConfig {
  fn default() -> Self {
    DummyConfig {
      overview:          v1::DashboardState::default(),
      proposals:         Vec::new(),
      merge_pr_number:   42,
      merge_task_name:   "仕上げタスク".to_owned(),
      approve_goal_id:   "g-approved".to_owned(),
      approve_objective: "承認した目的".to_owned(),
      reject_notice:     "登録を取り下げました。".to_owned(),
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
    tokio::time::timeout(std::time::Duration::from_secs(5), self.server_handle)
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
      .add_service(v1::takt_dash_service_server::TaktDashServiceServer::new(service))
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
    self.record(Recorded::GetOverview);
    Ok(Response::new(self.config.overview.clone()))
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

  async fn answer_inquiry(
    &self,
    request: Request<v1::AnswerInquiryRequest>,
  ) -> Result<Response<v1::AnswerInquiryResponse>, Status> {
    let request = request.into_inner();
    self.record(Recorded::AnswerInquiry {
      goal_id:    request.goal_id.clone(),
      inquiry_id: request.inquiry_id.clone(),
      text:       request.text.clone(),
      report:     request.report,
    });
    if let Some(status) = self.config.error_for("AnswerInquiry") {
      return Err(status);
    }
    Ok(Response::new(v1::AnswerInquiryResponse { goal_id: request.goal_id, inquiry_id: request.inquiry_id }))
  }

  async fn approve_merge(
    &self,
    request: Request<v1::ApproveMergeRequest>,
  ) -> Result<Response<v1::ApproveMergeResponse>, Status> {
    let request = request.into_inner();
    self.record(Recorded::ApproveMerge { goal_id: request.goal_id.clone() });
    if let Some(status) = self.config.error_for("ApproveMerge") {
      return Err(status);
    }
    Ok(Response::new(v1::ApproveMergeResponse {
      goal_id:   request.goal_id,
      pr_number: self.config.merge_pr_number,
      task_name: self.config.merge_task_name.clone(),
    }))
  }

  async fn list_goal_registration_proposals(
    &self,
    request: Request<v1::ListGoalRegistrationProposalsRequest>,
  ) -> Result<Response<v1::ListGoalRegistrationProposalsResponse>, Status> {
    let request = request.into_inner();
    self.record(Recorded::ListGoalRegistrationProposals { project: request.project.clone() });
    if let Some(status) = self.config.error_for("ListGoalRegistrationProposals") {
      return Err(status);
    }
    Ok(Response::new(v1::ListGoalRegistrationProposalsResponse { proposals: self.config.proposals.clone() }))
  }

  async fn approve_goal_registration(
    &self,
    request: Request<v1::ApproveGoalRegistrationRequest>,
  ) -> Result<Response<v1::ApproveGoalRegistrationResponse>, Status> {
    let request = request.into_inner();
    self.record(Recorded::ApproveGoalRegistration { project: request.project.clone(), revision: request.revision });
    if let Some(status) = self.config.error_for("ApproveGoalRegistration") {
      return Err(status);
    }
    Ok(Response::new(v1::ApproveGoalRegistrationResponse {
      revision:  request.revision,
      goal_id:   self.config.approve_goal_id.clone(),
      objective: self.config.approve_objective.clone(),
    }))
  }

  async fn reject_goal_registration(
    &self,
    request: Request<v1::RejectGoalRegistrationRequest>,
  ) -> Result<Response<v1::RejectGoalRegistrationResponse>, Status> {
    let request = request.into_inner();
    self.record(Recorded::RejectGoalRegistration { project: request.project.clone(), revision: request.revision });
    if let Some(status) = self.config.error_for("RejectGoalRegistration") {
      return Err(status);
    }
    Ok(Response::new(v1::RejectGoalRegistrationResponse {
      revision: request.revision,
      notice:   self.config.reject_notice.clone(),
    }))
  }

  async fn watch_state(
    &self,
    _request: Request<v1::WatchStateRequest>,
  ) -> Result<Response<Self::WatchStateStream>, Status> {
    Err(Status::unimplemented("このダミーサーバーは WatchState を提供しません。"))
  }
}

// ----------------------------------------------------------------------------
// 実行の補助
// ----------------------------------------------------------------------------

fn args(items: &[&str]) -> Vec<String> {
  items.iter().map(|item| (*item).to_owned()).collect()
}

fn run_with_addr(args: &[String], addr: SocketAddr) -> (i32, String, String) {
  let mut stdout = Vec::new();
  let mut stderr = Vec::new();
  let code = run_with(args, Ok(format!("{addr}")), &mut stdout, &mut stderr);
  let stdout = String::from_utf8(stdout).expect("標準出力は UTF-8 である必要があります");
  let stderr = String::from_utf8(stderr).expect("標準エラーは UTF-8 である必要があります");
  (code, stdout, stderr)
}

fn run_against_dummy(args: &[String], config: DummyConfig) -> (i32, String, String, Vec<Recorded>) {
  let runtime =
    tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("テスト用の実行環境を作れませんでした");
  let server = runtime.block_on(start_dummy_server(config));
  let (code, stdout, stderr) = run_with_addr(args, server.addr);
  let recorded = server.recorded();
  runtime.block_on(server.shutdown());
  (code, stdout, stderr, recorded)
}

fn reserve_closed_addr() -> SocketAddr {
  let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("空きポートを確保できませんでした");
  let addr = listener.local_addr().expect("待ち受けアドレスを取得できませんでした");
  drop(listener);
  addr
}

fn proposal(project: &str, revision: i64, objective: &str) -> v1::GoalRegistrationProposal {
  v1::GoalRegistrationProposal {
    revision,
    server_id: "mac-mini".to_owned(),
    project: project.to_owned(),
    objective: objective.to_owned(),
    ..Default::default()
  }
}

// ----------------------------------------------------------------------------
// 問い合わせへの回答
// ----------------------------------------------------------------------------

#[test]
fn answer_with_text_calls_answer_inquiry_and_prints_the_result() {
  let (code, stdout, stderr, recorded) = run_against_dummy(
    &args(&["answer", "--goal", "goal-1", "--question", "q-1", "--text", "実装しました"]),
    DummyConfig::default(),
  );

  assert_eq!(code, EXIT_OK, "回答の記録は成功する必要があります。stderr: {stderr}");
  assert!(stderr.is_empty(), "成功時に標準エラーへ出してはいけません: {stderr}");
  assert!(stdout.contains("回答を記録しました。"), "stdout: {stdout}");
  assert!(stdout.contains("ゴール: goal-1"), "stdout: {stdout}");
  assert!(stdout.contains("問い合わせ: q-1"), "stdout: {stdout}");
  assert_eq!(recorded, vec![Recorded::AnswerInquiry {
    goal_id:    "goal-1".to_owned(),
    inquiry_id: "q-1".to_owned(),
    text:       "実装しました".to_owned(),
    report:     0,
  }]);
}

#[test]
fn answer_with_report_sends_the_matching_report_value() {
  // 完了の報告と対応できない報告を、それぞれ別の実行で送る。
  for (flag, expected) in
    [("completed", v1::AnswerReport::Completed as i32), ("unable", v1::AnswerReport::Unable as i32)]
  {
    let (code, stdout, stderr, recorded) = run_against_dummy(
      &args(&["answer", "--goal", "goal-1", "--question", "q-1", "--report", flag]),
      DummyConfig::default(),
    );

    assert_eq!(code, EXIT_OK, "回答の記録は成功する必要があります。stderr: {stderr}");
    assert!(!stdout.is_empty(), "--report だけでも結果を表示します: {stdout}");
    assert_eq!(
      recorded,
      vec![Recorded::AnswerInquiry {
        goal_id:    "goal-1".to_owned(),
        inquiry_id: "q-1".to_owned(),
        text:       String::new(),
        report:     expected,
      }],
      "--report {flag} を対応する AnswerReport へ写す必要があります"
    );
  }
}

#[test]
fn answer_fails_when_the_backend_returns_a_tool_failure() {
  let config = DummyConfig {
    error: Some(("AnswerInquiry", Status::failed_precondition("提案はすでに変更されています。"))),
    ..DummyConfig::default()
  };
  let (code, stdout, stderr, _) =
    run_against_dummy(&args(&["answer", "--goal", "goal-1", "--question", "q-1", "--text", "実装しました"]), config);

  assert_eq!(code, EXIT_FAILURE, "backend が失敗したら異常終了する必要があります");
  assert!(stdout.is_empty(), "失敗時に結果を標準出力へ出してはいけません: {stdout}");
  assert!(stderr.contains("回答の記録に失敗しました"), "操作名を示す必要があります: {stderr}");
  assert!(stderr.contains("提案はすでに変更されています。"), "backend のメッセージを出す必要があります: {stderr}");
}

// ----------------------------------------------------------------------------
// プルリクエストのマージ承認
// ----------------------------------------------------------------------------

#[test]
fn merge_with_yes_calls_approve_merge_and_prints_the_result() {
  let (code, stdout, stderr, recorded) =
    run_against_dummy(&args(&["merge", "--goal", "goal-merge", "--yes"]), DummyConfig::default());

  assert_eq!(code, EXIT_OK, "マージの依頼は成功する必要があります。stderr: {stderr}");
  assert!(stderr.is_empty(), "成功時に標準エラーへ出してはいけません: {stderr}");
  assert!(stdout.contains("マージを依頼しました。"), "stdout: {stdout}");
  assert!(stdout.contains("ゴール: goal-merge"), "stdout: {stdout}");
  assert!(stdout.contains("プルリクエスト: #42"), "stdout: {stdout}");
  assert!(stdout.contains("タスク: 仕上げタスク"), "stdout: {stdout}");
  assert_eq!(recorded, vec![Recorded::ApproveMerge { goal_id: "goal-merge".to_owned() }]);
}

#[test]
fn merge_failure_uses_the_merge_operation_name() {
  let config = DummyConfig {
    error: Some(("ApproveMerge", Status::failed_precondition("マージできません。"))),
    ..Default::default()
  };
  let (code, stdout, stderr, _) = run_against_dummy(&args(&["merge", "--goal", "goal-merge", "--yes"]), config);

  assert_eq!(code, EXIT_FAILURE);
  assert!(stdout.is_empty(), "失敗時に結果を標準出力へ出してはいけません: {stdout}");
  assert!(stderr.contains("マージの依頼に失敗しました"), "操作名を示す必要があります: {stderr}");
  assert!(stderr.contains("マージできません。"), "backend のメッセージを出す必要があります: {stderr}");
}

// ----------------------------------------------------------------------------
// ゴール登録の承認待ち一覧
// ----------------------------------------------------------------------------

#[test]
fn registrations_prints_the_goal_registration_label_and_not_the_inquiry_label() {
  let config = DummyConfig {
    proposals: vec![proposal("repo-a", 3, "目的A"), proposal("repo-b", 5, "目的B")],
    ..DummyConfig::default()
  };
  let (code, stdout, stderr, recorded) = run_against_dummy(&args(&["registrations"]), config);

  assert_eq!(code, EXIT_OK, "承認待ちの取得は成功する必要があります。stderr: {stderr}");
  assert!(stderr.is_empty(), "成功時に標準エラーへ出してはいけません: {stderr}");
  assert!(stdout.contains("ゴール登録の承認待ち: 2件"), "件数を示す必要があります: {stdout}");
  assert!(
    stdout.contains("- [ゴール登録の承認待ち] repo-a（mac-mini）リビジョン 3: 目的A"),
    "種別と対象を示す必要があります: {stdout}"
  );
  assert!(
    stdout.contains("- [ゴール登録の承認待ち] repo-b（mac-mini）リビジョン 5: 目的B"),
    "種別と対象を示す必要があります: {stdout}"
  );
  assert!(
    !stdout.contains("問い合わせ（質問）"),
    "ゴール登録の承認待ちを問い合わせ（質問）と混同してはいけません: {stdout}"
  );
  assert_eq!(recorded, vec![Recorded::ListGoalRegistrationProposals { project: String::new() }]);
}

#[test]
fn registrations_passes_the_project_filter() {
  let config = DummyConfig { proposals: vec![proposal("repo-a", 3, "目的A")], ..Default::default() };
  let (code, stdout, stderr, recorded) = run_against_dummy(&args(&["registrations", "--project", "repo-a"]), config);

  assert_eq!(code, EXIT_OK, "stderr: {stderr}");
  assert!(stdout.contains("repo-a（mac-mini）リビジョン 3: 目的A"), "stdout: {stdout}");
  assert_eq!(recorded, vec![Recorded::ListGoalRegistrationProposals { project: "repo-a".to_owned() }]);
}

#[test]
fn registrations_with_no_proposals_shows_zero_and_none() {
  let (code, stdout, stderr, _) = run_against_dummy(&args(&["registrations"]), DummyConfig::default());

  assert_eq!(code, EXIT_OK, "stderr: {stderr}");
  assert!(stdout.contains("ゴール登録の承認待ち: 0件"), "stdout: {stdout}");
  assert!(stdout.contains("（なし）"), "空の一覧は空であることが分かる表示にする必要があります: {stdout}");
}

// ----------------------------------------------------------------------------
// ゴール登録の承認・却下
// ----------------------------------------------------------------------------

#[test]
fn approve_registration_with_yes_calls_the_tool_and_prints_the_goal() {
  let (code, stdout, stderr, recorded) = run_against_dummy(
    &args(&["approve-registration", "--project", "repo-a", "--revision", "3", "--yes"]),
    DummyConfig::default(),
  );

  assert_eq!(code, EXIT_OK, "承認は成功する必要があります。stderr: {stderr}");
  assert!(stderr.is_empty(), "成功時に標準エラーへ出してはいけません: {stderr}");
  assert!(stdout.contains("ゴール登録を承認しました。"), "stdout: {stdout}");
  assert!(stdout.contains("リビジョン: 3"), "stdout: {stdout}");
  assert!(stdout.contains("ゴール: g-approved「承認した目的」"), "stdout: {stdout}");
  assert_eq!(recorded, vec![Recorded::ApproveGoalRegistration { project: "repo-a".to_owned(), revision: 3 }]);
}

#[test]
fn reject_registration_with_yes_calls_the_tool_and_prints_the_notice() {
  let (code, stdout, stderr, recorded) = run_against_dummy(
    &args(&["reject-registration", "--project", "repo-a", "--revision", "5", "--yes"]),
    DummyConfig::default(),
  );

  assert_eq!(code, EXIT_OK, "却下は成功する必要があります。stderr: {stderr}");
  assert!(stderr.is_empty(), "成功時に標準エラーへ出してはいけません: {stderr}");
  assert!(stdout.contains("ゴール登録を却下しました。"), "stdout: {stdout}");
  assert!(stdout.contains("リビジョン: 5"), "stdout: {stdout}");
  assert!(stdout.contains("登録を取り下げました。"), "stdout: {stdout}");
  assert_eq!(recorded, vec![Recorded::RejectGoalRegistration { project: "repo-a".to_owned(), revision: 5 }]);
}

// ----------------------------------------------------------------------------
// 破壊的操作の確認と引数の誤り
// ----------------------------------------------------------------------------

#[test]
fn destructive_commands_require_yes_and_do_not_call_the_backend() {
  let cases: [(&str, &[&str]); 3] = [
    ("merge", &["merge", "--goal", "goal-merge"]),
    ("approve-registration", &["approve-registration", "--project", "repo-a", "--revision", "3"]),
    ("reject-registration", &["reject-registration", "--project", "repo-a", "--revision", "3"]),
  ];

  for (command, items) in cases {
    let (code, stdout, stderr, recorded) = run_against_dummy(&args(items), DummyConfig::default());

    assert_eq!(code, EXIT_USAGE, "{command} は --yes 無しで実行してはいけません");
    assert!(stdout.is_empty(), "使い方の誤りで標準出力へ出してはいけません: {stdout}");
    assert!(
      stderr.contains(&format!("{command} は破壊的な操作です。実行するには --yes を付けてください。")),
      "確認を促す文言を出す必要があります: {stderr}"
    );
    assert!(recorded.is_empty(), "{command} は --yes 無しで backend を呼んではいけません: {recorded:?}");
  }
}

#[test]
fn invalid_arguments_exit_with_usage_and_do_not_call_the_backend() {
  let cases: &[&[&str]] = &[
    &["answer", "--goal", "goal-1"],
    &["answer", "--goal", "goal-1", "--question", "q-1"],
    &["answer", "--goal", "goal-1", "--question", "q-1", "--text", ""],
    &["answer", "--goal", "goal-1", "--question", "q-1", "--text", "本文", "--report", "maybe"],
    &["answer", "--question", "q-1", "--text", "本文"],
    &["merge", "--yes"],
    &["merge", "--goal", "goal-1"],
    &["approve-registration", "--revision", "3", "--yes"],
    &["approve-registration", "--project", "repo-a", "--revision", "0", "--yes"],
    &["approve-registration", "--project", "repo-a", "--revision", "abc", "--yes"],
    &["reject-registration", "--project", "repo-a", "--revision", "-1", "--yes"],
    &["registrations", "--yes"],
    &["registrations", "--revision", "2"],
    &["unknown-command"],
  ];

  for items in cases {
    let (code, stdout, stderr, recorded) = run_against_dummy(&args(items), DummyConfig::default());

    assert_eq!(code, EXIT_USAGE, "引数の誤りは使い方の誤りとして終わる必要があります: {items:?} / {stderr}");
    assert!(stdout.is_empty(), "使い方の誤りで標準出力へ出してはいけません: {items:?} / {stdout}");
    assert!(!stderr.is_empty(), "使い方を標準エラーへ出す必要があります: {items:?}");
    assert!(recorded.is_empty(), "引数の誤りで backend を呼んではいけません: {items:?} / {recorded:?}");
  }
}

#[test]
fn overview_shows_the_ids_needed_for_answering_and_merging() {
  // 回答はゴール ID と問い合わせ ID、マージ依頼はゴール ID を要求するため、概要の判断待ち行から
  // それぞれの ID を取得できる必要があります。ゴールに属さない問い合わせや、ゴール ID が空の
  // 問い合わせではゴール ID を表示せず、問い合わせ ID だけを示します。
  let state = v1::DashboardState {
    pending_decisions: vec![
      v1::PendingDecision {
        r#type: v1::PendingDecisionType::Inquiry as i32,
        server_id: "mac-mini".to_owned(),
        project: "takt-dash".to_owned(),
        goal_id: Some("goal-1".to_owned()),
        inquiry_kind: v1::InquiryKind::Question as i32,
        body: "どちらにしますか。".to_owned(),
        inquiry_id: Some("q-open".to_owned()),
        ..Default::default()
      },
      v1::PendingDecision {
        r#type: v1::PendingDecisionType::AwaitingMerge as i32,
        server_id: "mac-mini".to_owned(),
        project: "takt-dash".to_owned(),
        goal_id: Some("goal-merge".to_owned()),
        inquiry_kind: v1::InquiryKind::Unspecified as i32,
        body: "マージしてください。".to_owned(),
        inquiry_id: None,
        ..Default::default()
      },
      v1::PendingDecision {
        r#type: v1::PendingDecisionType::Inquiry as i32,
        server_id: "mac-mini".to_owned(),
        project: "takt-dash".to_owned(),
        goal_id: None,
        inquiry_kind: v1::InquiryKind::Question as i32,
        body: "ゴールに属さない問い合わせ。".to_owned(),
        inquiry_id: Some("q-free".to_owned()),
        ..Default::default()
      },
      v1::PendingDecision {
        r#type: v1::PendingDecisionType::Inquiry as i32,
        server_id: "mac-mini".to_owned(),
        project: "takt-dash".to_owned(),
        goal_id: Some(String::new()),
        inquiry_kind: v1::InquiryKind::Question as i32,
        body: "ゴール ID が空の問い合わせ。".to_owned(),
        inquiry_id: Some("q-empty".to_owned()),
        ..Default::default()
      },
    ],
    ..Default::default()
  };

  let config = DummyConfig { overview: state, ..DummyConfig::default() };
  let (code, stdout, stderr, recorded) = run_against_dummy(&[], config);

  assert_eq!(code, EXIT_OK, "概要の取得は成功する必要があります。stderr: {stderr}");
  assert!(stderr.is_empty(), "成功時に標準エラーへ出してはいけません: {stderr}");
  assert!(stdout.contains("ゴールID: goal-1"), "回答に使うゴール ID を示す必要があります: {stdout}");
  assert!(stdout.contains("ゴールID: goal-merge"), "マージに使うゴール ID を示す必要があります: {stdout}");
  assert_eq!(
    stdout.matches("ゴールID:").count(),
    2,
    "ゴールに属さない問い合わせやゴール ID が空の問い合わせにはゴール ID を出してはいけません: {stdout}"
  );
  assert!(stdout.contains("問い合わせID: q-open"), "回答に使う問い合わせ ID を示す必要があります: {stdout}");
  assert!(stdout.contains("問い合わせID: q-free"), "ゴール非所属でも問い合わせ ID を示す必要があります: {stdout}");
  assert!(stdout.contains("問い合わせID: q-empty"), "ゴール ID が空でも問い合わせ ID を示す必要があります: {stdout}");
  assert_eq!(
    stdout.matches("問い合わせID:").count(),
    3,
    "問い合わせの行だけに問い合わせ ID を出す必要があります: {stdout}"
  );
  assert!(stdout.contains("人のマージ待ち"), "既存のマージ待ち表示を維持する必要があります: {stdout}");
  assert_eq!(recorded, vec![Recorded::GetOverview]);
}

#[test]
fn registration_commands_use_their_operation_names_on_failure() {
  // 道具の失敗（FailedPrecondition）は接続不能として扱わず、操作名つきの失敗にする。
  let cases: [(&str, &[&str], &str); 3] = [
    ("ListGoalRegistrationProposals", &["registrations"], "ゴール登録の承認待ちの取得に失敗しました"),
    (
      "ApproveGoalRegistration",
      &["approve-registration", "--project", "repo-a", "--revision", "3", "--yes"],
      "ゴール登録の承認に失敗しました",
    ),
    (
      "RejectGoalRegistration",
      &["reject-registration", "--project", "repo-a", "--revision", "3", "--yes"],
      "ゴール登録の却下に失敗しました",
    ),
  ];

  for (method, items, expected) in cases {
    let config = DummyConfig {
      error: Some((method, Status::failed_precondition("提案はすでに変更されています。"))),
      ..DummyConfig::default()
    };
    let (code, stdout, stderr, _) = run_against_dummy(&args(items), config);

    assert_eq!(code, EXIT_FAILURE, "{expected}: 異常終了する必要があります");
    assert!(stdout.is_empty(), "失敗時に結果を標準出力へ出してはいけません: {stdout}");
    assert!(stderr.contains(expected), "操作名を示す必要があります: {stderr}");
    assert!(!stderr.contains("backend に接続できません。"), "道具の失敗を接続不能と混同してはいけません: {stderr}");
  }
}

// ----------------------------------------------------------------------------
// 接続不能
// ----------------------------------------------------------------------------

#[test]
fn write_command_fails_with_the_existing_connection_message_when_backend_is_down() {
  let addr = reserve_closed_addr();
  let (code, stdout, stderr) =
    run_with_addr(&args(&["answer", "--goal", "goal-1", "--question", "q-1", "--text", "実装しました"]), addr);

  assert_eq!(code, EXIT_FAILURE, "接続できないときは取得の失敗として終わる必要があります");
  assert!(stderr.starts_with("backend に接続できません。"), "既存の文言で始まる必要があります: {stderr}");
  assert!(stdout.is_empty(), "接続できないときは結果を標準出力へ出してはいけません: {stdout}");
}

#[test]
fn usage_error_is_reported_before_connecting() {
  let addr = reserve_closed_addr();
  let (code, stdout, stderr) = run_with_addr(&args(&["merge", "--goal", "goal-1"]), addr);

  assert_eq!(code, EXIT_USAGE, "確認フラグの不足は接続前に判定する必要があります");
  assert!(stderr.contains("--yes"), "確認フラグを示す必要があります: {stderr}");
  assert!(!stderr.contains("backend に接続できません。"), "接続の失敗と混同してはいけません: {stderr}");
  assert!(stdout.is_empty());
}

#[test]
fn connection_failure_classification_still_covers_unavailable_and_unknown() {
  // 既存の接続不能の分類（4コード）を書き込みコマンドでも共有する。
  assert!(crate::is_connection_failure(Code::Unavailable));
  assert!(crate::is_connection_failure(Code::Unknown));
  assert!(!crate::is_connection_failure(Code::FailedPrecondition));
}

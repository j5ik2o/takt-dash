//! proto/taktdash.proto の TaktDashService（読み取り系と書き込み系の API）の実装。
//!
//! 状態ストアの投影と activity のクエリから、proto のメッセージ型どおりの応答を組み立てます。
//! unary の4つ（GetOverview・GetProjects・GetPendingDecisions・GetActivity）はストアから読みます。
//! GetRunDetail は状態ストアと run の保存場所から組み立て、ローカルで保存場所を読めた run では
//! takt-mcp を起動しません。保存場所の対応表に無い run と対応表を読めない run（SSH 越しに扱う
//! リモートのサーバーのプロジェクトなど）だけ、要求時に takt-mcp の takt_get_run を呼びます。
//! GetRunContent はレポートか指示書の本文を1件だけ読みます。本文は、GetRunDetail と同じ
//! 条件の run だけ takt_get_run から読みます。GetConversation は保存領域の manager
//! の会話を読みます。GetGoals・ GetGoalDetail・GetTaskDetail はゴールの一覧と詳細を
//! 返し、GetGoalDetail と GetTaskDetail は要求時に takt-mcp を呼びます。WatchState は接続時に
//! 全件を返し、定期読み取りの変化通知を受けるたびに状態を組み立て直して送ります。
//! GetRunUpdates は本体ログの追記分を返し、要求の検証と応答の組み立ては [`run_updates`]
//! に置きます。

pub mod assembly;
mod detached_read;
mod lanes;
pub mod mapping;
mod resolve;
mod run_content;
mod run_detail;
mod run_updates;
mod write_ledger;
mod writes;

use std::{
  path::PathBuf,
  sync::{Arc, Mutex},
  time::Instant,
};

use assembly::{activity_event, assemble_goals, assemble_state_for, conversation_response};
use chrono::{DateTime, Utc};
use mapping::{
  execution_status, goal_question, goal_status, goal_task, goal_work_log, optional_timestamp, run_status, task_status,
  timestamp,
};
use resolve::{resolve_project_in, ResolvedProject};
use run_updates::{parse_log_cursor, read_updates_response};
use tokio::sync::{broadcast, mpsc, watch};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use write_ledger::{WriteFailure, WriteLedger};

use crate::{
  activity::ActivityStore,
  config::load_config,
  conversation::{ConversationQuery, ConversationReader},
  domain::{GoalId, IdempotencyKey, InstructionId, ProjectName, ReportFilename, RunSlug, ServerId, TaskName},
  mcp::{McpClientRegistry, ToolError, TransportError},
  model::{self},
  run_log::{ContentRead, InitialRead, Instruction},
  run_progress::{RunLogHandle, RunLogHandles, RunReadFailure},
  store::StateStore,
  takt_dash::v1::{
    self, get_run_content_request, AnswerInquiryRequest, AnswerInquiryResponse, ApproveGoalMergeRequest,
    ApproveGoalMergeResponse, ApproveGoalRegistrationRequest, ApproveGoalRegistrationResponse, DashboardState,
    GetActivityRequest, GetActivityResponse, GetConversationRequest, GetConversationResponse, GetGoalDetailRequest,
    GetGoalDetailResponse, GetGoalsRequest, GetGoalsResponse, GetOverviewRequest, GetPendingDecisionsRequest,
    GetPendingDecisionsResponse, GetProjectsRequest, GetProjectsResponse, GetRunContentRequest, GetRunContentResponse,
    GetRunDetailRequest, GetRunDetailResponse, GetRunUpdatesRequest, GetRunUpdatesResponse, GetTaskDetailRequest,
    GetTaskDetailResponse, GoalRegistrationProposal, ListGoalRegistrationProposalsRequest,
    ListGoalRegistrationProposalsResponse, OperationAnswerReport, RejectGoalRegistrationRequest,
    RejectGoalRegistrationResponse, Run, RunInstruction, WatchStateRequest,
  },
};

/// GetProjects が返す直近の出来事の件数。proto に指定がないため設計で決めます。
const RECENT_EVENTS_LIMIT: usize = 20;
/// GetActivity が返す出来事の一覧の件数。proto に指定がないため設計で決めます。
const ACTIVITY_EVENTS_LIMIT: usize = 100;
/// WatchState の送信元タスクとストリームの間のバッファの大きさ。
const WATCH_CHANNEL_CAPACITY: usize = 16;
/// 状態が変わらなくても WatchState の状態を再配信するまでの間隔。
///
/// 開いている画面が自動更新を確認できるよう、読み取り間隔（既定30秒）より短い15秒で再送します。
const WATCH_RESEND_INTERVAL: std::time::Duration = std::time::Duration::from_secs(15);
/// 画面の要求時の読み取りで常駐 HTTP サーバーへ送る 1 呼び出しの期限。
const READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
/// GetRunDetail の総上限（issue #59・#180）。保存場所から読めない run の回り道は takt_get_run を
/// 3 回まで順に呼ぶ（run 本体、view instructions、view task）ので、1 回の期限の 3 倍にします。
/// Web の deadline（`frontend/web/src/grpc/client.ts` の `RUN_DETAIL_DEADLINE_MS`、50 秒）は
/// これより長くし、backend の DEADLINE_EXCEEDED が Web の打ち切りより先に届くようにします。
pub const RUN_DETAIL_BUDGET: std::time::Duration = std::time::Duration::from_secs(45);
const _: () = assert!(3 * READ_TIMEOUT.as_secs() <= RUN_DETAIL_BUDGET.as_secs());
/// run 詳細が、読み位置を持たない本体ログを初めて読むときに読む末尾の大きさ（1 MiB）。
///
/// 実行中の run の詳細を2秒以内に返すため、大きな本体ログの全体を読みません。代わりに、この大きさを
/// 超える本体ログの run
/// を初めて開いたときは、ステップの記録が末尾の範囲に含まれる分だけになります。
const DETAIL_INITIAL_TAIL_BYTES: u64 = 1024 * 1024;
/// GetRunContent が返す本文の上限（1 MiB）。gRPC の既定の受信上限（4MB）を超える応答を作りません。
const RUN_CONTENT_MAX_BYTES: u64 = 1024 * 1024;

/// unary ハンドラの処理時間と、その間に接続ロックを待った時間を記録する。
///
/// `Drop` で記録するため、成功・失敗・早期 return のどの終端でも1行が残ります。
/// `lock_wait_us` はハンドラ開始時と終了時の累積値の差で、SQLite のクエリ実行時間は含みません。
struct RpcTimingGuard {
  rpc:                   &'static str,
  activity:              Arc<ActivityStore>,
  started_at:            Instant,
  lock_wait_us_at_start: u64,
}

impl RpcTimingGuard {
  fn start(rpc: &'static str, activity: &Arc<ActivityStore>) -> Self {
    RpcTimingGuard {
      rpc,
      activity: activity.clone(),
      started_at: Instant::now(),
      lock_wait_us_at_start: activity.lock_wait_total_us(),
    }
  }
}

impl Drop for RpcTimingGuard {
  fn drop(&mut self) {
    let elapsed_us = self.started_at.elapsed().as_micros() as u64;
    let lock_wait_us = self.activity.lock_wait_total_us().saturating_sub(self.lock_wait_us_at_start);
    tracing::info!(rpc = %self.rpc, elapsed_us, lock_wait_us, "unary RPC を処理しました。");
  }
}

/// 読み取り系 API のサービス。
pub struct TaktDashServiceImpl {
  store:             Arc<StateStore>,
  activity:          Arc<ActivityStore>,
  config_path:       PathBuf,
  conversation:      Arc<ConversationReader>,
  changes:           broadcast::Sender<()>,
  shutdown:          watch::Receiver<bool>,
  registry:          McpClientRegistry,
  clock:             Clock,
  /// プロジェクトごとの run の集計と、その集計を作ったときの cwd。GetRunDetail が要求をまたいで
  /// 持ち、同じ run の本体ログを前回の続きから読むために使います。読み位置を持たない本体ログは
  /// 末尾の [`DETAIL_INITIAL_TAIL_BYTES`] だけを読みます。lock は取っ手の受け渡しの間だけ持ち、
  /// 読み取りの間は持ちません。
  run_logs:          Mutex<RunLogHandles<ProjectName>>,
  /// 書き込み要求の鍵ごとの記録。同じ操作の再試行と同時の重複を、takt へ送り直さずに扱います。
  write_ledger:      WriteLedger,
  /// GetRunDetail の総上限。既定は [`RUN_DETAIL_BUDGET`]。テストは短い値を入れます。
  run_detail_budget: std::time::Duration,
}

/// サービスの現在時刻を返す関数。テストは固定時計を注入します。
pub type Clock = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;

/// サービスが読み取りと書き込みに使う依存のまとまり。
///
/// 引数の数が増えすぎないよう、サービスを作る材料を一つにまとめます。各項目の意味は
/// [`TaktDashServiceImpl::with_clock`] の説明を参照してください。
pub struct ServiceDeps {
  pub store:        Arc<StateStore>,
  pub activity:     Arc<ActivityStore>,
  pub config_path:  PathBuf,
  pub conversation: Arc<ConversationReader>,
  pub changes:      broadcast::Sender<()>,
  pub shutdown:     watch::Receiver<bool>,
  pub registry:     McpClientRegistry,
}

impl TaktDashServiceImpl {
  /// 状態ストアと activity と設定のパスと、共有する購読・停止のチャネルからサービスを作る。
  ///
  /// `changes` は状態変化の通知の送信元で、WatchState がこれを購読します。poller の
  /// read_cycle_once も同じ送信元へ通知するため、bin とテストは poller とサービスへ同じ
  /// broadcast の送信元を渡します。`shutdown` はサーバーの停止の合図で、WatchState の送信
  /// タスクはこの合図が立つと終了します。終了しないと tonic のグレースフルシャットダウンが
  /// 開いたままのストリームを待って止まれません。生成責務を呼び出し元へ寄せ、両方を必須引数
  /// にして、シャットダウン配線の省略がコンパイルエラーになるようにします。
  ///
  /// `registry` は Poller と共有する takt-mcp のクライアントのレジストリです。書き込み RPC は
  /// 同じレジストリから同じプロジェクトの子プロセスを取得します。
  ///
  /// GetRunDetail と GetRunContent は要求のたびに設定のパスからプロジェクトの cwd を解決し、
  /// run の保存場所だけを読みます。takt-mcp の子プロセスは起動しません。
  pub fn new(deps: ServiceDeps) -> Self {
    Self::with_clock(deps, Arc::new(Utc::now))
  }

  /// 現在時刻を返す関数を注入してサービスを作る。既定は実時計です。
  pub fn with_clock(deps: ServiceDeps, clock: Clock) -> Self {
    let ServiceDeps { store, activity, config_path, conversation, changes, shutdown, registry } = deps;
    TaktDashServiceImpl {
      store,
      activity,
      config_path,
      conversation,
      changes,
      shutdown,
      registry,
      clock,
      run_logs: Mutex::new(RunLogHandles::with_initial_read(InitialRead::Tail(DETAIL_INITIAL_TAIL_BYTES))),
      write_ledger: WriteLedger::default(),
      run_detail_budget: RUN_DETAIL_BUDGET,
    }
  }

  /// GetRunDetail の総上限を差し替える。テストが短い上限で打ち切りを確かめるために使います。
  pub fn with_run_detail_budget(mut self, budget: std::time::Duration) -> Self {
    self.run_detail_budget = budget;
    self
  }

  /// プロジェクトの run の集計の取っ手を返す。cwd が変わったときの作り直しは [`RunLogHandles`]
  /// が決めます。
  fn run_log_handle(&self, project: &ProjectName, cwd: &std::path::Path) -> Result<RunLogHandle, Status> {
    let mut run_logs =
      self.run_logs.lock().map_err(|error| Status::internal(format!("run の集計を取得できませんでした: {error}")))?;
    Ok(run_logs.handle_for(project, cwd))
  }

  /// サービスが今回の要求で使う現在時刻を返す。
  fn now(&self) -> DateTime<Utc> {
    (self.clock)()
  }
}

/// 同期の SQLite 処理を `spawn_blocking` で実行し、待ち合わせの失敗を gRPC のステータスへ変換する。
async fn blocking<T, F>(task: F) -> Result<T, Status>
where
  F: FnOnce() -> T + Send + 'static,
  T: Send + 'static, {
  tokio::task::spawn_blocking(task)
    .await
    .map_err(|error| Status::internal(format!("ブロッキング処理を完了できませんでした: {error}")))
}

fn assemble_runs(views: &[model::ProjectView], store: &StateStore, now: DateTime<Utc>) -> Vec<Run> {
  let mut runs = Vec::new();
  for view in views {
    for run in &view.runs {
      runs.push(Run {
        run_slug:                run.run_slug.clone(),
        session_id:              run.session_id.clone(),
        server_id:               view.server_id.clone(),
        project:                 view.name.clone(),
        goal_id:                 run.goal_id.as_ref().map(|goal_id| goal_id.as_str().to_owned()).unwrap_or_default(),
        task_name:               run.task_name.as_str().to_owned(),
        summary:                 run.summary.clone(),
        started_at:              optional_timestamp(run.started_at),
        steps:                   run
          .steps
          .iter()
          .map(|step| v1::RunStep {
            step_name:  step.step_name.clone(),
            model:      step.model.clone(),
            started_at: Some(timestamp(step.started_at)),
            ended_at:   optional_timestamp(step.ended_at),
          })
          .collect(),
        current_step_started_at: optional_timestamp(run.current_step_started_at),
        current_step_name:       run.current_step.clone(),
        last_log_at:             optional_timestamp(run.last_log_at),
        reports:                 run
          .reports
          .iter()
          .map(|report| v1::ReportSummary { filename: report.filename.clone(), size_bytes: report.size_bytes })
          .collect(),
        status:                  run_status(run.status) as i32,
        log_silent:              store.is_log_silent(run, now),
        current_step_number:     run.current_step_number,
        total_steps:             run.total_steps,
        current_iteration:       run.current_iteration,
      });
    }
  }
  runs
}

/// takt-mcp の道具エラーを gRPC のステータスへ変換する境界。
fn tool_error_status(error: ToolError) -> Status {
  match error {
    | ToolError::Parse(message) => Status::internal(format!("takt-mcp の応答を解析できませんでした: {message}")),
    | ToolError::Tool(error) => Status::unavailable(format!("takt-mcp の道具が失敗しました: {error}")),
    | ToolError::Transport(error) => Status::unavailable(format!("takt-mcp へ接続できませんでした: {error}")),
  }
}

/// 書き込み要求の鍵を検証する境界。鍵の無い要求は `None` のまま通します。
fn idempotency_key(key: &Option<String>) -> Result<Option<IdempotencyKey>, Status> {
  key.as_deref().map(IdempotencyKey::try_from).transpose().map_err(|error| {
    Status::invalid_argument(format!("idempotency_key が不正です（1 から 128 文字の ASCII 表示文字）: {error}"))
  })
}

/// 書き込み道具のエラーを、書き込みの失敗へ変換する境界。
///
/// 接続の確立は道具を送る前に試みていますが、確立済みのセッションを使い回す HTTP 接続先では、
/// 道具を送る段で初めて接続の失敗が分かることがあります。接続を張れなかった失敗と、消えた
/// セッションを作り直せなかった失敗（[`TransportError::NotSent`]）は、道具が takt へ届いて
/// いないので、書き込みは適用されていません。
/// takt が拒否を返した失敗（道具のエラー、JSON-RPC のエラー、HTTP のエラーステータス）も、
/// 書き込みが適用されていないと分かります。応答を受け取れなかった失敗と応答を読めなかった失敗は、
/// 適用されたかどうか分かりません。接続の失敗と区別して DEADLINE_EXCEEDED で返し、結果が不明で
/// あることを利用者へ伝えます。
fn write_failure(error: ToolError) -> WriteFailure {
  const CHECK: &str = "状態を確かめてから、必要ならやり直してください。";
  match error {
    | ToolError::Transport(TransportError::Timeout(seconds)) => WriteFailure::Unknown(Status::deadline_exceeded(
      format!("takt-mcp から {seconds} 秒以内に応答がなく、操作の結果は不明です。{CHECK}"),
    )),
    | ToolError::Transport(
      error @ (TransportError::Io(_)
      | TransportError::ProcessExited
      | TransportError::InvalidJson(_)
      | TransportError::Protocol(_)),
    ) => WriteFailure::Unknown(Status::deadline_exceeded(format!(
      "takt-mcp の応答を受け取れず、操作の結果は不明です（{error}）。{CHECK}"
    ))),
    | ToolError::Parse(message) => WriteFailure::Unknown(Status::deadline_exceeded(format!(
      "takt-mcp の応答を解析できず、操作の結果は不明です（{message}）。{CHECK}"
    ))),
    | error @ (ToolError::Tool(_)
    | ToolError::Transport(
      TransportError::Rpc { .. }
      | TransportError::Http(_)
      | TransportError::Spawn(_)
      | TransportError::Backoff(_)
      | TransportError::NotSent(_),
    )) => WriteFailure::Failed(tool_error_status(error)),
  }
}

/// run の保存場所の読み取り失敗を gRPC のステータスへ変換する。
fn run_read_failure_status(failure: RunReadFailure) -> Status {
  match failure {
    | RunReadFailure::NotListed | RunReadFailure::Missing => Status::not_found(failure.to_string()),
    | RunReadFailure::Unreadable(_) | RunReadFailure::Rejected(_) => Status::unavailable(failure.to_string()),
    | RunReadFailure::InvalidMeta(_) => Status::internal(failure.to_string()),
  }
}

/// 要求の runSlug を検証する。空と、run のディレクトリ名にならない値は入力の誤りです。
fn parse_run_slug(run_slug: String) -> Result<RunSlug, Status> {
  if run_slug.trim().is_empty() {
    return Err(Status::invalid_argument("runSlug が空です。"));
  }
  RunSlug::try_from(run_slug.as_str()).map_err(|error| Status::invalid_argument(format!("runSlug が不正です: {error}")))
}

/// 指示書を一覧の項目にする。本文は GetRunContent で返します。
fn run_instruction(instruction: &Instruction) -> RunInstruction {
  RunInstruction {
    instruction_id: instruction.instruction_id.get(),
    issued_at:      Some(timestamp(instruction.issued_at)),
    state:          instruction.state.as_str().to_owned(),
  }
}

/// GetRunContent が本文を返す対象。
enum ContentTarget {
  Report(ReportFilename),
  Instruction(InstructionId),
}

#[tonic::async_trait]
impl v1::takt_dash_service_server::TaktDashService for TaktDashServiceImpl {
  type WatchStateStream = ReceiverStream<Result<DashboardState, Status>>;

  async fn get_overview(&self, _request: Request<GetOverviewRequest>) -> Result<Response<DashboardState>, Status> {
    let _timing = RpcTimingGuard::start("GetOverview", &self.activity);
    let store = self.store.clone();
    let activity = self.activity.clone();
    let now = self.now();
    let state = blocking(move || assemble_state_for(&store, &activity, now, assemble_runs)).await?;
    Ok(Response::new(state))
  }

  async fn get_projects(&self, _request: Request<GetProjectsRequest>) -> Result<Response<GetProjectsResponse>, Status> {
    let _timing = RpcTimingGuard::start("GetProjects", &self.activity);
    let activity = self.activity.clone();
    let response = blocking(move || -> Result<GetProjectsResponse, Status> {
      let recent_events = activity
        .recent_events(RECENT_EVENTS_LIMIT)
        .map_err(|error| Status::internal(format!("出来事の一覧を取得できませんでした: {error}")))?
        .into_iter()
        .map(activity_event)
        .collect();
      Ok(GetProjectsResponse { state: None, recent_events })
    })
    .await??;
    Ok(Response::new(response))
  }

  async fn get_pending_decisions(
    &self,
    _request: Request<GetPendingDecisionsRequest>,
  ) -> Result<Response<GetPendingDecisionsResponse>, Status> {
    let _timing = RpcTimingGuard::start("GetPendingDecisions", &self.activity);
    let store = self.store.clone();
    let activity = self.activity.clone();
    let now = self.now();
    let state = blocking(move || assemble_state_for(&store, &activity, now, assemble_runs)).await?;
    Ok(Response::new(GetPendingDecisionsResponse { state: Some(state) }))
  }

  async fn get_activity(&self, _request: Request<GetActivityRequest>) -> Result<Response<GetActivityResponse>, Status> {
    let _timing = RpcTimingGuard::start("GetActivity", &self.activity);
    let activity = self.activity.clone();
    let now = self.now();
    let response = blocking(move || -> Result<GetActivityResponse, Status> {
      let events = activity
        .recent_events(ACTIVITY_EVENTS_LIMIT)
        .map_err(|error| Status::internal(format!("出来事の一覧を取得できませんでした: {error}")))?
        .into_iter()
        .map(activity_event)
        .collect();
      let daily_summaries = activity
        .daily_summaries(now)
        .map_err(|error| Status::internal(format!("日次集計を取得できませんでした: {error}")))?
        .into_iter()
        .map(|summary| v1::DailySummary {
          date:      summary.date.as_str().to_owned(),
          total:     summary.total,
          attention: summary.attention,
        })
        .collect();
      let today_counts_by_server = activity
        .today_counts_by_server(now)
        .map_err(|error| Status::internal(format!("サーバーごとの件数を取得できませんでした: {error}")))?
        .into_iter()
        .map(|count| v1::ServerEventCount { server_id: count.server_id.as_str().to_owned(), count: count.count })
        .collect();
      Ok(GetActivityResponse { state: None, events, daily_summaries, today_counts_by_server })
    })
    .await??;
    Ok(Response::new(response))
  }

  async fn get_run_detail(
    &self,
    request: Request<GetRunDetailRequest>,
  ) -> Result<Response<GetRunDetailResponse>, Status> {
    let _timing = RpcTimingGuard::start("GetRunDetail", &self.activity);
    let run_slug = parse_run_slug(request.into_inner().run_slug)?;
    run_detail::within_budget(self.run_detail_budget, &run_slug, self.read_run_detail(run_slug.clone()))
      .await
      .map(Response::new)
  }

  async fn get_run_updates(
    &self,
    request: Request<GetRunUpdatesRequest>,
  ) -> Result<Response<GetRunUpdatesResponse>, Status> {
    let _timing = RpcTimingGuard::start("GetRunUpdates", &self.activity);
    let request = request.into_inner();
    let run_slug = parse_run_slug(request.run_slug)?;
    let after = parse_log_cursor(request.epoch, request.after_sequence)?;
    let target = self.resolve_run_target(&run_slug)?;
    // GetRunDetail と同じ取っ手を使い、本体ログを前回の読み位置の続きから読む。
    let run_logs = self.run_log_handle(&target.resolved.project, &target.resolved.cwd)?;
    let mut source = self.read_source(&target.resolved);
    let response = read_updates_response(&run_logs, &mut source, &target.resolved.cwd, run_slug, after).await?;
    Ok(Response::new(response))
  }

  async fn get_run_content(
    &self,
    request: Request<GetRunContentRequest>,
  ) -> Result<Response<GetRunContentResponse>, Status> {
    let _timing = RpcTimingGuard::start("GetRunContent", &self.activity);
    let request = request.into_inner();
    let run_slug = parse_run_slug(request.run_slug)?;
    let target = match request.target {
      | Some(get_run_content_request::Target::ReportFilename(filename)) => ContentTarget::Report(
        ReportFilename::try_from(filename.as_str())
          .map_err(|error| Status::invalid_argument(format!("レポートの名前が不正です: {error}")))?,
      ),
      | Some(get_run_content_request::Target::InstructionId(instruction_id)) => ContentTarget::Instruction(
        InstructionId::try_from(instruction_id)
          .map_err(|error| Status::invalid_argument(format!("指示書の ID が不正です: {error}")))?,
      ),
      | None => return Err(Status::invalid_argument("レポートの名前か指示書の ID のどちらかを指定してください。")),
    };
    let resolved = self.resolve_run_target(&run_slug)?.resolved;
    let mut source = self.read_source(&resolved);
    let (read, missing) = match target {
      | ContentTarget::Report(filename) => (
        run_content::read_report_body(&mut source, &resolved, &run_slug, &filename, RUN_CONTENT_MAX_BYTES).await,
        format!("run {} にレポート {} はありません。", run_slug.as_str(), filename.as_str()),
      ),
      | ContentTarget::Instruction(instruction_id) => (
        run_content::read_instruction_body(&mut source, &resolved, &run_slug, instruction_id, RUN_CONTENT_MAX_BYTES)
          .await,
        format!("run {} に指示書 #{} はありません。", run_slug.as_str(), instruction_id.get()),
      ),
    };
    let content = match read.map_err(run_read_failure_status)? {
      | ContentRead::Found(content) => content,
      | ContentRead::Missing => return Err(Status::not_found(missing)),
      | ContentRead::Unreadable(reason) => return Err(Status::unavailable(reason)),
    };
    Ok(Response::new(GetRunContentResponse {
      run_slug:   run_slug.as_str().to_owned(),
      content:    content.content,
      size_bytes: i64::try_from(content.size_bytes).unwrap_or(i64::MAX),
      truncated:  content.truncated,
    }))
  }

  async fn get_goals(&self, _request: Request<GetGoalsRequest>) -> Result<Response<GetGoalsResponse>, Status> {
    let goals = assemble_goals(&self.store.project_views());
    Ok(Response::new(GetGoalsResponse { goals }))
  }

  async fn get_goal_detail(
    &self,
    request: Request<GetGoalDetailRequest>,
  ) -> Result<Response<GetGoalDetailResponse>, Status> {
    let goal_id_text = request.into_inner().goal_id;
    if goal_id_text.trim().is_empty() {
      return Err(Status::invalid_argument("goalId が空です。"));
    }
    let goal_id = GoalId::try_from(goal_id_text.as_str())
      .map_err(|error| Status::invalid_argument(format!("goalId が不正です: {error}")))?;
    let view = self
      .store
      .find_goal_project(goal_id.as_str())
      .ok_or_else(|| Status::not_found(format!("goalId {} は状態ストアにありません。", goal_id.as_str())))?;
    let view_server_id = ServerId::try_from(view.server_id.as_str())
      .map_err(|error| Status::internal(format!("状態ストアの server_id が不正です: {error}")))?;
    let view_project = ProjectName::try_from(view.name.as_str())
      .map_err(|error| Status::internal(format!("状態ストアの project が不正です: {error}")))?;
    let resolved = self.resolve_project(&view_server_id, &view_project)?;
    let mut source = self.read_source(&resolved);
    let detail = match source.read_goal_detail(&resolved.cwd, &goal_id).await {
      | Ok(detail) => detail,
      | Err(error) => return Err(tool_error_status(error)),
    };
    Ok(Response::new(GetGoalDetailResponse {
      goal_id:             detail.goal_id.as_str().to_owned(),
      server_id:           view.server_id.clone(),
      project:             view.name.clone(),
      status:              goal_status(detail.status) as i32,
      execution_status:    execution_status(detail.execution_status) as i32,
      objective:           detail.objective,
      out_of_scope:        detail.out_of_scope,
      acceptance_criteria: detail.acceptance_criteria,
      constraints:         detail.constraints,
      specifications:      detail.specifications.into_iter().collect(),
      branch:              detail.branch,
      integration_branch:  detail.integration_branch,
      work_log:            detail.work_log.into_iter().map(goal_work_log).collect(),
      questions:           detail.questions.into_iter().map(goal_question).collect(),
      tasks:               detail.tasks.into_iter().map(goal_task).collect(),
    }))
  }

  async fn get_task_detail(
    &self,
    request: Request<GetTaskDetailRequest>,
  ) -> Result<Response<GetTaskDetailResponse>, Status> {
    let request = request.into_inner();
    if request.project.trim().is_empty() {
      return Err(Status::invalid_argument("project が空です。"));
    }
    if request.task_name.trim().is_empty() {
      return Err(Status::invalid_argument("taskName が空です。"));
    }
    let project = ProjectName::try_from(request.project.as_str())
      .map_err(|error| Status::invalid_argument(format!("project が不正です: {error}")))?;
    let task_name = TaskName::try_from(request.task_name.as_str())
      .map_err(|error| Status::invalid_argument(format!("taskName が不正です: {error}")))?;
    let goal_id = match request.goal_id.as_deref() {
      | None => None,
      | Some("") => None,
      | Some(text) => {
        Some(GoalId::try_from(text).map_err(|error| Status::invalid_argument(format!("goalId が不正です: {error}")))?)
      },
    };
    let config = load_config(&self.config_path)
      .map_err(|error| Status::unavailable(format!("設定を読み込めませんでした: {error}")))?;
    let configured = config
      .projects
      .iter()
      .find(|configured| configured.name == project.as_str())
      .ok_or_else(|| Status::not_found(format!("プロジェクト {} は設定にありません。", project.as_str())))?;
    let configured_server_id = ServerId::try_from(configured.server.as_str())
      .map_err(|error| Status::internal(format!("設定のサーバー ID が不正です: {error}")))?;
    let resolved = resolve_project_in(&config, &configured_server_id, &project)?;
    let mut source = self.read_source(&resolved);
    let detail = match source.read_task_detail(&resolved.cwd, &project, &task_name, goal_id.as_ref()).await {
      | Ok(Some(detail)) => detail,
      | Ok(None) => {
        return Err(Status::not_found(format!(
          "タスク {} はプロジェクト {} にありません。",
          request.task_name, request.project
        )));
      },
      | Err(error) => return Err(tool_error_status(error)),
    };
    Ok(Response::new(GetTaskDetailResponse {
      project:             detail.project.as_str().to_owned(),
      task_name:           detail.task_name.as_str().to_owned(),
      goal_id:             detail.goal_id.map(|goal_id| goal_id.as_str().to_owned()),
      status:              task_status(detail.status) as i32,
      summary:             detail.summary,
      created_at:          optional_timestamp(detail.created_at),
      started_at:          optional_timestamp(detail.started_at),
      completed_at:        optional_timestamp(detail.completed_at),
      run_slug:            detail.run_slug.map(|run_slug| run_slug.as_str().to_owned()),
      pull_request_number: detail.pull_request_number.map(|number| number.get()),
      linked_run_slugs:    detail.linked_run_slugs.into_iter().map(|slug| slug.as_str().to_owned()).collect(),
      branch:              detail.branch,
      pull_request_url:    detail.pull_request_url,
    }))
  }

  async fn get_conversation(
    &self,
    request: Request<GetConversationRequest>,
  ) -> Result<Response<GetConversationResponse>, Status> {
    let request = request.into_inner();
    let project = request.project.trim();
    if project.is_empty() {
      return Err(Status::invalid_argument("project が空です。"));
    }
    let project_name = ProjectName::try_from(project)
      .map_err(|error| Status::invalid_argument(format!("project が不正です: {error}")))?;
    let config = load_config(&self.config_path)
      .map_err(|error| Status::unavailable(format!("設定を読み込めませんでした: {error}")))?;
    let configured = config
      .projects
      .iter()
      .find(|configured| configured.name == project_name.as_str())
      .ok_or_else(|| Status::not_found(format!("project {project} は設定にありません。")))?;
    let query = ConversationQuery {
      project:       project_name,
      cwd:           configured.cwd.clone(),
      project_roots: config.projects.iter().map(|project| project.cwd.clone()).collect(),
      session_id:    request.session_id.filter(|session_id| !session_id.is_empty()),
      after_seq:     request.after_seq,
      limit:         usize::try_from(request.limit.max(1)).unwrap_or(usize::MAX),
    };
    let conversation = self.conversation.clone();
    let page = tokio::task::spawn_blocking(move || conversation.read(&query))
      .await
      .map_err(|error| Status::internal(format!("会話の読み取りを実行できませんでした: {error}")))?;
    Ok(Response::new(conversation_response(page)))
  }

  async fn watch_state(
    &self,
    _request: Request<WatchStateRequest>,
  ) -> Result<Response<Self::WatchStateStream>, Status> {
    // 購読を初回の組み立てより先に取り、初回送信と変化通知の間に取りこぼしを作りません。
    let mut changes = self.changes.subscribe();
    let (tx, rx) = mpsc::channel(WATCH_CHANNEL_CAPACITY);
    let initial = {
      let store = self.store.clone();
      let activity = self.activity.clone();
      let now = self.now();
      blocking(move || assemble_state_for(&store, &activity, now, assemble_runs)).await
    };
    match initial {
      | Ok(state) => {
        let _ = tx.send(Ok(state)).await;
      },
      | Err(status) => {
        let _ = tx.send(Err(status)).await;
      },
    }
    let sender = tx.clone();
    let store = self.store.clone();
    let activity = self.activity.clone();
    let clock = self.clock.clone();
    let mut shutdown = self.shutdown.clone();
    tokio::spawn(async move {
      // 期限到達の再配信のためのタイマー。初回の即時送信の直後から数え始めます。
      let mut resend = tokio::time::interval(WATCH_RESEND_INTERVAL);
      resend.tick().await;
      loop {
        // 終了判断（クライアント切断・停止通知・購読の終端）と、変化通知・期限到達の
        // 送信判断を別の分岐に分けます。終了する分岐だけが return し、それ以外は
        // そのまま下の送信へ進みます。
        tokio::select! {
          // クライアントがストリームを閉じたら、次の変化通知を待たずに購読を解放します。
          _ = sender.closed() => return,
          _ = shutdown.changed() => return,
          _ = resend.tick() => (),
          result = changes.recv() => match result {
            | Ok(()) | Err(broadcast::error::RecvError::Lagged(_)) => (),
            | Err(broadcast::error::RecvError::Closed) => return,
          },
        }
        let assembled = {
          let store = store.clone();
          let activity = activity.clone();
          let now = (clock)();
          blocking(move || assemble_state_for(&store, &activity, now, assemble_runs)).await
        };
        let message = match assembled {
          | Ok(state) => Ok(state),
          | Err(status) => Err(status),
        };
        // 送信の待機中も停止通知とクライアント切断を監視し、待たずに終了します。
        tokio::select! {
          result = sender.send(message) => {
            if result.is_err() {
              return;
            }
          },
          _ = sender.closed() => return,
          _ = shutdown.changed() => return,
        }
      }
    });
    Ok(Response::new(ReceiverStream::new(rx)))
  }

  async fn answer_inquiry(
    &self,
    request: Request<AnswerInquiryRequest>,
  ) -> Result<Response<AnswerInquiryResponse>, Status> {
    let request = request.into_inner();
    let key = idempotency_key(&request.idempotency_key)?;
    self.write_ledger.run("AnswerInquiry", key, &request, || self.prepare_answer(request.clone())).await
  }

  async fn approve_goal_merge(
    &self,
    request: Request<ApproveGoalMergeRequest>,
  ) -> Result<Response<ApproveGoalMergeResponse>, Status> {
    let request = request.into_inner();
    let key = idempotency_key(&request.idempotency_key)?;
    self.write_ledger.run("ApproveGoalMerge", key, &request, || self.prepare_merge(request.clone())).await
  }

  async fn approve_goal_registration(
    &self,
    request: Request<ApproveGoalRegistrationRequest>,
  ) -> Result<Response<ApproveGoalRegistrationResponse>, Status> {
    let request = request.into_inner();
    let key = idempotency_key(&request.idempotency_key)?;
    self
      .write_ledger
      .run("ApproveGoalRegistration", key, &request, || self.prepare_registration_approval(request.clone()))
      .await
  }

  async fn reject_goal_registration(
    &self,
    request: Request<RejectGoalRegistrationRequest>,
  ) -> Result<Response<RejectGoalRegistrationResponse>, Status> {
    let request = request.into_inner();
    let key = idempotency_key(&request.idempotency_key)?;
    self
      .write_ledger
      .run("RejectGoalRegistration", key, &request, || self.prepare_registration_rejection(request.clone()))
      .await
  }

  async fn list_goal_registration_proposals(
    &self,
    request: Request<ListGoalRegistrationProposalsRequest>,
  ) -> Result<Response<ListGoalRegistrationProposalsResponse>, Status> {
    let request = request.into_inner();
    if request.server_id.trim().is_empty() {
      return Err(Status::invalid_argument("server_id が空です。"));
    }
    if request.project.trim().is_empty() {
      return Err(Status::invalid_argument("project が空です。"));
    }
    let server_id = ServerId::try_from(request.server_id.as_str())
      .map_err(|error| Status::invalid_argument(format!("server_id が不正です: {error}")))?;
    let project = ProjectName::try_from(request.project.as_str())
      .map_err(|error| Status::invalid_argument(format!("project が不正です: {error}")))?;
    // 一覧は `GetPendingDecisions` の承認待ち射影と同じデータ源（ストアの snapshot.proposals）
    // から作り、ストアだけを読む。
    let proposals = self
      .store
      .registration_proposals(&server_id, &project)
      .into_iter()
      .map(|entry| GoalRegistrationProposal {
        revision:             entry.revision.get(),
        server_id:            request.server_id.clone(),
        project:              request.project.clone(),
        body:                 entry.summary,
        repository_directory: entry.repository_directory,
      })
      .collect();
    Ok(Response::new(ListGoalRegistrationProposalsResponse { proposals }))
  }
}

/// 回答の入力から takt の `answer` オブジェクトを組み立てる。
///
/// 質問は `text`、作業の依頼は `report` を使います。判断待ちの種別が分かる場合は種別と回答の
/// 種類を照合し、取り違え・同時指定は入力の誤りとして拒否します。種別が分からない場合は、
/// どちらか一方だけが指定されていればそれを使います。どちらも無い回答は拒否します。
fn answer_payload(
  kind: Option<model::InquiryKind>,
  text: Option<String>,
  report: Option<i32>,
) -> Result<serde_json::Value, Status> {
  match kind {
    | Some(model::InquiryKind::Question) => {
      if report.is_some() {
        return Err(Status::invalid_argument("質問への回答には text を使い、report は使えません。"));
      }
      let text = text.ok_or_else(|| Status::invalid_argument("質問への回答には text が必要です。"))?;
      Ok(serde_json::json!({ "text": text }))
    },
    | Some(model::InquiryKind::OperationRequest) => {
      if text.is_some() {
        return Err(Status::invalid_argument("作業の依頼への回答には report を使い、text は使えません。"));
      }
      report_payload(report)
    },
    | None => match (text, report) {
      | (Some(_), Some(_)) => {
        Err(Status::invalid_argument("回答には text か report のどちらか一方だけを指定してください。"))
      },
      | (Some(text), None) => Ok(serde_json::json!({ "text": text })),
      | (None, Some(_)) => report_payload(report),
      | (None, None) => Err(Status::invalid_argument("回答には text か report のどちらかが必要です。")),
    },
  }
}

/// operation_request への回答 `report` を takt の `answer` オブジェクトへ変換する。
fn report_payload(report: Option<i32>) -> Result<serde_json::Value, Status> {
  let report = report.ok_or_else(|| Status::invalid_argument("作業の依頼への回答には report が必要です。"))?;
  let report =
    OperationAnswerReport::try_from(report).map_err(|_| Status::invalid_argument("report が未知の値です。"))?;
  let report = match report {
    | OperationAnswerReport::Completed => "completed",
    | OperationAnswerReport::Unable => "unable",
    | OperationAnswerReport::Unspecified => {
      return Err(Status::invalid_argument("report が指定されていません。"));
    },
  };
  Ok(serde_json::json!({ "report": report }))
}

/// `listener` で待ち受け、`stop` が真になったら終了する gRPC サーバを起動する。
pub async fn serve_with_listener(
  listener: tokio::net::TcpListener,
  service: TaktDashServiceImpl,
  mut stop: watch::Receiver<bool>,
) -> Result<(), tonic::transport::Error> {
  let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
  let shutdown = async move {
    while !*stop.borrow() {
      if stop.changed().await.is_err() {
        return;
      }
    }
  };
  tonic::transport::Server::builder()
    .add_service(v1::takt_dash_service_server::TaktDashServiceServer::new(service))
    .serve_with_incoming_shutdown(incoming, shutdown)
    .await
}

//! proto/taktdash.proto の TaktDashService（読み取り系と書き込み系の API）の実装。
//!
//! 状態ストアの投影と activity のクエリから、proto のメッセージ型どおりの応答を組み立てます。
//! unary の4つ（GetOverview・GetProjects・GetPendingDecisions・GetActivity）はストアから読みます。
//! GetRunDetail は要求時に takt-mcp の takt_get_run を呼びます。GetConversation は保存領域の
//! manager の会話を読みます。GetGoals・GetGoalDetail・GetTaskDetail はゴールの一覧と詳細を
//! 返し、GetGoalDetail と GetTaskDetail は要求時に takt-mcp を呼びます。WatchState は接続時に
//! 全件を返し、定期読み取りの変化通知を受けるたびに状態を組み立て直して送ります。

pub mod mapping;

use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

use chrono::{DateTime, Utc};
use mapping::{
  activity_kind, connection_state, conversation_speaker, execution_status, goal_question, goal_status, goal_task,
  goal_work_log, inquiry_kind, optional_timestamp, pending_decision_type, run_status, step_log, task_status, timestamp,
};
use tokio::sync::{broadcast, mpsc, watch};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

use crate::{
  activity::ActivityStore,
  config::{load_config, Config},
  conversation::{ConversationPage, ConversationQuery, ConversationReader},
  domain::{
    GoalId, InquiryId, MergeUntil, ProjectName, PullRequestNumber, RegistrationRevision, RunSlug, ServerId, TaskCount,
    TaskName,
  },
  mcp::{client_config, McpClient, McpClientRegistry, ToolError, ToolSet},
  model::{self, TaskEntry},
  store::StateStore,
  takt_dash::v1::{
    self, ActivityEvent, AnswerInquiryRequest, AnswerInquiryResponse, ApproveGoalMergeRequest,
    ApproveGoalMergeResponse, ApproveGoalRegistrationRequest, ApproveGoalRegistrationResponse, DashboardState,
    GetActivityRequest, GetActivityResponse, GetConversationRequest, GetConversationResponse, GetGoalDetailRequest,
    GetGoalDetailResponse, GetGoalsRequest, GetGoalsResponse, GetOverviewRequest, GetPendingDecisionsRequest,
    GetPendingDecisionsResponse, GetProjectsRequest, GetProjectsResponse, GetRunDetailRequest, GetRunDetailResponse,
    GetTaskDetailRequest, GetTaskDetailResponse, GoalRegistrationProposal, ListGoalRegistrationProposalsRequest,
    ListGoalRegistrationProposalsResponse, OperationAnswerReport, PendingDecision, Project,
    RejectGoalRegistrationRequest, RejectGoalRegistrationResponse, Run, RunInstruction, RunReport, Server,
    TaskStatusCounts, WatchStateRequest,
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

/// 読み取り系 API のサービス。
pub struct TaktDashServiceImpl {
  store:        Arc<StateStore>,
  activity:     Arc<ActivityStore>,
  config_path:  PathBuf,
  conversation: Arc<ConversationReader>,
  changes:      broadcast::Sender<()>,
  shutdown:     watch::Receiver<bool>,
  registry:     McpClientRegistry,
  clock:        Clock,
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
  /// GetRunDetail は要求のたびに設定のパスから対象サーバーの起動方法と cwd を解決し、
  /// takt-mcp の子プロセスを起動して takt_get_run を呼びます。
  pub fn new(deps: ServiceDeps) -> Self {
    Self::with_clock(deps, Arc::new(Utc::now))
  }

  /// 現在時刻を返す関数を注入してサービスを作る。既定は実時計です。
  pub fn with_clock(deps: ServiceDeps, clock: Clock) -> Self {
    let ServiceDeps { store, activity, config_path, conversation, changes, shutdown, registry } = deps;
    TaktDashServiceImpl { store, activity, config_path, conversation, changes, shutdown, registry, clock }
  }

  /// サービスが今回の要求で使う現在時刻を返す。
  fn now(&self) -> DateTime<Utc> {
    (self.clock)()
  }

  /// 状態ストアから DashboardState を組み立てる。
  fn assemble_state(&self, now: DateTime<Utc>) -> DashboardState {
    assemble_state_for(self.store.as_ref(), self.activity.as_ref(), now)
  }
}

/// サーバーの接続状態と読めた時刻を、そのサーバーのプロジェクト群から組み立てる。
fn assemble_servers(
  servers: &BTreeMap<String, crate::config::ServerMeta>,
  views: &[model::ProjectView],
) -> Vec<Server> {
  servers
    .iter()
    .map(|(server_id, meta)| {
      let server_projects: Vec<&model::ProjectView> =
        views.iter().filter(|view| &view.server_id == server_id).collect();
      let connected = server_projects.iter().any(|view| view.connected);
      let last_read_at = server_projects.iter().filter_map(|view| view.last_read_at).max();
      Server {
        server_id: server_id.clone(),
        name: meta.name.clone(),
        kind: meta.kind.clone(),
        connected,
        last_read_at: optional_timestamp(last_read_at),
        concurrency: meta.concurrency,
      }
    })
    .collect()
}

/// プロジェクトの一覧を、直近24時間の完了タスクの件数つきで組み立てる。
fn assemble_projects(views: &[model::ProjectView], activity: &ActivityStore, now: DateTime<Utc>) -> Vec<Project> {
  let since = now - chrono::Duration::hours(24);
  views
    .iter()
    .map(|view| {
      let completed_tasks_24h =
        activity.completed_tasks_since(&view.name, since).map(TaskCount::get).unwrap_or_default();
      Project {
        name: view.name.clone(),
        about: view.about.clone(),
        server_id: view.server_id.clone(),
        completed_tasks_24h,
        connected: view.connected,
        last_read_at: optional_timestamp(view.last_read_at),
        connection_state: connection_state(view.connection_state) as i32,
      }
    })
    .collect()
}

/// ゴールの一覧を、タスクの件数と仕上げタスクの最後の状態つきで組み立てる。
fn assemble_goals(views: &[model::ProjectView]) -> Vec<v1::Goal> {
  let mut goals = Vec::new();
  for view in views {
    let merge_tasks: Vec<&TaskEntry> = view.tasks.iter().filter(|task| task.merge_task).collect();
    for goal in &view.goals {
      let goal_tasks: Vec<&TaskEntry> =
        view.tasks.iter().filter(|task| task.goal_id.as_ref() == Some(&goal.goal_id)).collect();
      let merge_task_last = merge_tasks
        .iter()
        .rev()
        .find(|task| task.goal_id.as_ref() == Some(&goal.goal_id))
        .map(|task| task_status(task.status) as i32);
      goals.push(v1::Goal {
        goal_id: goal.goal_id.as_str().to_owned(),
        server_id: view.server_id.clone(),
        project: view.name.clone(),
        name: goal.objective.clone(),
        status: goal_status(goal.status) as i32,
        execution_status: execution_status(goal.execution_status) as i32,
        task_counts: Some(task_status_counts(&goal_tasks)),
        paused_reason: goal.paused_reason.clone(),
        pull_request: pull_request(
          goal.pull_request_number.map(PullRequestNumber::get),
          goal.pull_request_url.as_deref(),
        ),
        awaiting_merge_at: optional_timestamp(goal.awaiting_merge_at),
        merge_task_last,
      });
    }
  }
  goals
}

fn task_status_counts(tasks: &[&TaskEntry]) -> TaskStatusCounts {
  // 状態ごとの件数も `TaskCount` を通し、`i32` の上限を超える件数は proto の表現に合わせて
  // 飽和させます。実運用の件数は上限に届きませんが、集計の型を迂回しません。
  let count = |status: model::TaskStatus| {
    let total = tasks.iter().filter(|task| task.status == status).count();
    let total = i64::try_from(total).unwrap_or(i64::MAX);
    TaskCount::try_from(total).map(TaskCount::get).unwrap_or(i32::MAX)
  };
  TaskStatusCounts {
    pending:   count(model::TaskStatus::Pending),
    running:   count(model::TaskStatus::Running),
    completed: count(model::TaskStatus::Completed),
    failed:    count(model::TaskStatus::Failed),
    exceeded:  count(model::TaskStatus::Exceeded),
    pr_failed: count(model::TaskStatus::PrFailed),
  }
}

fn pull_request(number: Option<i64>, url: Option<&str>) -> Option<v1::PullRequest> {
  match (number, url) {
    | (Some(number), Some(url)) => Some(v1::PullRequest { number, url: url.to_owned() }),
    | _ => None,
  }
}

/// run の一覧を組み立てる。ログの無音の判定は状態ストアの閾値に従います。
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

/// 判断待ちの一覧を、状態ストアの投影（未回答の問い合わせ・人のマージ待ち・
/// ゴール登録の承認待ち）から組み立てる。
fn assemble_pending_decisions(store: &StateStore, now: DateTime<Utc>) -> Vec<PendingDecision> {
  store
    .pending_decisions(now, model::PendingDecisionFilter::All)
    .into_iter()
    .map(|decision| PendingDecision {
      r#type:       pending_decision_type(decision.decision_type) as i32,
      server_id:    decision.server_id,
      project:      decision.project,
      goal_id:      decision.goal_id,
      inquiry_kind: inquiry_kind(decision.inquiry_kind) as i32,
      created_at:   Some(timestamp(decision.created_at)),
      body:         decision.body,
      options:      decision.options,
      pull_request: pull_request(
        decision.pull_request_number.map(PullRequestNumber::get),
        decision.pull_request_url.as_deref(),
      ),
      stale:        decision.stale,
      inquiry_id:   decision.inquiry_id.as_ref().map(|inquiry_id| inquiry_id.as_str().to_owned()).unwrap_or_default(),
      revision:     decision.revision.map(RegistrationRevision::get),
    })
    .collect()
}

fn activity_event(event: model::ActivityEvent) -> ActivityEvent {
  ActivityEvent {
    occurred_at: Some(timestamp(event.occurred_at)),
    server_id:   event.server_id,
    project:     event.project,
    kind:        activity_kind(event.kind) as i32,
    body:        event.body,
    run_slug:    event.run_slug.map(|run_slug| run_slug.as_str().to_owned()),
  }
}

/// takt-mcp の道具エラーを gRPC のステータスへ変換する境界。
fn tool_error_status(error: ToolError) -> Status {
  match error {
    | ToolError::Parse(message) => Status::internal(format!("takt-mcp の応答を解析できませんでした: {message}")),
    | ToolError::Tool(error) => Status::unavailable(format!("takt-mcp の道具が失敗しました: {error}")),
    | ToolError::Transport(error) => Status::unavailable(format!("takt-mcp へ接続できませんでした: {error}")),
  }
}

/// 設定から解決した、対象サーバーの起動方法とプロジェクトの cwd。
struct ResolvedProject {
  server_id:     ServerId,
  project:       ProjectName,
  client_config: crate::mcp::McpClientConfig,
  cwd:           PathBuf,
}

/// 読み込み済みの設定から、`server_id` と `project` の起動方法と cwd を解決する。
///
/// 設定に無いサーバー・プロジェクトは `unavailable` で返し、takt-mcp を起動しません。
fn resolve_project_in(config: &Config, server_id: &ServerId, project: &ProjectName) -> Result<ResolvedProject, Status> {
  let server = config
    .servers
    .get(server_id.as_str())
    .ok_or_else(|| Status::unavailable(format!("サーバー {} の起動方法が設定にありません。", server_id.as_str())))?
    .clone();
  let cwd = config
    .projects
    .iter()
    .find(|entry| entry.name == project.as_str() && entry.server == server_id.as_str())
    .map(|entry| entry.cwd.clone())
    .ok_or_else(|| Status::unavailable(format!("プロジェクト {} の cwd が設定にありません。", project.as_str())))?;
  Ok(ResolvedProject {
    server_id: server_id.clone(),
    project: project.clone(),
    client_config: client_config(&server, &cwd),
    cwd,
  })
}

impl TaktDashServiceImpl {
  /// 設定を読み、`server_id` と `project` から対象サーバーの起動方法と cwd を解決する。
  ///
  /// 設定に無いサーバー・プロジェクトは `unavailable` で返し、takt-mcp を起動しません。
  fn resolve_project(&self, server_id: &ServerId, project: &ProjectName) -> Result<ResolvedProject, Status> {
    let config = load_config(&self.config_path)
      .map_err(|error| Status::unavailable(format!("設定を読み込めませんでした: {error}")))?;
    resolve_project_in(&config, server_id, project)
  }

  /// 書き込み RPC の共通の入口。型で検証済みの対象を解決する。
  fn resolve_write_target(&self, server_id: &ServerId, project: &ProjectName) -> Result<ResolvedProject, Status> {
    self.resolve_project(server_id, project)
  }
}

/// 会話のページを proto の応答へ写す。
fn conversation_response(page: ConversationPage) -> GetConversationResponse {
  GetConversationResponse {
    project:            page.project.as_str().to_owned(),
    session_id:         page.session_id,
    provider:           page.provider,
    messages:           page
      .messages
      .into_iter()
      .map(|message| v1::ConversationMessage {
        seq:         message.seq,
        occurred_at: Some(timestamp(message.occurred_at)),
        speaker:     conversation_speaker(message.speaker) as i32,
        text:        message.text,
        notice:      message.notice,
      })
      .collect(),
    next_after_seq:     page.next_after_seq,
    has_more:           page.has_more,
    skipped:            page
      .skipped
      .into_iter()
      .map(|summary| v1::ConversationSkipped {
        reason: summary.reason,
        count:  i32::try_from(summary.count).unwrap_or(i32::MAX),
      })
      .collect(),
    unavailable_reason: page.unavailable_reason,
  }
}

#[tonic::async_trait]
impl v1::takt_dash_service_server::TaktDashService for TaktDashServiceImpl {
  type WatchStateStream = ReceiverStream<Result<DashboardState, Status>>;

  async fn get_overview(&self, _request: Request<GetOverviewRequest>) -> Result<Response<DashboardState>, Status> {
    Ok(Response::new(self.assemble_state(self.now())))
  }

  async fn get_projects(&self, _request: Request<GetProjectsRequest>) -> Result<Response<GetProjectsResponse>, Status> {
    let state = self.assemble_state(self.now());
    let recent_events = self
      .activity
      .recent_events(RECENT_EVENTS_LIMIT)
      .map_err(|error| Status::internal(format!("出来事の一覧を取得できませんでした: {error}")))?
      .into_iter()
      .map(activity_event)
      .collect();
    Ok(Response::new(GetProjectsResponse { state: Some(state), recent_events }))
  }

  async fn get_pending_decisions(
    &self,
    _request: Request<GetPendingDecisionsRequest>,
  ) -> Result<Response<GetPendingDecisionsResponse>, Status> {
    Ok(Response::new(GetPendingDecisionsResponse { state: Some(self.assemble_state(self.now())) }))
  }

  async fn get_activity(&self, _request: Request<GetActivityRequest>) -> Result<Response<GetActivityResponse>, Status> {
    let now = self.now();
    let state = self.assemble_state(now);
    let events = self
      .activity
      .recent_events(ACTIVITY_EVENTS_LIMIT)
      .map_err(|error| Status::internal(format!("出来事の一覧を取得できませんでした: {error}")))?
      .into_iter()
      .map(activity_event)
      .collect();
    let daily_summaries = self
      .activity
      .daily_summaries(now)
      .map_err(|error| Status::internal(format!("日次集計を取得できませんでした: {error}")))?
      .into_iter()
      .map(|summary| v1::DailySummary {
        date:      summary.date.as_str().to_owned(),
        total:     summary.total,
        attention: summary.attention,
      })
      .collect();
    let today_counts_by_server = self
      .activity
      .today_counts_by_server(now)
      .map_err(|error| Status::internal(format!("サーバーごとの件数を取得できませんでした: {error}")))?
      .into_iter()
      .map(|count| v1::ServerEventCount { server_id: count.server_id.as_str().to_owned(), count: count.count })
      .collect();
    Ok(Response::new(GetActivityResponse { state: Some(state), events, daily_summaries, today_counts_by_server }))
  }

  async fn get_run_detail(
    &self,
    request: Request<GetRunDetailRequest>,
  ) -> Result<Response<GetRunDetailResponse>, Status> {
    let run_slug_text = request.into_inner().run_slug;
    if run_slug_text.trim().is_empty() {
      return Err(Status::invalid_argument("runSlug が空です。"));
    }
    let run_slug = RunSlug::try_from(run_slug_text.as_str())
      .map_err(|error| Status::invalid_argument(format!("runSlug が不正です: {error}")))?;
    let view = self
      .store
      .find_run_project(&run_slug)
      .ok_or_else(|| Status::not_found(format!("runSlug {} は状態ストアにありません。", run_slug.as_str())))?;
    let stored_run = view.runs.iter().find(|run| run.run_slug == run_slug.as_str()).cloned();
    // 完了した run は実行中一覧から消えるため、状態ストアの完了タスクからゴールと
    // タスク名を補います。run の詳細 URL は完了後も同じまま取得できます。
    let stored_task = view.tasks.iter().find(|task| task.run_slug.as_ref() == Some(&run_slug));
    let view_server_id = ServerId::try_from(view.server_id.as_str())
      .map_err(|error| Status::internal(format!("状態ストアの server_id が不正です: {error}")))?;
    let view_project = ProjectName::try_from(view.name.as_str())
      .map_err(|error| Status::internal(format!("状態ストアの project が不正です: {error}")))?;
    let resolved = self.resolve_project(&view_server_id, &view_project)?;
    let mut client = McpClient::new(resolved.client_config);
    let detail = match ToolSet::new(&mut client, resolved.cwd.clone()).read_run_detail(&run_slug).await {
      | Ok(detail) => detail,
      | Err(error) => {
        client.shutdown().await;
        return Err(tool_error_status(error));
      },
    };
    client.shutdown().await;
    let server_id = resolved.server_id.as_str().to_owned();
    // 対象 run のゴールとタスク名は、状態ストアの run（定期読み取りで task
    // から解決済み）を正とする。 takt_get_run の応答に無い場合だけ、ストアの値で補う。
    // run が完了して一覧から消えている場合は、完了タスクの値で補う。
    let goal_id = detail
      .goal_id
      .clone()
      .or_else(|| stored_run.as_ref().and_then(|run| run.goal_id.clone()))
      .or_else(|| stored_task.and_then(|task| task.goal_id.clone()));
    let fallback_task_name =
      stored_run.as_ref().map(|run| run.task_name.clone()).or_else(|| stored_task.map(|task| task.name.clone()));
    let task_name = detail
      .task_name
      .clone()
      .or(fallback_task_name)
      .map(|task_name| task_name.as_str().to_owned())
      .unwrap_or_default();
    let session_id = if detail.session_id.is_empty() {
      stored_run.as_ref().map(|run| run.session_id.clone()).unwrap_or_default()
    } else {
      detail.session_id.clone()
    };
    let log_silent = self.store.is_log_silent_at(detail.last_log_at, self.now());
    Ok(Response::new(GetRunDetailResponse {
      run_slug: run_slug.as_str().to_owned(),
      session_id,
      server_id,
      project: view.name.clone(),
      goal_id: goal_id.map(|goal_id| goal_id.as_str().to_owned()).unwrap_or_default(),
      task_name,
      task_text: detail.task_text,
      summary: detail.summary,
      workflow: detail.workflow,
      status: run_status(detail.status) as i32,
      started_at: optional_timestamp(detail.started_at),
      current_step: detail.current_step,
      current_iteration: detail.current_iteration,
      current_step_started_at: optional_timestamp(detail.current_step_started_at),
      current_step_execution_count: detail.current_step_execution_count,
      phase: detail.phase,
      step_logs: detail.step_logs.iter().map(step_log).collect(),
      log_tail: detail.log_tail,
      reports: detail
        .reports
        .into_iter()
        .map(|report| RunReport {
          filename:   report.filename,
          content:    report.content,
          size_bytes: report.size_bytes,
        })
        .collect(),
      instructions: detail
        .instructions
        .into_iter()
        .map(|instruction| RunInstruction {
          instruction_id: instruction.instruction_id,
          issued_at:      Some(timestamp(instruction.issued_at)),
          state:          instruction.state,
          content:        instruction.content,
        })
        .collect(),
      last_log_at: optional_timestamp(detail.last_log_at),
      log_silent,
      server_connected: view.connected,
      server_last_read_at: optional_timestamp(view.last_read_at),
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
    let mut client = McpClient::new(resolved.client_config);
    let detail = match ToolSet::new(&mut client, resolved.cwd.clone()).read_goal_detail(goal_id.as_str()).await {
      | Ok(detail) => detail,
      | Err(error) => {
        client.shutdown().await;
        return Err(tool_error_status(error));
      },
    };
    client.shutdown().await;
    Ok(Response::new(GetGoalDetailResponse {
      goal_id:             detail.goal_id,
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
    let mut client = McpClient::new(resolved.client_config);
    let detail = match ToolSet::new(&mut client, resolved.cwd.clone())
      .read_task_detail(project.as_str(), task_name.as_str(), goal_id.as_ref().map(|goal_id| goal_id.as_str()))
      .await
    {
      | Ok(Some(detail)) => detail,
      | Ok(None) => {
        client.shutdown().await;
        return Err(Status::not_found(format!(
          "タスク {} はプロジェクト {} にありません。",
          request.task_name, request.project
        )));
      },
      | Err(error) => {
        client.shutdown().await;
        return Err(tool_error_status(error));
      },
    };
    client.shutdown().await;
    Ok(Response::new(GetTaskDetailResponse {
      project:             detail.project,
      task_name:           detail.task_name,
      goal_id:             detail.goal_id,
      status:              task_status(detail.status) as i32,
      summary:             detail.summary,
      created_at:          optional_timestamp(detail.created_at),
      started_at:          optional_timestamp(detail.started_at),
      completed_at:        optional_timestamp(detail.completed_at),
      run_slug:            detail.run_slug,
      pull_request_number: detail.pull_request_number,
      linked_run_slugs:    detail.linked_run_slugs,
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
    let _ = tx.send(Ok(self.assemble_state(self.now()))).await;
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
        let state = assemble_state_for(&store, &activity, (clock)());
        // 送信の待機中も停止通知とクライアント切断を監視し、待たずに終了します。
        tokio::select! {
          result = sender.send(Ok(state)) => {
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
    if request.inquiry_id.trim().is_empty() {
      return Err(Status::invalid_argument("inquiry_id が空です。"));
    }
    if request.server_id.trim().is_empty() {
      return Err(Status::invalid_argument("server_id が空です。"));
    }
    if request.project.trim().is_empty() {
      return Err(Status::invalid_argument("project が空です。"));
    }
    // 保留項目は (server_id, project, inquiry_id) の3条件一致で引き当てる。同じ id が別の
    // プロジェクトや別のサーバーにあっても、対象の判断だけを選ぶ。
    let matches: Vec<model::PendingDecision> = self
      .store
      .pending_decisions(self.now(), model::PendingDecisionFilter::Inquiry)
      .into_iter()
      .filter(|decision| {
        decision.server_id == request.server_id
          && decision.project == request.project
          && decision.inquiry_id.as_ref().map(InquiryId::as_str) == Some(request.inquiry_id.as_str())
      })
      .collect();
    let pending = match matches.len() {
      | 0 => None,
      | 1 => matches.into_iter().next(),
      | _ => {
        return Err(Status::invalid_argument(format!(
          "問い合わせ {} はサーバー {} のプロジェクト {} 内で一意に特定できません。",
          request.inquiry_id, request.server_id, request.project
        )));
      },
    };
    let inquiry_kind = pending.as_ref().and_then(|decision| decision.inquiry_kind);
    let answer = answer_payload(inquiry_kind, request.text, request.report)?;
    // 保留項目を引き当てられない問い合わせは、要求がゴールを明示していても回答しません。
    // 回答本文の検証を先に済ませることで、空の回答は入力の誤りとして扱います。
    let Some(pending) = pending else {
      return Err(Status::not_found(format!(
        "問い合わせ {} はサーバー {} のプロジェクト {} にありません。",
        request.inquiry_id, request.server_id, request.project
      )));
    };
    // takt_answer_goal_question の入力契約では goalId は必須（UUID）です。要求が明示した
    // ゴールを優先し、無ければ引き当てた判断待ちのゴールを使います。要求の値の誤りは入力の
    // 誤り、状態ストア由来の値の誤りは内部の不整合として扱います。
    let requested_goal_id = request.goal_id.clone().filter(|goal_id| !goal_id.trim().is_empty());
    let goal_id = match requested_goal_id {
      | Some(goal_id) => GoalId::try_from(goal_id.as_str())
        .map_err(|error| Status::invalid_argument(format!("goal_id が不正です: {error}")))?,
      | None => {
        let goal_id = pending.goal_id.clone().filter(|goal_id| !goal_id.trim().is_empty()).ok_or_else(|| {
          Status::invalid_argument("goal_id を特定できません。ゴールに属さない問い合わせには回答できません。")
        })?;
        GoalId::try_from(goal_id.as_str())
          .map_err(|error| Status::internal(format!("状態ストアの goal_id が不正です: {error}")))?
      },
    };
    let question_id = InquiryId::try_from(request.inquiry_id.as_str())
      .map_err(|error| Status::invalid_argument(format!("inquiry_id が不正です: {error}")))?;
    let server_id = ServerId::try_from(request.server_id.as_str())
      .map_err(|error| Status::invalid_argument(format!("server_id が不正です: {error}")))?;
    let project = ProjectName::try_from(request.project.as_str())
      .map_err(|error| Status::invalid_argument(format!("project が不正です: {error}")))?;
    let resolved = self.resolve_write_target(&server_id, &project)?;
    let client =
      self.registry.client(resolved.server_id.as_str(), resolved.project.as_str(), || resolved.client_config.clone());
    let mut client = client.lock().await;
    ToolSet::new(&mut client, resolved.cwd.clone())
      .answer_goal_question(&goal_id, &question_id, answer)
      .await
      .map_err(tool_error_status)?;
    Ok(Response::new(AnswerInquiryResponse { inquiry_id: question_id.as_str().to_owned() }))
  }

  async fn approve_goal_merge(
    &self,
    request: Request<ApproveGoalMergeRequest>,
  ) -> Result<Response<ApproveGoalMergeResponse>, Status> {
    let request = request.into_inner();
    if request.goal_id.trim().is_empty() {
      return Err(Status::invalid_argument("goal_id が空です。"));
    }
    if request.pr_number <= 0 {
      return Err(Status::invalid_argument("pr_number は正の整数で指定してください。"));
    }
    if !matches!(request.until.as_str(), "prepare" | "merge") {
      return Err(Status::invalid_argument("until は \"prepare\" か \"merge\" を指定してください。"));
    }
    if request.server_id.trim().is_empty() {
      return Err(Status::invalid_argument("server_id が空です。"));
    }
    if request.project.trim().is_empty() {
      return Err(Status::invalid_argument("project が空です。"));
    }
    let goal_id = GoalId::try_from(request.goal_id.as_str())
      .map_err(|error| Status::invalid_argument(format!("goal_id が不正です: {error}")))?;
    let pr_number = PullRequestNumber::try_from(request.pr_number)
      .map_err(|error| Status::invalid_argument(format!("pr_number が不正です: {error}")))?;
    let until = MergeUntil::try_from(request.until.as_str())
      .map_err(|error| Status::invalid_argument(format!("until が不正です: {error}")))?;
    let server_id = ServerId::try_from(request.server_id.as_str())
      .map_err(|error| Status::invalid_argument(format!("server_id が不正です: {error}")))?;
    let project = ProjectName::try_from(request.project.as_str())
      .map_err(|error| Status::invalid_argument(format!("project が不正です: {error}")))?;
    let resolved = self.resolve_write_target(&server_id, &project)?;
    let client =
      self.registry.client(resolved.server_id.as_str(), resolved.project.as_str(), || resolved.client_config.clone());
    let mut client = client.lock().await;
    let result = ToolSet::new(&mut client, resolved.cwd.clone())
      .merge_goal_pr(&goal_id, pr_number, until)
      .await
      .map_err(tool_error_status)?;
    Ok(Response::new(ApproveGoalMergeResponse {
      task_name: result.task_name.as_str().to_owned(),
      workflow:  result.workflow,
    }))
  }

  async fn approve_goal_registration(
    &self,
    request: Request<ApproveGoalRegistrationRequest>,
  ) -> Result<Response<ApproveGoalRegistrationResponse>, Status> {
    let request = request.into_inner();
    if request.revision <= 0 {
      return Err(Status::invalid_argument("revision は正の整数で指定してください。"));
    }
    let revision = RegistrationRevision::try_from(request.revision)
      .map_err(|error| Status::invalid_argument(format!("revision が不正です: {error}")))?;
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
    let resolved = self.resolve_write_target(&server_id, &project)?;
    let client =
      self.registry.client(resolved.server_id.as_str(), resolved.project.as_str(), || resolved.client_config.clone());
    let mut client = client.lock().await;
    let result = ToolSet::new(&mut client, resolved.cwd.clone())
      .approve_goal_registration(revision)
      .await
      .map_err(tool_error_status)?;
    Ok(Response::new(ApproveGoalRegistrationResponse {
      revision: result.revision.get(),
      goal_id:  result.goal_id.as_str().to_owned(),
    }))
  }

  async fn reject_goal_registration(
    &self,
    request: Request<RejectGoalRegistrationRequest>,
  ) -> Result<Response<RejectGoalRegistrationResponse>, Status> {
    let request = request.into_inner();
    if request.revision <= 0 {
      return Err(Status::invalid_argument("revision は正の整数で指定してください。"));
    }
    let revision = RegistrationRevision::try_from(request.revision)
      .map_err(|error| Status::invalid_argument(format!("revision が不正です: {error}")))?;
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
    let resolved = self.resolve_write_target(&server_id, &project)?;
    let client =
      self.registry.client(resolved.server_id.as_str(), resolved.project.as_str(), || resolved.client_config.clone());
    let mut client = client.lock().await;
    let rejected = ToolSet::new(&mut client, resolved.cwd.clone())
      .reject_goal_registration(revision)
      .await
      .map_err(tool_error_status)?;
    Ok(Response::new(RejectGoalRegistrationResponse { revision: rejected.get() }))
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

/// 状態ストアと activity のクエリから DashboardState を組み立てる。
///
/// サーバーの表示情報と閾値は呼び出しのたびに状態ストアから読むため、稼働中の設定変更が
/// 次の組み立て（WatchState の変化通知後の再組み立てを含む）へ反映されます。
fn assemble_state_for(store: &StateStore, activity: &ActivityStore, now: DateTime<Utc>) -> DashboardState {
  let views = store.project_views();
  let servers = store.servers();
  let thresholds = store.thresholds();
  DashboardState {
    read_at:           optional_timestamp(views.iter().filter_map(|view| view.last_read_at).max()),
    servers:           assemble_servers(&servers, &views),
    projects:          assemble_projects(&views, activity, now),
    goals:             assemble_goals(&views),
    runs:              assemble_runs(&views, store, now),
    pending_decisions: assemble_pending_decisions(store, now),
    thresholds:        Some(v1::AnomalyThresholds {
      pending_decision_stale_minutes: thresholds.pending_decision_stale_minutes.get(),
      run_log_silent_minutes:         thresholds.run_log_silent_minutes.get(),
    }),
    build_info:        Some(crate::build_info::current()),
  }
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

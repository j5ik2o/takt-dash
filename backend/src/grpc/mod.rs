//! proto/taktdash.proto の TaktDashService（読み取り系 API）の実装。
//!
//! 状態ストアの投影と activity のクエリから、proto のメッセージ型どおりの応答を組み立てます。
//! unary の4つ（GetOverview・GetProjects・GetPendingDecisions・GetActivity）はストアから読みます。
//! GetRunDetail は要求時に takt-mcp の takt_get_run を呼びます。WatchState は接続時に全件を返し、
//! 定期読み取りの変化通知を受けるたびに状態を組み立て直して送ります。

pub mod mapping;

use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

use chrono::{DateTime, Utc};
use mapping::{
  activity_kind, execution_status, goal_status, inquiry_kind, optional_timestamp, pending_decision_type, run_status,
  step_log, task_status, timestamp,
};
use tokio::sync::{broadcast, mpsc, watch};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

use crate::{
  activity::ActivityStore,
  config::load_config,
  mcp::{client_config, McpClient, ToolError, ToolSet},
  model::{self, TaskEntry},
  store::StateStore,
  takt_dash::v1::{
    self, ActivityEvent, DashboardState, GetActivityRequest, GetActivityResponse, GetOverviewRequest,
    GetPendingDecisionsRequest, GetPendingDecisionsResponse, GetProjectsRequest, GetProjectsResponse,
    GetRunDetailRequest, GetRunDetailResponse, PendingDecision, Project, Run, RunInstruction, RunReport, Server,
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
  store:       Arc<StateStore>,
  activity:    Arc<ActivityStore>,
  config_path: PathBuf,
  changes:     broadcast::Sender<()>,
  shutdown:    watch::Receiver<bool>,
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
  /// GetRunDetail は要求のたびに設定のパスから対象サーバーの起動方法と cwd を解決し、
  /// takt-mcp の子プロセスを起動して takt_get_run を呼びます。
  pub fn new(
    store: Arc<StateStore>,
    activity: Arc<ActivityStore>,
    config_path: PathBuf,
    changes: broadcast::Sender<()>,
    shutdown: watch::Receiver<bool>,
  ) -> Self {
    TaktDashServiceImpl { store, activity, config_path, changes, shutdown }
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
      let completed_tasks_24h = activity.completed_tasks_since(&view.name, since).unwrap_or_default();
      Project {
        name: view.name.clone(),
        about: view.about.clone(),
        server_id: view.server_id.clone(),
        completed_tasks_24h,
        connected: view.connected,
        last_read_at: optional_timestamp(view.last_read_at),
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
        view.tasks.iter().filter(|task| task.goal_id.as_deref() == Some(goal.goal_id.as_str())).collect();
      let merge_task_last = merge_tasks
        .iter()
        .rev()
        .find(|task| task.goal_id.as_deref() == Some(goal.goal_id.as_str()))
        .map(|task| task_status(task.status) as i32);
      goals.push(v1::Goal {
        goal_id: goal.goal_id.clone(),
        server_id: view.server_id.clone(),
        project: view.name.clone(),
        name: goal.objective.clone(),
        status: goal_status(goal.status) as i32,
        execution_status: execution_status(goal.execution_status) as i32,
        task_counts: Some(task_status_counts(&goal_tasks)),
        paused_reason: goal.paused_reason.clone(),
        pull_request: pull_request(goal.pull_request_number, goal.pull_request_url.as_deref()),
        awaiting_merge_at: optional_timestamp(goal.awaiting_merge_at),
        merge_task_last,
      });
    }
  }
  goals
}

fn task_status_counts(tasks: &[&TaskEntry]) -> TaskStatusCounts {
  let mut counts = TaskStatusCounts::default();
  for task in tasks {
    match task.status {
      | model::TaskStatus::Pending => counts.pending += 1,
      | model::TaskStatus::Running => counts.running += 1,
      | model::TaskStatus::Completed => counts.completed += 1,
      | model::TaskStatus::Failed => counts.failed += 1,
      | model::TaskStatus::Exceeded => counts.exceeded += 1,
      | model::TaskStatus::PrFailed => counts.pr_failed += 1,
    }
  }
  counts
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
        goal_id:                 run.goal_id.clone().unwrap_or_default(),
        task_name:               run.task_name.clone(),
        summary:                 run.summary.clone(),
        started_at:              Some(timestamp(run.started_at)),
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
        last_log_at:             Some(timestamp(run.last_log_at)),
        reports:                 run
          .reports
          .iter()
          .map(|report| v1::ReportSummary { filename: report.filename.clone(), size_bytes: report.size_bytes })
          .collect(),
        status:                  run_status(run.status) as i32,
        log_silent:              store.is_log_silent(run, now),
      });
    }
  }
  runs
}

/// 判断待ちの一覧を、状態ストアの投影（未回答の問い合わせと人のマージ待ち）から組み立てる。
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
      pull_request: pull_request(decision.pull_request_number, decision.pull_request_url.as_deref()),
      stale:        decision.stale,
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
    run_slug:    event.run_slug,
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

#[tonic::async_trait]
impl v1::takt_dash_service_server::TaktDashService for TaktDashServiceImpl {
  type WatchStateStream = ReceiverStream<Result<DashboardState, Status>>;

  async fn get_overview(&self, _request: Request<GetOverviewRequest>) -> Result<Response<DashboardState>, Status> {
    Ok(Response::new(self.assemble_state(Utc::now())))
  }

  async fn get_projects(&self, _request: Request<GetProjectsRequest>) -> Result<Response<GetProjectsResponse>, Status> {
    let state = self.assemble_state(Utc::now());
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
    Ok(Response::new(GetPendingDecisionsResponse { state: Some(self.assemble_state(Utc::now())) }))
  }

  async fn get_activity(&self, _request: Request<GetActivityRequest>) -> Result<Response<GetActivityResponse>, Status> {
    let now = Utc::now();
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
        date:      summary.date,
        total:     summary.total,
        attention: summary.attention,
      })
      .collect();
    let today_counts_by_server = self
      .activity
      .today_counts_by_server(now)
      .map_err(|error| Status::internal(format!("サーバーごとの件数を取得できませんでした: {error}")))?
      .into_iter()
      .map(|count| v1::ServerEventCount { server_id: count.server_id, count: count.count })
      .collect();
    Ok(Response::new(GetActivityResponse { state: Some(state), events, daily_summaries, today_counts_by_server }))
  }

  async fn get_run_detail(
    &self,
    request: Request<GetRunDetailRequest>,
  ) -> Result<Response<GetRunDetailResponse>, Status> {
    let run_slug = request.into_inner().run_slug;
    if run_slug.trim().is_empty() {
      return Err(Status::invalid_argument("runSlug が空です。"));
    }
    let view = self
      .store
      .find_run_project(&run_slug)
      .ok_or_else(|| Status::not_found(format!("runSlug {run_slug} は状態ストアにありません。")))?;
    let stored_run = view.runs.iter().find(|run| run.run_slug == run_slug).cloned();
    // 完了した run は実行中一覧から消えるため、状態ストアの完了タスクからゴールと
    // タスク名を補います。run の詳細 URL は完了後も同じまま取得できます。
    let stored_task = view.tasks.iter().find(|task| task.run_slug.as_deref() == Some(run_slug.as_str()));
    let config = load_config(&self.config_path)
      .map_err(|error| Status::unavailable(format!("設定を読み込めませんでした: {error}")))?;
    let server_id = view.server_id.clone();
    let server = config
      .servers
      .get(&server_id)
      .ok_or_else(|| Status::unavailable(format!("サーバー {server_id} の起動方法が設定にありません。")))?
      .clone();
    let project_cwd = config
      .projects
      .iter()
      .find(|project| project.name == view.name)
      .map(|project| project.cwd.clone())
      .ok_or_else(|| Status::unavailable(format!("プロジェクト {} の cwd が設定にありません。", view.name)))?;
    let mut client = McpClient::new(client_config(&server, &project_cwd));
    let detail = match ToolSet::new(&mut client, project_cwd.clone()).read_run_detail(&run_slug).await {
      | Ok(detail) => detail,
      | Err(error) => {
        client.shutdown().await;
        return Err(tool_error_status(error));
      },
    };
    client.shutdown().await;
    // 対象 run のゴールとタスク名は、状態ストアの run（定期読み取りで task
    // から解決済み）を正とする。 takt_get_run の応答に無い場合だけ、ストアの値で補う。
    // run が完了して一覧から消えている場合は、完了タスクの値で補う。
    let goal_id = detail
      .goal_id
      .clone()
      .or_else(|| stored_run.as_ref().and_then(|run| run.goal_id.clone()))
      .or_else(|| stored_task.and_then(|task| task.goal_id.clone()));
    let fallback_task_name = stored_run
      .as_ref()
      .map(|run| run.task_name.clone())
      .filter(|name| !name.is_empty())
      .or_else(|| stored_task.map(|task| task.name.clone()));
    let task_name =
      if detail.task_name.is_empty() { fallback_task_name.unwrap_or_default() } else { detail.task_name.clone() };
    let session_id = if detail.session_id.is_empty() {
      stored_run.as_ref().map(|run| run.session_id.clone()).unwrap_or_default()
    } else {
      detail.session_id.clone()
    };
    let log_silent = self.store.is_log_silent(
      &model::RunEntry {
        run_slug:                detail.run_slug.clone(),
        session_id:              session_id.clone(),
        task_name:               task_name.clone(),
        goal_id:                 goal_id.clone(),
        summary:                 detail.summary.clone(),
        status:                  detail.status,
        current_step:            detail.current_step.clone(),
        started_at:              detail.started_at,
        current_step_started_at: detail.current_step_started_at,
        last_log_at:             detail.last_log_at,
        steps:                   Vec::new(),
        reports:                 Vec::new(),
      },
      Utc::now(),
    );
    Ok(Response::new(GetRunDetailResponse {
      run_slug: detail.run_slug,
      session_id,
      server_id,
      project: view.name.clone(),
      goal_id: goal_id.unwrap_or_default(),
      task_name,
      task_text: detail.task_text,
      summary: detail.summary,
      workflow: detail.workflow,
      status: run_status(detail.status) as i32,
      started_at: Some(timestamp(detail.started_at)),
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
      last_log_at: Some(timestamp(detail.last_log_at)),
      log_silent,
      server_connected: view.connected,
      server_last_read_at: optional_timestamp(view.last_read_at),
    }))
  }

  async fn watch_state(
    &self,
    _request: Request<WatchStateRequest>,
  ) -> Result<Response<Self::WatchStateStream>, Status> {
    // 購読を初回の組み立てより先に取り、初回送信と変化通知の間に取りこぼしを作りません。
    let mut changes = self.changes.subscribe();
    let (tx, rx) = mpsc::channel(WATCH_CHANNEL_CAPACITY);
    let _ = tx.send(Ok(self.assemble_state(Utc::now()))).await;
    let sender = tx.clone();
    let store = self.store.clone();
    let activity = self.activity.clone();
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
        let state = assemble_state_for(&store, &activity, Utc::now());
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
      pending_decision_stale_minutes: thresholds.pending_decision_stale_minutes as i32,
      run_log_silent_minutes:         thresholds.run_log_silent_minutes as i32,
    }),
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

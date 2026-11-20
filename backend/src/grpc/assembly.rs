//! 状態ストア・activity・会話の読み取り結果から、proto のメッセージ型を組み立てる。
//!
//! 状態ストアと activity の読み取りと、proto のメッセージへの写像をまとめます。`assemble_state_for`
//! は DashboardState を組み立て、unary ハンドラの GetOverview・GetProjects・GetPendingDecisions・
//! GetActivity・WatchState が共有します。`conversation_response` は会話のページを GetConversation
//! の応答へ写します。

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

use super::{
  mapping::{
    activity_kind, connection_state, conversation_speaker, execution_status, goal_status, inquiry_kind,
    optional_timestamp, pending_decision_type, task_status, timestamp,
  },
  v1::{
    self, ActivityEvent, DashboardState, GetConversationResponse, PendingDecision, Project, Run, Server,
    TaskStatusCounts,
  },
};
use crate::{
  activity::ActivityStore,
  conversation::ConversationPage,
  domain::{PullRequestNumber, RegistrationRevision, TaskCount},
  model::{self, TaskEntry},
  store::StateStore,
};

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
        concurrency: view.concurrency,
        completed_tasks_24h,
        connected: view.connected,
        last_read_at: optional_timestamp(view.last_read_at),
        connection_state: connection_state(view.connection_state) as i32,
      }
    })
    .collect()
}

/// ゴールの一覧を、タスクの件数と仕上げタスクの最後の状態つきで組み立てる。
pub(super) fn assemble_goals(views: &[model::ProjectView]) -> Vec<v1::Goal> {
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

/// 判断待ちの一覧を、状態ストアの投影（未回答の問い合わせ・人のマージ待ち・
/// ゴール登録の承認待ち）から組み立てる。
fn assemble_pending_decisions(store: &StateStore, now: DateTime<Utc>) -> Vec<PendingDecision> {
  store
    .pending_decisions(now, model::PendingDecisionFilter::All)
    .into_iter()
    .map(|decision| PendingDecision {
      r#type:          pending_decision_type(decision.decision_type) as i32,
      server_id:       decision.server_id,
      project:         decision.project,
      goal_id:         decision.goal_id,
      inquiry_kind:    inquiry_kind(decision.inquiry_kind) as i32,
      created_at:      Some(timestamp(decision.created_at)),
      body:            decision.body,
      options:         decision.options,
      pull_request:    pull_request(
        decision.pull_request_number.map(PullRequestNumber::get),
        decision.pull_request_url.as_deref(),
      ),
      stale:           decision.stale,
      inquiry_id:      decision
        .inquiry_id
        .as_ref()
        .map(|inquiry_id| inquiry_id.as_str().to_owned())
        .unwrap_or_default(),
      revision:        decision.revision.map(RegistrationRevision::get),
      merge_task_last: decision.merge_task_last.map(|status| task_status(status) as i32),
    })
    .collect()
}

pub(super) fn activity_event(event: model::ActivityEvent) -> ActivityEvent {
  ActivityEvent {
    occurred_at: Some(timestamp(event.occurred_at)),
    server_id:   event.server_id,
    project:     event.project,
    kind:        activity_kind(event.kind) as i32,
    body:        event.body,
    run_slug:    event.run_slug.map(|run_slug| run_slug.as_str().to_owned()),
  }
}

/// 会話のページを proto の応答へ写す。
pub(super) fn conversation_response(page: ConversationPage) -> GetConversationResponse {
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

/// サーバーの表示情報と閾値は呼び出しのたびに状態ストアから読むため、稼働中の設定変更が
/// 次の組み立て（WatchState の変化通知後の再組み立てを含む）へ反映されます。
pub(super) fn assemble_state_for(
  store: &StateStore,
  activity: &ActivityStore,
  now: DateTime<Utc>,
  assemble_runs: impl Fn(&[model::ProjectView], &StateStore, DateTime<Utc>) -> Vec<Run>,
) -> DashboardState {
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

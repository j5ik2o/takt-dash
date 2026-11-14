//! gRPC の読み取り系 unary RPC（GetOverview・GetProjects・GetPendingDecisions・GetActivity）が、
//! 状態ストアの投影と activity のクエリから proto のメッセージ型どおりの応答を組み立てることを、
//! 実ソケットのテストサーバと生成クライアントの経路で検証する。

mod common;
mod grpc_harness;

use std::path::Path;

use chrono::{DateTime, Duration, Utc};
use common::*;
use grpc_harness::{client, start_server, GrpcHarness};
use serde_json::json;
use takt_dash_backend::{
  model::{
    ActivityEvent, EventKind, ExecutionStatus, GoalStatus, InquiryEntry, InquiryKind, ProjectSnapshot, RunEntry,
    TaskEntry, TaskStatus,
  },
  takt_dash::v1::{
    ActivityKind, AnomalyThresholds, ExecutionStatus as ProtoExecutionStatus, GetActivityRequest, GetOverviewRequest,
    GetPendingDecisionsRequest, GetProjectsRequest, GoalStatus as ProtoGoalStatus, InquiryKind as ProtoInquiryKind,
    PendingDecisionType as ProtoPendingDecisionType, RunStatus, TaskStatus as ProtoTaskStatus,
  },
};

fn project_dir() -> &'static Path {
  Path::new("/abs/takt-dash")
}

fn goal_with_pause(id: &str, reason: &str) -> takt_dash_backend::model::GoalEntry {
  takt_dash_backend::model::GoalEntry {
    paused_reason: Some(reason.to_owned()),
    ..goal(id, GoalStatus::Created, ExecutionStatus::Paused)
  }
}

fn run_entry(run_slug: &str) -> RunEntry {
  RunEntry {
    run_slug:                run_slug.to_owned(),
    session_id:              "sess-1".to_owned(),
    task_name:               "implement".to_owned(),
    goal_id:                 Some("g1".to_owned()),
    summary:                 "実装の run".to_owned(),
    status:                  takt_dash_backend::model::RunStatus::Running,
    current_step:            Some("implement".to_owned()),
    started_at:              Some(time("2026-11-13T09:00:00Z")),
    current_step_started_at: Some(time("2026-11-13T09:05:00Z")),
    last_log_at:             Some(time("2026-11-13T09:06:00Z")),
    steps:                   vec![
      takt_dash_backend::model::RunStepEntry {
        step_name:  "plan".to_owned(),
        model:      "opus".to_owned(),
        started_at: time("2026-11-13T09:00:00Z"),
        ended_at:   Some(time("2026-11-13T09:05:00Z")),
      },
      takt_dash_backend::model::RunStepEntry {
        step_name:  "implement".to_owned(),
        model:      "sonnet".to_owned(),
        started_at: time("2026-11-13T09:05:00Z"),
        ended_at:   None,
      },
    ],
    reports:                 vec![takt_dash_backend::model::ReportEntry {
      filename:   "plan.md".to_owned(),
      size_bytes: 1234,
    }],
  }
}

/// 最後のログの時刻を今にした
/// run。一覧の組み立てで「無音ではない」経路を決定的に観測するために使う。
fn recent_run(run_slug: &str) -> RunEntry {
  let mut run = run_entry(run_slug);
  run.last_log_at = Some(Utc::now());
  run
}

/// 状態ストアに、ゴール・タスク・run・問い合わせをすべて持つ 1 プロジェクト分の状態を入れる。
fn seeded_harness() -> GrpcHarness {
  let mut merge = merge_goal("g-merge", 42);
  merge.awaiting_merge_at = Some(time("2026-11-13T08:00:00Z"));
  let snapshot = ProjectSnapshot {
    goals:         vec![
      goal_with_pause("g-pause", "公開の日取りを待っています。"),
      merge,
      goal("g-open", GoalStatus::Created, ExecutionStatus::Active),
      goal("g1", GoalStatus::Created, ExecutionStatus::Active),
    ],
    tasks:         vec![
      TaskEntry {
        name:         "impl".to_owned(),
        goal_id:      Some("g1".to_owned()),
        status:       TaskStatus::Running,
        started_at:   Some(time("2026-11-13T09:00:00Z")),
        run_slug:     Some("run-1".to_owned()),
        current_step: Some("implement".to_owned()),
        merge_task:   false,
      },
      task_for_goal("merge", "g-merge", TaskStatus::Completed, true),
      TaskEntry {
        name:         "doc".to_owned(),
        goal_id:      Some("g-open".to_owned()),
        status:       TaskStatus::Pending,
        started_at:   None,
        run_slug:     None,
        current_step: None,
        merge_task:   false,
      },
    ],
    inquiries:     vec![
      InquiryEntry { kind: InquiryKind::OperationRequest, ..inquiry("q-open", false, time("2026-11-13T07:00:00Z")) },
      inquiry("q-answered", true, time("2026-11-13T07:00:00Z")),
    ],
    runs:          vec![recent_run("run-1")],
    notifications: vec![],
  };
  let harness =
    GrpcHarness::builder().single_project(project("takt-dash", project_dir()), server_meta(), json!({})).build();
  harness.store.record_success("takt-dash", snapshot, time("2026-11-13T12:00:00Z"));
  harness
}

#[tokio::test]
async fn get_overview_assembles_the_dashboard_state_from_the_store() {
  let harness = seeded_harness();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let state = client.get_overview(GetOverviewRequest {}).await.expect("概要を取得できます").into_inner();

  assert_eq!(
    state.read_at,
    Some(timestamp("2026-11-13T12:00:00Z")),
    "read_at は最後に読めたプロジェクトの last_read_at です"
  );
  assert_eq!(state.servers.len(), 1);
  assert_eq!(state.servers[0].server_id, "mac-mini");
  assert_eq!(state.servers[0].name, "j5ik2o-mac-mini");
  assert_eq!(state.servers[0].kind, "Mac mini");
  assert!(state.servers[0].connected, "読めていれば接続ありです");
  assert_eq!(state.servers[0].concurrency, 4);

  assert_eq!(state.projects.len(), 1);
  assert_eq!(state.projects[0].name, "takt-dash");
  assert_eq!(state.projects[0].server_id, "mac-mini");
  assert_eq!(state.projects[0].about, "takt-dash の説明");

  assert_eq!(state.goals.len(), 4);
  let paused = state.goals.iter().find(|goal| goal.goal_id == "g-pause").expect("一時停止のゴール");
  assert_eq!(paused.status, ProtoGoalStatus::Created as i32);
  assert_eq!(paused.execution_status, ProtoExecutionStatus::Paused as i32);
  assert_eq!(paused.paused_reason.as_deref(), Some("公開の日取りを待っています。"), "一時停止の理由を埋めます");
  let merge = state.goals.iter().find(|goal| goal.goal_id == "g-merge").expect("マージ待ちのゴール");
  assert_eq!(merge.status, ProtoGoalStatus::AwaitingMerge as i32);
  assert_eq!(merge.paused_reason, None, "理由のないゴールは paused_reason を設定しません");
  let pull_request = merge.pull_request.clone().expect("プルリクエスト");
  assert_eq!(pull_request.number, 42);
  assert_eq!(pull_request.url, "https://example.test/pr/42");
  assert_eq!(merge.awaiting_merge_at, Some(timestamp("2026-11-13T08:00:00Z")), "マージ待ちの起点時刻を写します");
  assert_eq!(merge.merge_task_last, Some(ProtoTaskStatus::Completed as i32), "仕上げタスクの最後の状態を写します");

  let running_goal = state.goals.iter().find(|goal| goal.goal_id == "g1").expect("進行中タスクのゴール");
  let running_counts = running_goal.task_counts.expect("タスクの状態ごとの件数");
  assert_eq!(running_counts.running, 1, "running のタスクをゴールの件数へ数えます");
  assert_eq!(running_counts.pending, 0);

  let open = state.goals.iter().find(|goal| goal.goal_id == "g-open").expect("登録済みのゴール");
  let counts = open.task_counts.expect("タスクの状態ごとの件数");
  assert_eq!(counts.pending, 1, "pending のタスクを数えます");
  assert_eq!(counts.running, 0);
  assert_eq!(counts.completed, 0);
  assert_eq!(open.merge_task_last, None, "仕上げタスクのないゴールは最後の状態を持ちません");

  assert_eq!(state.runs.len(), 1);
  let run = &state.runs[0];
  assert_eq!(run.run_slug, "run-1");
  assert_eq!(run.session_id, "sess-1", "セッションを写します");
  assert_eq!(run.server_id, "mac-mini");
  assert_eq!(run.project, "takt-dash");
  assert_eq!(run.goal_id, "g1");
  assert_eq!(run.task_name, "implement");
  assert_eq!(run.summary, "実装の run", "要約を写します");
  assert_eq!(run.status, RunStatus::Running as i32, "run の状態を写します");
  assert_eq!(run.steps.len(), 2);
  assert_eq!(run.steps[0].step_name, "plan");
  assert_eq!(run.steps[0].model, "opus");
  assert_eq!(run.steps[1].ended_at, None, "継続中のステップは終了時刻を持ちません");
  assert_eq!(run.steps[1].started_at, Some(timestamp("2026-11-13T09:05:00Z")));
  assert_eq!(run.current_step_started_at, Some(timestamp("2026-11-13T09:05:00Z")));
  assert_eq!(run.reports.len(), 1);
  assert_eq!(run.reports[0].filename, "plan.md");
  assert_eq!(run.reports[0].size_bytes, 1234);
  assert!(!run.log_silent, "最後のログの時刻は十分新しく、10分の無音ではありません");

  assert_eq!(
    state.thresholds,
    Some(AnomalyThresholds { pending_decision_stale_minutes: 240, run_log_silent_minutes: 10 })
  );

  server.stop().await;
}

#[tokio::test]
async fn get_overview_marks_a_run_silent_beyond_the_configured_threshold() {
  // 最後のログの時刻が現在より十分に古ければ、run のログが無音と判定されます。
  let harness =
    GrpcHarness::builder().single_project(project("takt-dash", project_dir()), server_meta(), json!({})).build();
  let mut stale_run = run_entry("run-1");
  stale_run.last_log_at = Some(Utc::now() - Duration::minutes(30));
  let snapshot = ProjectSnapshot {
    goals:         vec![],
    tasks:         vec![],
    inquiries:     vec![],
    runs:          vec![stale_run],
    notifications: vec![],
  };
  harness.store.record_success("takt-dash", snapshot, Utc::now());
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let state = client.get_overview(GetOverviewRequest {}).await.expect("概要を取得できます").into_inner();

  assert_eq!(state.runs.len(), 1);
  assert!(state.runs[0].log_silent, "10分の基準を超えた無音を flag します");

  server.stop().await;
}

#[tokio::test]
async fn get_overview_marks_a_disconnected_project_and_keeps_its_last_read_at() {
  // 応答がない間は connected が偽になり、read_at は最後に読めた時刻のままです。
  let harness = seeded_harness();
  harness.store.record_failure("takt-dash");
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let state = client.get_overview(GetOverviewRequest {}).await.expect("概要を取得できます").into_inner();

  assert!(!state.servers[0].connected, "読み取りに失敗していれば接続なしです");
  assert_eq!(state.read_at, Some(timestamp("2026-11-13T12:00:00Z")), "応答がない間も最後に読めた時刻を返します");

  server.stop().await;
}

#[tokio::test]
async fn get_pending_decisions_returns_only_unanswered_inquiries_and_human_merge_goals() {
  let harness = seeded_harness();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let state = client
    .get_pending_decisions(GetPendingDecisionsRequest {})
    .await
    .expect("判断待ちを取得できます")
    .into_inner()
    .state
    .expect("状態があります");

  let decisions = &state.pending_decisions;
  assert_eq!(
    decisions.len(),
    2,
    "未回答の問い合わせと人のマージ待ちだけを返します: {:?}",
    decisions.iter().map(|d| d.r#type).collect::<Vec<_>>()
  );
  let inquiry = decisions
    .iter()
    .find(|decision| decision.r#type == ProtoPendingDecisionType::Inquiry as i32)
    .expect("問い合わせの判断待ち");
  assert_eq!(inquiry.inquiry_kind, ProtoInquiryKind::OperationRequest as i32, "問い合わせの種類を写します");
  assert_eq!(inquiry.body, "問い合わせ q-open");
  assert_eq!(inquiry.options, vec!["はい".to_owned(), "いいえ".to_owned()]);
  assert_eq!(inquiry.goal_id, None, "ゴールに属さない問い合わせは goal_id を持ちません");
  assert_eq!(inquiry.pull_request, None);

  let merge = decisions
    .iter()
    .find(|decision| decision.r#type == ProtoPendingDecisionType::AwaitingMerge as i32)
    .expect("人のマージ待ちの判断待ち");
  assert_eq!(
    merge.inquiry_kind,
    ProtoInquiryKind::Unspecified as i32,
    "人のマージ待ちは inquiry_kind を INQUIRY_KIND_UNSPECIFIED に写します"
  );
  assert_eq!(merge.goal_id.as_deref(), Some("g-merge"));
  assert_eq!(merge.pull_request.as_ref().map(|pr| pr.number), Some(42));
  assert_eq!(merge.created_at, Some(timestamp("2026-11-13T08:00:00Z")), "マージ待ちの起点時刻を写します");
  assert!(merge.stale, "起点が 240 分より前なので滞留の判定済みフラグが立ちます");

  assert!(inquiry.stale, "未回答で 240 分より前なので滞留です");

  server.stop().await;
}

#[tokio::test]
async fn get_projects_returns_the_state_and_recent_events_newest_first() {
  let harness = seeded_harness();
  harness
    .activity
    .record(&event("2026-11-13T09:00:00Z", "mac-mini", Some("takt-dash"), EventKind::GoalCreated, "古い出来事"))
    .expect("出来事を記録できます");
  harness
    .activity
    .record(&event("2026-11-13T11:00:00Z", "mac-mini", Some("takt-dash"), EventKind::TaskCompleted, "新しい出来事"))
    .expect("出来事を記録できます");
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let response = client.get_projects(GetProjectsRequest {}).await.expect("プロジェクトを取得できます").into_inner();

  let state = response.state.expect("状態があります");
  assert_eq!(state.projects.len(), 1);
  assert_eq!(state.projects[0].name, "takt-dash");
  assert_eq!(state.goals.len(), 4, "プロジェクトの状態にゴールを含みます");

  let bodies: Vec<&str> = response.recent_events.iter().map(|event| event.body.as_str()).collect();
  assert_eq!(bodies, vec!["新しい出来事", "古い出来事"], "直近の出来事を新しい順に返します");
  assert_eq!(response.recent_events[0].kind, ActivityKind::TaskCompleted as i32);
  assert_eq!(response.recent_events[0].project.as_deref(), Some("takt-dash"));

  server.stop().await;
}

#[tokio::test]
async fn get_activity_returns_events_daily_summaries_and_today_counts() {
  let harness = seeded_harness();
  // 当日（JST の今日）に 2 件、24 時間前（JST の前日）に 1 件を記録する。当日判定は
  // JST の日付境界で行われるため、種の時刻を Utc::now() からの相対で作り、今日に確実に入れる。
  let now = Utc::now();
  harness
    .activity
    .record(&event(
      &(now - Duration::minutes(5)).to_rfc3339(),
      "mac-mini",
      Some("takt-dash"),
      EventKind::GoalCreated,
      "登録",
    ))
    .expect("出来事を記録できます");
  harness
    .activity
    .record(&event(
      &(now - Duration::minutes(10)).to_rfc3339(),
      "mac-mini",
      Some("takt-dash"),
      EventKind::Inquiry,
      "問い合わせ",
    ))
    .expect("出来事を記録できます");
  harness
    .activity
    .record(&event(&(now - Duration::hours(24)).to_rfc3339(), "mac-studio", Some("ddd"), EventKind::RunStarted, "開始"))
    .expect("出来事を記録できます");
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let response = client.get_activity(GetActivityRequest {}).await.expect("アクティビティを取得できます").into_inner();

  assert!(response.state.is_some(), "状態を同梱します");
  let event_bodies: Vec<&str> = response.events.iter().map(|event| event.body.as_str()).collect();
  assert_eq!(event_bodies, vec!["登録", "問い合わせ", "開始"], "出来事を新しい順に返します");
  assert_eq!(response.events[0].kind, ActivityKind::GoalCreated as i32);
  assert_eq!(response.events[2].server_id, "mac-studio");

  // 当日（JST）のサーバー別件数は、今日記録した mac-mini の 2 件だけを返す。
  let counts: Vec<(&str, i32)> =
    response.today_counts_by_server.iter().map(|count| (count.server_id.as_str(), count.count)).collect();
  assert_eq!(counts, vec![("mac-mini", 2)], "today_counts_by_server は当日のサーバー別件数を返します");

  assert_eq!(response.daily_summaries.len(), 7, "7日分の日次集計を返します");
  let dates: Vec<&str> = response.daily_summaries.iter().map(|summary| summary.date.as_str()).collect();
  let today = now.with_timezone(&chrono::FixedOffset::east_opt(9 * 3600).unwrap()).date_naive();
  let oldest = (today - Duration::days(6)).format("%Y-%m-%d").to_string();
  let newest = today.format("%Y-%m-%d").to_string();
  assert_eq!(dates.first().copied(), Some(oldest.as_str()), "古い日から並べます");
  assert_eq!(dates.last().copied(), Some(newest.as_str()), "最後が今日です");

  server.stop().await;
}

#[tokio::test]
async fn get_overview_counts_completed_tasks_in_the_last_24_hours_per_project() {
  // 直近24時間のタスク完了の出来事だけを数え、24時間より前の完了は数えない。
  let harness = seeded_harness();
  let now = Utc::now();
  harness
    .activity
    .record(&event(
      &(now - Duration::hours(1)).to_rfc3339(),
      "mac-mini",
      Some("takt-dash"),
      EventKind::TaskCompleted,
      "直近の完了",
    ))
    .expect("出来事を記録できます");
  harness
    .activity
    .record(&event(
      &(now - Duration::hours(3)).to_rfc3339(),
      "mac-mini",
      Some("takt-dash"),
      EventKind::TaskCompleted,
      "直近の完了2",
    ))
    .expect("出来事を記録できます");
  harness
    .activity
    .record(&event(
      &(now - Duration::hours(30)).to_rfc3339(),
      "mac-mini",
      Some("takt-dash"),
      EventKind::TaskCompleted,
      "24時間より前の完了",
    ))
    .expect("出来事を記録できます");
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let state = client.get_overview(GetOverviewRequest {}).await.expect("概要を取得できます").into_inner();

  let project = state.projects.iter().find(|project| project.name == "takt-dash").expect("プロジェクト");
  assert_eq!(project.completed_tasks_24h, 2, "直近24時間に完了したタスクだけを数えます");

  server.stop().await;
}

fn event(at: &str, server: &str, project: Option<&str>, kind: EventKind, body: &str) -> ActivityEvent {
  ActivityEvent {
    occurred_at: DateTime::parse_from_rfc3339(at).expect("時刻を解析できます").with_timezone(&Utc),
    server_id: server.to_owned(),
    project: project.map(str::to_owned),
    kind,
    body: body.to_owned(),
    run_slug: None,
  }
}

fn timestamp(text: &str) -> prost_types::Timestamp {
  let time = time(text);
  prost_types::Timestamp { seconds: time.timestamp(), nanos: time.timestamp_subsec_nanos() as i32 }
}

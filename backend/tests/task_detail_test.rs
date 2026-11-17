//! gRPC の GetTaskDetail が、mock MCP サーバの takt_list_tasks を全ページ取得し、
//! プロジェクトとタスク名（任意のゴール ID）でタスク詳細を組み立てることを検証する。

mod common;
mod grpc_harness;

use std::path::Path;

use common::*;
use grpc_harness::{client, start_server, GrpcHarness};
use serde_json::{json, Value};
use takt_dash_backend::takt_dash::v1::{GetTaskDetailRequest, TaskStatus as ProtoTaskStatus};

const TASK_COUNT: usize = 120;
const TARGET_INDEX: usize = 109;

fn tasks() -> Vec<Value> {
  let mut tasks = Vec::new();
  for index in 0..TASK_COUNT {
    let mut task = json!({
      "name": format!("task-{index}"),
      "goalId": "g1",
      "status": if index == TARGET_INDEX { "completed" } else { "pending" },
      "summary": format!("要約 {index}"),
      "startedAt": "2026-11-13T09:00:00Z",
      "runSlug": format!("run-{index}"),
    });
    if index == TARGET_INDEX {
      task["mergeTask"] = json!(true);
      task["prNumber"] = json!(42);
    }
    tasks.push(task);
  }
  tasks
}

fn task_harness() -> GrpcHarness {
  GrpcHarness::builder()
    .single_project(project("takt-dash", Path::new("/abs/takt-dash")), server_meta(), json!({ "tasks": tasks() }))
    .build()
}

fn task_harness_with_goal_summary(goal_summary: Value) -> GrpcHarness {
  GrpcHarness::builder()
    .single_project(
      project("takt-dash", Path::new("/abs/takt-dash")),
      server_meta(),
      json!({ "tasks": tasks(), "goalDetails": { "g1": { "summary": goal_summary } } }),
    )
    .build()
}

fn timestamp(text: &str) -> prost_types::Timestamp {
  let time = time(text);
  prost_types::Timestamp { seconds: time.timestamp(), nanos: time.timestamp_subsec_nanos() as i32 }
}

#[tokio::test]
async fn get_task_detail_collects_a_task_beyond_the_first_page() {
  let harness = task_harness();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let detail = client
    .get_task_detail(GetTaskDetailRequest {
      project:   "takt-dash".to_owned(),
      task_name: "task-109".to_owned(),
      goal_id:   Some("g1".to_owned()),
    })
    .await
    .expect("2 ページ目より後にあるタスクも取得できます")
    .into_inner();

  assert_eq!(detail.project, "takt-dash");
  assert_eq!(detail.task_name, "task-109");
  assert_eq!(detail.goal_id.as_deref(), Some("g1"));
  assert_eq!(detail.status, ProtoTaskStatus::Completed as i32);
  assert_eq!(detail.summary, "要約 109");
  assert_eq!(detail.started_at, Some(timestamp("2026-11-13T09:00:00Z")));
  assert_eq!(detail.run_slug.as_deref(), Some("run-109"));
  assert_eq!(detail.linked_run_slugs, vec!["run-109".to_owned()], "紐づく run の一覧を返します");
  assert_eq!(detail.pull_request_number, Some(42), "仕上げタスクのプルリクエスト番号を返します");

  server.stop().await;
}

#[tokio::test]
async fn get_task_detail_returns_no_linked_runs_when_the_task_has_no_run_slug() {
  let tasks = vec![json!({
    "name": "merge-task",
    "goalId": "g1",
    "status": "completed",
    "summary": "仕上げ",
  })];
  let harness = GrpcHarness::builder()
    .single_project(project("takt-dash", Path::new("/abs/takt-dash")), server_meta(), json!({ "tasks": tasks }))
    .build();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let detail = client
    .get_task_detail(GetTaskDetailRequest {
      project:   "takt-dash".to_owned(),
      task_name: "merge-task".to_owned(),
      goal_id:   Some("g1".to_owned()),
    })
    .await
    .expect("runSlug を持たないタスクも取得できます")
    .into_inner();

  assert_eq!(detail.task_name, "merge-task");
  assert!(detail.run_slug.is_none(), "runSlug を持たないタスクは run_slug を返しません");
  assert!(detail.linked_run_slugs.is_empty(), "runSlug を持たないタスクは紐づく run を返しません");

  server.stop().await;
}

#[tokio::test]
async fn get_task_detail_finds_a_task_without_a_goal_id() {
  let harness = task_harness();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let detail = client
    .get_task_detail(GetTaskDetailRequest {
      project:   "takt-dash".to_owned(),
      task_name: "task-109".to_owned(),
      goal_id:   None,
    })
    .await
    .expect("ゴール ID を指定しなくてもタスク名で取得できます")
    .into_inner();

  assert_eq!(detail.task_name, "task-109");
  assert_eq!(detail.goal_id.as_deref(), Some("g1"));

  server.stop().await;
}

#[tokio::test]
async fn get_task_detail_returns_not_found_when_the_goal_does_not_match() {
  let harness = task_harness();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let status = client
    .get_task_detail(GetTaskDetailRequest {
      project:   "takt-dash".to_owned(),
      task_name: "task-109".to_owned(),
      goal_id:   Some("g2".to_owned()),
    })
    .await
    .expect_err("ゴール ID が一致しないタスクは見つかりません");

  assert_eq!(status.code(), tonic::Code::NotFound, "ゴール不一致は Not Found です: {status:?}");

  server.stop().await;
}

#[tokio::test]
async fn get_task_detail_returns_not_found_for_an_unknown_task() {
  let harness = task_harness();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let status = client
    .get_task_detail(GetTaskDetailRequest {
      project:   "takt-dash".to_owned(),
      task_name: "does-not-exist".to_owned(),
      goal_id:   None,
    })
    .await
    .expect_err("存在しないタスクはエラーです");

  assert_eq!(status.code(), tonic::Code::NotFound, "未知のタスクは Not Found です: {status:?}");

  server.stop().await;
}

#[tokio::test]
async fn get_task_detail_returns_not_found_for_an_unknown_project() {
  let harness = task_harness();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let status = client
    .get_task_detail(GetTaskDetailRequest {
      project:   "does-not-exist".to_owned(),
      task_name: "task-109".to_owned(),
      goal_id:   None,
    })
    .await
    .expect_err("設定にないプロジェクトはエラーです");

  assert_eq!(status.code(), tonic::Code::NotFound, "未知のプロジェクトは Not Found です: {status:?}");

  server.stop().await;
}

#[tokio::test]
async fn get_task_detail_rejects_an_empty_project_as_invalid_argument() {
  let harness = task_harness();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let status = client
    .get_task_detail(GetTaskDetailRequest {
      project:   String::new(),
      task_name: "task-109".to_owned(),
      goal_id:   None,
    })
    .await
    .expect_err("空のプロジェクトは拒否します");

  assert_eq!(status.code(), tonic::Code::InvalidArgument, "空のプロジェクトは Invalid Argument です: {status:?}");

  server.stop().await;
}

#[tokio::test]
async fn get_task_detail_rejects_an_empty_task_name_as_invalid_argument() {
  let harness = task_harness();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let status = client
    .get_task_detail(GetTaskDetailRequest {
      project:   "takt-dash".to_owned(),
      task_name: String::new(),
      goal_id:   None,
    })
    .await
    .expect_err("空のタスク名は拒否します");

  assert_eq!(status.code(), tonic::Code::InvalidArgument, "空のタスク名は Invalid Argument です: {status:?}");

  server.stop().await;
}

#[tokio::test]
async fn get_task_detail_stops_following_pages_at_the_limit_instead_of_returning_a_partial_task() {
  let tasks: Vec<Value> = (0..6000)
    .map(|index| {
      json!({
        "name": format!("overflow-task-{index}"),
        "goalId": "g1",
        "status": "pending",
        "summary": format!("要約 {index}"),
      })
    })
    .collect();
  let harness = GrpcHarness::builder()
    .single_project(project("takt-dash", Path::new("/abs/takt-dash")), server_meta(), json!({ "tasks": tasks }))
    .build();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let status = client
    .get_task_detail(GetTaskDetailRequest {
      project:   "takt-dash".to_owned(),
      task_name: "overflow-task-5999".to_owned(),
      goal_id:   None,
    })
    .await
    .expect_err("ページ上限を超えた応答は部分結果ではなくエラーにします");

  assert_eq!(status.code(), tonic::Code::Internal, "ページ上限の超過は Internal です: {status:?}");

  server.stop().await;
}

#[tokio::test]
async fn get_task_detail_maps_a_takt_mcp_tool_error_to_unavailable() {
  let harness = GrpcHarness::builder()
    .single_project(
      project("takt-dash", Path::new("/abs/takt-dash")),
      server_meta(),
      json!({ "tasks": [], "isError": ["takt_list_tasks"] }),
    )
    .build();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let status = client
    .get_task_detail(GetTaskDetailRequest {
      project:   "takt-dash".to_owned(),
      task_name: "task-109".to_owned(),
      goal_id:   None,
    })
    .await
    .expect_err("道具エラーは失敗になります");

  assert_eq!(status.code(), tonic::Code::Unavailable, "takt-mcp の道具エラーは Unavailable です: {status:?}");

  server.stop().await;
}

#[tokio::test]
async fn get_task_detail_maps_the_goal_branch_and_pull_request_url() {
  let harness = task_harness_with_goal_summary(json!({
    "id": "g1",
    "branch": "takt/goal-g1",
    "pullRequest": {
      "number": 42,
      "url": "https://github.com/j5ik2o/takt-dash/pull/42",
    },
  }));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let detail = client
    .get_task_detail(GetTaskDetailRequest {
      project:   "takt-dash".to_owned(),
      task_name: "task-109".to_owned(),
      goal_id:   Some("g1".to_owned()),
    })
    .await
    .expect("ゴールに branch とプルリクエストを持つタスクも取得できます")
    .into_inner();

  assert_eq!(detail.branch.as_deref(), Some("takt/goal-g1"), "ゴールのブランチを返します");
  assert_eq!(
    detail.pull_request_url.as_deref(),
    Some("https://github.com/j5ik2o/takt-dash/pull/42"),
    "ゴールのプルリクエスト URL を返します"
  );
  assert_eq!(detail.project, "takt-dash");
  assert_eq!(detail.task_name, "task-109");
  assert_eq!(detail.goal_id.as_deref(), Some("g1"));
  assert_eq!(detail.status, ProtoTaskStatus::Completed as i32);
  assert_eq!(detail.summary, "要約 109");
  assert_eq!(detail.started_at, Some(timestamp("2026-11-13T09:00:00Z")));
  assert_eq!(detail.run_slug.as_deref(), Some("run-109"));
  assert_eq!(detail.linked_run_slugs, vec!["run-109".to_owned()]);
  assert_eq!(detail.pull_request_number, Some(42), "既存のプルリクエスト番号は意味を変えません");

  server.stop().await;
}

#[tokio::test]
async fn get_task_detail_leaves_branch_and_pull_request_url_unset_when_the_goal_has_none() {
  let harness = task_harness_with_goal_summary(json!({ "id": "g1" }));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let detail = client
    .get_task_detail(GetTaskDetailRequest {
      project:   "takt-dash".to_owned(),
      task_name: "task-109".to_owned(),
      goal_id:   Some("g1".to_owned()),
    })
    .await
    .expect("branch とプルリクエストを持たないゴールのタスクも取得できます")
    .into_inner();

  assert!(detail.branch.is_none(), "ゴールに branch が無ければ未設定です");
  assert!(detail.pull_request_url.is_none(), "ゴールにプルリクエスト URL が無ければ未設定です");

  server.stop().await;
}

#[tokio::test]
async fn get_task_detail_leaves_branch_and_pull_request_url_unset_when_the_task_has_no_goal_id() {
  let tasks = vec![json!({
    "name": "standalone-task",
    "status": "pending",
    "summary": "単独",
  })];
  let harness = GrpcHarness::builder()
    .single_project(
      project("takt-dash", Path::new("/abs/takt-dash")),
      server_meta(),
      json!({ "tasks": tasks, "isError": ["takt_get_goal"] }),
    )
    .build();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let detail = client
    .get_task_detail(GetTaskDetailRequest {
      project:   "takt-dash".to_owned(),
      task_name: "standalone-task".to_owned(),
      goal_id:   None,
    })
    .await
    .expect("ゴール ID を持たないタスクは takt_get_goal を呼びません")
    .into_inner();

  assert!(detail.goal_id.is_none());
  assert!(detail.branch.is_none(), "ゴール ID を持たないタスクは branch を未設定にします");
  assert!(detail.pull_request_url.is_none(), "ゴール ID を持たないタスクはプルリクエスト URL を未設定にします");

  server.stop().await;
}

#[tokio::test]
async fn get_task_detail_succeeds_for_a_task_with_an_empty_goal_id() {
  let tasks = vec![json!({
    "name": "standalone-task",
    "goalId": "",
    "status": "pending",
    "summary": "単独",
  })];
  let harness = GrpcHarness::builder()
    .single_project(
      project("takt-dash", Path::new("/abs/takt-dash")),
      server_meta(),
      json!({ "tasks": tasks, "isError": ["takt_get_goal"] }),
    )
    .build();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let detail = client
    .get_task_detail(GetTaskDetailRequest {
      project:   "takt-dash".to_owned(),
      task_name: "standalone-task".to_owned(),
      goal_id:   None,
    })
    .await
    .expect("goalId が空文字のタスクは takt_get_goal を呼ばずに取得できます")
    .into_inner();

  assert_eq!(detail.task_name, "standalone-task");
  assert_eq!(detail.status, ProtoTaskStatus::Pending as i32);
  assert!(detail.goal_id.is_none(), "goalId が空文字のタスクはゴール ID を未設定にします");
  assert!(detail.branch.is_none(), "goalId が空文字のタスクは branch を未設定にします");
  assert!(detail.pull_request_url.is_none(), "goalId が空文字のタスクはプルリクエスト URL を未設定にします");

  server.stop().await;
}

#[tokio::test]
async fn get_task_detail_maps_created_started_and_completed_times() {
  let tasks = vec![json!({
    "name": "time-task",
    "goalId": "g1",
    "status": "completed",
    "summary": "時刻を持つタスク",
    "createdAt": "2026-11-13T08:00:00Z",
    "startedAt": "2026-11-13T09:00:00Z",
    "completedAt": "2026-11-13T10:00:00Z",
  })];
  let harness = GrpcHarness::builder()
    .single_project(project("takt-dash", Path::new("/abs/takt-dash")), server_meta(), json!({ "tasks": tasks }))
    .build();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let detail = client
    .get_task_detail(GetTaskDetailRequest {
      project:   "takt-dash".to_owned(),
      task_name: "time-task".to_owned(),
      goal_id:   Some("g1".to_owned()),
    })
    .await
    .expect("作成・開始・完了の時刻を持つタスクも取得できます")
    .into_inner();

  assert_eq!(detail.created_at, Some(timestamp("2026-11-13T08:00:00Z")), "作成時刻を返します");
  assert_eq!(detail.started_at, Some(timestamp("2026-11-13T09:00:00Z")), "開始時刻を返します");
  assert_eq!(detail.completed_at, Some(timestamp("2026-11-13T10:00:00Z")), "完了時刻を返します");

  server.stop().await;
}

#[tokio::test]
async fn get_task_detail_leaves_created_and_completed_unset_when_the_task_has_none() {
  let tasks = vec![json!({
    "name": "running-task",
    "goalId": "g1",
    "status": "running",
    "summary": "実行中のタスク",
    "startedAt": "2026-11-13T09:00:00Z",
  })];
  let harness = GrpcHarness::builder()
    .single_project(project("takt-dash", Path::new("/abs/takt-dash")), server_meta(), json!({ "tasks": tasks }))
    .build();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let detail = client
    .get_task_detail(GetTaskDetailRequest {
      project:   "takt-dash".to_owned(),
      task_name: "running-task".to_owned(),
      goal_id:   Some("g1".to_owned()),
    })
    .await
    .expect("作成・完了の時刻を持たないタスクも取得できます")
    .into_inner();

  assert_eq!(detail.started_at, Some(timestamp("2026-11-13T09:00:00Z")), "開始時刻は返します");
  assert!(detail.created_at.is_none(), "作成時刻を持たないタスクは created_at を未設定にします");
  assert!(detail.completed_at.is_none(), "完了時刻を持たないタスクは completed_at を未設定にします");

  server.stop().await;
}

#[tokio::test]
async fn get_task_detail_lists_the_source_run_slug_after_the_run_slug() {
  let tasks = vec![json!({
    "name": "linked-task",
    "goalId": "g1",
    "status": "completed",
    "summary": "run を持つタスク",
    "runSlug": "run-main",
    "sourceRunSlug": "run-source",
  })];
  let harness = GrpcHarness::builder()
    .single_project(project("takt-dash", Path::new("/abs/takt-dash")), server_meta(), json!({ "tasks": tasks }))
    .build();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let detail = client
    .get_task_detail(GetTaskDetailRequest {
      project:   "takt-dash".to_owned(),
      task_name: "linked-task".to_owned(),
      goal_id:   Some("g1".to_owned()),
    })
    .await
    .expect("sourceRunSlug を持つタスクも取得できます")
    .into_inner();

  assert_eq!(detail.run_slug.as_deref(), Some("run-main"), "run_slug は従来の runSlug のまま返します");
  assert_eq!(
    detail.linked_run_slugs,
    vec!["run-main".to_owned(), "run-source".to_owned()],
    "runSlug と sourceRunSlug の両方を紐づく run として返します"
  );

  server.stop().await;
}

#[tokio::test]
async fn get_task_detail_lists_the_source_run_slug_when_the_task_has_no_run_slug() {
  let tasks = vec![json!({
    "name": "source-only-task",
    "goalId": "g1",
    "status": "completed",
    "summary": "sourceRunSlug だけを持つタスク",
    "sourceRunSlug": "run-source",
  })];
  let harness = GrpcHarness::builder()
    .single_project(project("takt-dash", Path::new("/abs/takt-dash")), server_meta(), json!({ "tasks": tasks }))
    .build();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let detail = client
    .get_task_detail(GetTaskDetailRequest {
      project:   "takt-dash".to_owned(),
      task_name: "source-only-task".to_owned(),
      goal_id:   Some("g1".to_owned()),
    })
    .await
    .expect("sourceRunSlug だけを持つタスクも取得できます")
    .into_inner();

  assert!(detail.run_slug.is_none(), "runSlug を持たないタスクは run_slug を返しません");
  assert_eq!(detail.linked_run_slugs, vec!["run-source".to_owned()], "sourceRunSlug を紐づく run として返します");

  server.stop().await;
}

#[tokio::test]
async fn get_task_detail_deduplicates_a_source_run_slug_equal_to_the_run_slug() {
  let tasks = vec![json!({
    "name": "same-run-task",
    "goalId": "g1",
    "status": "completed",
    "summary": "同じ slug を持つタスク",
    "runSlug": "run-same",
    "sourceRunSlug": "run-same",
  })];
  let harness = GrpcHarness::builder()
    .single_project(project("takt-dash", Path::new("/abs/takt-dash")), server_meta(), json!({ "tasks": tasks }))
    .build();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let detail = client
    .get_task_detail(GetTaskDetailRequest {
      project:   "takt-dash".to_owned(),
      task_name: "same-run-task".to_owned(),
      goal_id:   Some("g1".to_owned()),
    })
    .await
    .expect("sourceRunSlug と runSlug が同じタスクも取得できます")
    .into_inner();

  assert_eq!(detail.linked_run_slugs, vec!["run-same".to_owned()], "同じ slug は紐づく run に重複して含めません");

  server.stop().await;
}

#[tokio::test]
async fn get_task_detail_maps_a_goal_read_failure_to_unavailable() {
  let harness = GrpcHarness::builder()
    .single_project(
      project("takt-dash", Path::new("/abs/takt-dash")),
      server_meta(),
      json!({ "tasks": tasks(), "isError": ["takt_get_goal:g1"] }),
    )
    .build();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let status = client
    .get_task_detail(GetTaskDetailRequest {
      project:   "takt-dash".to_owned(),
      task_name: "task-109".to_owned(),
      goal_id:   Some("g1".to_owned()),
    })
    .await
    .expect_err("ゴールの読み取り失敗は空値ではなく失敗になります");

  assert_eq!(status.code(), tonic::Code::Unavailable, "ゴールの読み取り失敗は Unavailable です: {status:?}");

  server.stop().await;
}

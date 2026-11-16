//! gRPC の GetGoalDetail が、mock MCP サーバの takt_get_goal（path/textOffset）と
//! takt_list_goal_work_log・takt_list_goal_questions・takt_list_tasks の全ページから、
//! ゴール詳細を組み立てることを検証する。

mod common;
mod grpc_harness;

use std::path::Path;

use common::*;
use grpc_harness::{client, start_server, GrpcHarness};
use serde_json::{json, Value};
use takt_dash_backend::{
  model::{ExecutionStatus, GoalStatus, ProjectSnapshot},
  takt_dash::v1::{
    ExecutionStatus as ProtoExecutionStatus, GetGoalDetailRequest, GoalStatus as ProtoGoalStatus,
    TaskStatus as ProtoTaskStatus,
  },
};

const OBJECTIVE_LEN: usize = 5000;
const OUT_OF_SCOPE_COUNT: usize = 120;
const WORK_LOG_COUNT: usize = 55;
const QUESTION_COUNT: usize = 55;
const G1_TASK_COUNT: usize = 110;
const G2_TASK_COUNT: usize = 10;
const JAPANESE_OBJECTIVE_CHARS: usize = 3000;
const JAPANESE_GHERKIN_CHARS: usize = 2000;

fn objective() -> String {
  "o".repeat(OBJECTIVE_LEN)
}

fn japanese(chars: usize) -> String {
  "あ".repeat(chars)
}

fn minimal_goal_summary() -> Value {
  json!({
    "id": "g1",
    "status": "created",
    "executionStatus": "paused",
  })
}

fn gherkin() -> String {
  "g".repeat(5000)
}

fn out_of_scope() -> Vec<Value> {
  (0..OUT_OF_SCOPE_COUNT).map(|index| json!(format!("対象外 {index}"))).collect()
}

fn work_log() -> Vec<Value> {
  (0..WORK_LOG_COUNT)
    .map(|index| {
      json!({
        "id": format!("w{index}"),
        "recordedAt": format!("2026-11-13T09:{index:02}:00Z"),
        "summary": format!("記録 {index}"),
      })
    })
    .collect()
}

fn questions() -> Vec<Value> {
  (0..QUESTION_COUNT)
    .map(|index| {
      if index % 2 == 0 {
        json!({
          "id": format!("q{index}"),
          "kind": "question",
          "status": "answered",
          "body": format!("質問 {index}"),
          "options": ["はい", "いいえ"],
          "answer": {
            "text": format!("回答 {index}"),
            "decision": "allow",
            "report": "completed",
            "answeredAt": "2026-11-13T09:30:00Z",
          },
          "createdAt": "2026-11-13T09:00:00Z",
        })
      } else {
        json!({
          "id": format!("q{index}"),
          "kind": "question",
          "status": "pending",
          "body": format!("質問 {index}"),
          "options": ["はい", "いいえ"],
          "createdAt": "2026-11-13T09:00:00Z",
        })
      }
    })
    .collect()
}

fn tasks() -> Vec<Value> {
  let mut tasks = Vec::new();
  for index in 0..G1_TASK_COUNT {
    tasks.push(json!({
      "name": format!("g1-task-{index}"),
      "goalId": "g1",
      "status": if index == 0 { "running" } else { "completed" },
      "startedAt": "2026-11-13T09:00:00Z",
      "runSlug": format!("run-{index}"),
      "currentStep": "implement",
    }));
  }
  for index in 0..G2_TASK_COUNT {
    tasks.push(json!({ "name": format!("g2-task-{index}"), "goalId": "g2", "status": "completed" }));
  }
  tasks
}

fn goal_summary() -> Value {
  json!({
    "id": "g1",
    "status": "created",
    "executionStatus": "paused",
    "branch": "takt/goal-g1",
    "integrationBranch": "takt/integration",
    "omissions": {
      "specifications": {
        "gherkin": { "totalBytes": 5000 },
        "quint": { "totalBytes": 2 },
        "alloy": { "totalBytes": 2 },
      },
    },
  })
}

fn goal_detail_state() -> Value {
  json!({
    "goalDetails": {
      "g1": {
        "summary": goal_summary(),
        "objective": objective(),
        "outOfScope": out_of_scope(),
        "acceptanceCriteria": ["条件1", "条件2"],
        "constraints": ["制約1", "制約2"],
        "specifications": { "gherkin": gherkin(), "quint": "Q", "alloy": "A" },
        "workLog": work_log(),
        "questions": questions(),
      },
    },
    "tasks": tasks(),
  })
}

fn detail_harness(state: Value) -> GrpcHarness {
  let harness = GrpcHarness::builder()
    .single_project(project("takt-dash", Path::new("/abs/takt-dash")), server_meta(), state)
    .build();
  let snapshot = ProjectSnapshot {
    goals: vec![goal("g1", GoalStatus::Created, ExecutionStatus::Active)],
    ..ProjectSnapshot::empty()
  };
  harness.store.record_success("takt-dash", snapshot, time("2026-11-13T12:00:00Z"));
  harness
}

fn timestamp(text: &str) -> prost_types::Timestamp {
  let time = time(text);
  prost_types::Timestamp { seconds: time.timestamp(), nanos: time.timestamp_subsec_nanos() as i32 }
}

#[tokio::test]
async fn get_goal_detail_assembles_the_full_text_and_collections_from_the_mock() {
  let harness = detail_harness(goal_detail_state());
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let detail = client
    .get_goal_detail(GetGoalDetailRequest { goal_id: "g1".to_owned() })
    .await
    .expect("ゴール詳細を取得できます")
    .into_inner();

  assert_eq!(detail.goal_id, "g1");
  assert_eq!(detail.server_id, "mac-mini");
  assert_eq!(detail.project, "takt-dash");
  assert_eq!(detail.status, ProtoGoalStatus::Created as i32);
  assert_eq!(
    detail.execution_status,
    ProtoExecutionStatus::Paused as i32,
    "実行状態は要求時の takt_get_goal の現在値です"
  );
  assert_eq!(detail.branch, "takt/goal-g1");
  assert_eq!(detail.integration_branch, "takt/integration");

  assert_eq!(detail.objective.len(), OBJECTIVE_LEN, "目的はプレビューではなく全文です");
  assert!(detail.objective.starts_with('o'));

  assert_eq!(detail.out_of_scope.len(), OUT_OF_SCOPE_COUNT, "対象外は全ページを集めます");
  assert_eq!(detail.out_of_scope[0], "対象外 0");
  assert_eq!(detail.out_of_scope[OUT_OF_SCOPE_COUNT - 1], format!("対象外 {}", OUT_OF_SCOPE_COUNT - 1));
  assert_eq!(detail.acceptance_criteria, vec!["条件1".to_owned(), "条件2".to_owned()]);
  assert_eq!(detail.constraints, vec!["制約1".to_owned(), "制約2".to_owned()]);
  assert_eq!(
    detail.specifications.get("gherkin").map(String::as_str),
    Some(gherkin().as_str()),
    "4096 バイトを超える仕様も全文を集めます"
  );
  assert_eq!(detail.specifications.get("quint").map(String::as_str), Some("Q"));
  assert_eq!(detail.specifications.get("alloy").map(String::as_str), Some("A"));

  assert_eq!(detail.work_log.len(), WORK_LOG_COUNT, "作業記録は全ページを集めます");
  assert_eq!(detail.work_log[0].work_log_id, "w54", "作業記録は新しい順に並びます");
  assert_eq!(detail.work_log[0].summary, "記録 54");
  assert_eq!(detail.work_log[0].recorded_at, Some(timestamp("2026-11-13T09:54:00Z")));
  assert_eq!(detail.work_log[WORK_LOG_COUNT - 1].work_log_id, "w0");
  assert_eq!(detail.work_log[WORK_LOG_COUNT - 1].recorded_at, Some(timestamp("2026-11-13T09:00:00Z")));
  assert!(
    detail.work_log.windows(2).all(|pair| {
      let left = pair[0].recorded_at.expect("作業記録には recorded_at があります");
      let right = pair[1].recorded_at.expect("作業記録には recorded_at があります");
      (left.seconds, left.nanos) >= (right.seconds, right.nanos)
    }),
    "作業記録は記録時刻の降順です: {:?}",
    detail.work_log.iter().map(|entry| entry.work_log_id.as_str()).collect::<Vec<_>>()
  );

  assert_eq!(detail.questions.len(), QUESTION_COUNT, "質問は全ページを集めます");
  let answered = detail.questions.iter().find(|question| question.question_id == "q0").expect("回答済みの質問 q0");
  assert_eq!(answered.status, "answered");
  assert_eq!(answered.body, "質問 0");
  assert_eq!(answered.options, vec!["はい".to_owned(), "いいえ".to_owned()]);
  assert_eq!(answered.answer_text.as_deref(), Some("回答 0"));
  assert_eq!(answered.answer_decision.as_deref(), Some("allow"));
  assert_eq!(answered.answer_report.as_deref(), Some("completed"));
  assert_eq!(answered.answered_at, Some(timestamp("2026-11-13T09:30:00Z")));
  let pending = detail.questions.iter().find(|question| question.question_id == "q1").expect("未回答の質問 q1");
  assert_eq!(pending.status, "pending");
  assert!(pending.answer_text.is_none(), "未回答の質問は回答を持ちません");

  assert_eq!(detail.tasks.len(), G1_TASK_COUNT, "g1 のタスクだけを全ページから集めます");
  assert!(
    detail.tasks.iter().all(|task| task.task_name.starts_with("g1-task-")),
    "他ゴールのタスクを含めません: {:?}",
    detail.tasks
  );
  let first = detail.tasks.iter().find(|task| task.task_name == "g1-task-0").expect("g1-task-0");
  assert_eq!(first.status, ProtoTaskStatus::Running as i32);
  assert_eq!(first.run_slug.as_deref(), Some("run-0"));

  server.stop().await;
}

#[tokio::test]
async fn get_goal_detail_reads_short_specifications_that_are_not_listed_in_omissions() {
  // takt_get_goal は省略した大きい値だけを omissions に載せるため、omissions に現れない
  // 短い仕様も読み取る必要があります。omissions が gherkin だけを持つ状態で、quint と
  // alloy が結果に含まれることを観測します。
  let harness = detail_harness(json!({
    "goalDetails": {
      "g1": {
        "summary": {
          "id": "g1",
          "status": "created",
          "executionStatus": "paused",
          "omissions": { "specifications": { "gherkin": { "totalBytes": 5000 } } },
        },
        "objective": "",
        "outOfScope": [],
        "acceptanceCriteria": [],
        "constraints": [],
        "specifications": { "gherkin": gherkin(), "quint": "Q", "alloy": "A" },
        "workLog": [],
        "questions": [],
      },
    },
    "tasks": [],
  }));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let detail = client
    .get_goal_detail(GetGoalDetailRequest { goal_id: "g1".to_owned() })
    .await
    .expect("短い仕様を持つゴール詳細を取得できます")
    .into_inner();

  assert_eq!(
    detail.specifications.get("gherkin").map(String::as_str),
    Some(gherkin().as_str()),
    "omissions に載る長い仕様も全文を返します"
  );
  assert_eq!(
    detail.specifications.get("quint").map(String::as_str),
    Some("Q"),
    "omissions に載らない短い仕様も返します"
  );
  assert_eq!(
    detail.specifications.get("alloy").map(String::as_str),
    Some("A"),
    "omissions に載らない短い仕様も返します"
  );

  server.stop().await;
}

#[tokio::test]
async fn get_goal_detail_returns_not_found_for_an_unknown_goal_id() {
  let harness = detail_harness(json!({ "goalDetails": {}, "tasks": [] }));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let status = client
    .get_goal_detail(GetGoalDetailRequest { goal_id: "does-not-exist".to_owned() })
    .await
    .expect_err("状態ストアにないゴールはエラーです");

  assert_eq!(status.code(), tonic::Code::NotFound, "未知のゴールは Not Found です: {status:?}");

  server.stop().await;
}

#[tokio::test]
async fn get_goal_detail_rejects_an_empty_goal_id_as_invalid_argument() {
  let harness = detail_harness(goal_detail_state());
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let status = client
    .get_goal_detail(GetGoalDetailRequest { goal_id: String::new() })
    .await
    .expect_err("空のゴール ID は拒否します");

  assert_eq!(status.code(), tonic::Code::InvalidArgument, "空のゴール ID は Invalid Argument です: {status:?}");

  server.stop().await;
}

#[tokio::test]
async fn get_goal_detail_maps_a_takt_mcp_tool_error_to_unavailable() {
  let harness =
    detail_harness(json!({ "goalDetails": { "g1": { "summary": goal_summary() } }, "isError": ["takt_get_goal"] }));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let status = client
    .get_goal_detail(GetGoalDetailRequest { goal_id: "g1".to_owned() })
    .await
    .expect_err("道具エラーは失敗になります");

  assert_eq!(status.code(), tonic::Code::Unavailable, "takt-mcp の道具エラーは Unavailable です: {status:?}");

  server.stop().await;
}

#[tokio::test]
async fn get_goal_detail_maps_a_malformed_collection_response_to_internal() {
  let harness = detail_harness(json!({
    "goalDetails": {
      "g1": {
        "summary": goal_summary(),
        "objective": "目的",
        "outOfScope": 42,
      },
    },
  }));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let status = client
    .get_goal_detail(GetGoalDetailRequest { goal_id: "g1".to_owned() })
    .await
    .expect_err("配列でない対象外は解析できません");

  assert_eq!(status.code(), tonic::Code::Internal, "解析できない応答は Internal です: {status:?}");

  server.stop().await;
}

#[tokio::test]
async fn get_goal_detail_maps_a_malformed_text_response_to_internal() {
  let harness = detail_harness(json!({
    "goalDetails": {
      "g1": {
        "summary": minimal_goal_summary(),
        "malformedFields": { "objective": { "content": 42 } },
        "outOfScope": [],
        "acceptanceCriteria": [],
        "constraints": [],
        "workLog": [],
        "questions": [],
      },
    },
    "tasks": [],
  }));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let status = client
    .get_goal_detail(GetGoalDetailRequest { goal_id: "g1".to_owned() })
    .await
    .expect_err("文字列でない content は解析できません");

  assert_eq!(status.code(), tonic::Code::Internal, "解析できない応答は Internal です: {status:?}");

  server.stop().await;
}

#[tokio::test]
async fn get_goal_detail_returns_empty_values_for_explicitly_empty_optional_fields() {
  let harness = detail_harness(json!({
    "goalDetails": {
      "g1": {
        "summary": minimal_goal_summary(),
        "objective": "",
        "outOfScope": [],
        "acceptanceCriteria": [],
        "constraints": [],
        "specifications": {},
        "workLog": [],
        "questions": [],
      },
    },
    "tasks": [],
  }));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let detail = client
    .get_goal_detail(GetGoalDetailRequest { goal_id: "g1".to_owned() })
    .await
    .expect("空の任意項目を持つゴールでも取得できます")
    .into_inner();

  assert_eq!(detail.objective, "");
  assert!(detail.out_of_scope.is_empty(), "空の対象外は空配列です: {:?}", detail.out_of_scope);
  assert!(detail.acceptance_criteria.is_empty(), "空の受け入れ条件は空配列です");
  assert!(detail.constraints.is_empty(), "空の制約は空配列です");
  assert!(detail.specifications.is_empty(), "仕様が無ければ空です");

  server.stop().await;
}

#[tokio::test]
async fn get_goal_detail_treats_a_missing_optional_field_tool_error_as_empty() {
  let harness = detail_harness(json!({
    "goalDetails": {
      "g1": {
        "summary": minimal_goal_summary(),
        "workLog": [],
        "questions": [],
      },
    },
    "tasks": [],
  }));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let detail = client
    .get_goal_detail(GetGoalDetailRequest { goal_id: "g1".to_owned() })
    .await
    .expect("実 takt-mcp が項目不在を道具エラーで返しても取得できます")
    .into_inner();

  assert_eq!(detail.objective, "", "項目不在の目的は空文字です");
  assert!(detail.out_of_scope.is_empty(), "項目不在の対象外は空配列です");
  assert!(detail.acceptance_criteria.is_empty(), "項目不在の受け入れ条件は空配列です");
  assert!(detail.constraints.is_empty(), "項目不在の制約は空配列です");

  server.stop().await;
}

#[tokio::test]
async fn get_goal_detail_treats_an_omitted_optional_field_as_empty() {
  let harness = detail_harness(json!({
    "goalDetails": {
      "g1": {
        "summary": minimal_goal_summary(),
        "omitFields": ["objective", "outOfScope", "acceptanceCriteria", "constraints"],
        "workLog": [],
        "questions": [],
      },
    },
    "tasks": [],
  }));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let detail = client
    .get_goal_detail(GetGoalDetailRequest { goal_id: "g1".to_owned() })
    .await
    .expect("成功応答から content や items が省かれても取得できます")
    .into_inner();

  assert_eq!(detail.objective, "", "content が無い目的は空文字です");
  assert!(detail.out_of_scope.is_empty(), "items が無い対象外は空配列です");
  assert!(detail.acceptance_criteria.is_empty(), "items が無い受け入れ条件は空配列です");
  assert!(detail.constraints.is_empty(), "items が無い制約は空配列です");

  server.stop().await;
}

#[tokio::test]
async fn get_goal_detail_keeps_multibyte_text_intact_across_text_offset_pages() {
  let objective = japanese(JAPANESE_OBJECTIVE_CHARS);
  let gherkin = japanese(JAPANESE_GHERKIN_CHARS);
  assert!(objective.len() > 4096 * 2, "目的は複数ページにまたがる必要があります");
  assert!(gherkin.len() > 4096, "仕様は複数ページにまたがる必要があります");

  let harness = detail_harness(json!({
    "goalDetails": {
      "g1": {
        "summary": {
          "id": "g1",
          "status": "created",
          "executionStatus": "paused",
          "omissions": { "specifications": { "gherkin": { "totalBytes": gherkin.len() } } },
        },
        "objective": &objective,
        "outOfScope": [],
        "acceptanceCriteria": [],
        "constraints": [],
        "specifications": { "gherkin": &gherkin },
        "workLog": [],
        "questions": [],
      },
    },
    "tasks": [],
  }));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let detail = client
    .get_goal_detail(GetGoalDetailRequest { goal_id: "g1".to_owned() })
    .await
    .expect("日本語の長文も全ページを連結して取得できます")
    .into_inner();

  assert_eq!(detail.objective, objective, "ページ境界で日本語の目的が壊れてはいけません");
  assert!(!detail.objective.contains('\u{FFFD}'), "置換文字 U+FFFD を含めてはいけません");
  assert_eq!(
    detail.specifications.get("gherkin").map(String::as_str),
    Some(gherkin.as_str()),
    "ページ境界で日本語の仕様が壊れてはいけません"
  );
  assert!(
    !detail.specifications.get("gherkin").expect("gherkin の仕様").contains('\u{FFFD}'),
    "置換文字 U+FFFD を含めてはいけません"
  );

  server.stop().await;
}

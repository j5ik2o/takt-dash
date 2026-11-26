//! takt-mcp の応答の切り詰めの印を、backend が読み取りから proto・アクティビティまで運ぶことを
//! 検証する（issue #230、設計: docs/design/issue-230.md）。
//!
//! 印は文の形から推定せず、takt-mcp の応答の `omissions` だけから決めます。

mod common;
mod grpc_harness;

use std::{io::Write, path::Path};

use common::*;
use grpc_harness::{client, start_server, GrpcHarness};
use serde_json::json;
use takt_dash_backend::{
  events::detect,
  model::{EventKind, ExecutionStatus, GoalEntry, GoalStatus, InquiryEntry, PreviewText, ProjectSnapshot, TaskStatus},
  takt_dash::v1::{GetGoalsRequest, GetPendingDecisionsRequest, PendingDecisionType},
};

fn write_state(directory: &Path, state: serde_json::Value) -> std::path::PathBuf {
  let path = directory.join("state.json");
  let mut file = std::fs::File::create(&path).expect("状態ファイルを作れます");
  file.write_all(state.to_string().as_bytes()).expect("状態ファイルへ書けます");
  path
}

/// 実 takt-mcp の takt_list_goals の要素（goalSummary）の形。`total_bytes`
/// は元の目的のバイト数です。
fn listed_goal(id: &str, objective: &str, total_bytes: Option<usize>) -> serde_json::Value {
  let mut goal = json!({
    "id": id, "status": "awaiting_merge", "executionStatus": "active", "objective": objective,
    "integrationBranch": format!("takt/{id}"), "truncated": true,
  });
  if let Some(total_bytes) = total_bytes {
    goal["omissions"] = json!({ "objective": { "totalBytes": total_bytes } });
  }
  goal
}

async fn read(state: serde_json::Value) -> ProjectSnapshot {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(directory.path(), state);
  let mut client = mock_client(mock_config(directory.path(), &state, None));
  let snapshot = tool_set(&mut client, directory.path())
    .read_snapshot(None, &run_logs(directory.path()))
    .await
    .expect("状態を読み取れます");
  client.shutdown().await;
  snapshot
}

#[tokio::test]
async fn goal_objective_is_truncated_only_when_the_total_bytes_exceed_the_preview() {
  // QA で見つかった現象：目的が「…プルリク」で切れ、takt は元の文のバイト数を別の項目に持つ。
  let cut = "土台と完成申請の宛先は、プルリク";
  let exact = "あ".repeat(85) + "a"; // 256 バイトちょうどで、文末記号で終わらない。
  let snapshot = read(json!({
    "goals": [
      listed_goal("g1", cut, Some(cut.len() + 120)),
      listed_goal("g2", &exact, Some(exact.len())),
      listed_goal("g3", "印の無い古い takt-mcp の目的", None),
    ],
  }))
  .await;

  let objective = |id: &str| {
    snapshot.goals.iter().find(|goal| goal.goal_id.as_str() == id).expect("ゴールがあります").objective.clone()
  };
  assert_eq!(objective("g1"), PreviewText { text: cut.to_owned(), truncated: true }, "元の文が長いので切ってあります");
  assert_eq!(
    objective("g2"),
    PreviewText { text: exact.clone(), truncated: false },
    "256 バイトちょうどでも、元の文と同じ長さなら切っていません"
  );
  assert!(!objective("g3").truncated, "印が無ければ切ってあるとはみなしません");
}

#[tokio::test]
async fn page_omissions_mark_the_shortened_fields_of_listed_items() {
  // 64 KiB に収まらない 1 件だけのページは、最上位の omissions に `<配列名>[].<項目>` が付く。
  let snapshot = read(json!({
    "goals": [ listed_goal("g1", "目標", Some(6)), listed_goal("g2", "目標", Some(6)) ],
    "goalQuestions": {
      "g1": [ { "id": "q1", "status": "pending", "kind": "question", "body": "長い質問の先頭", "createdAt": "2026-11-13T09:00:00Z" } ],
      "g2": [ { "id": "q2", "status": "pending", "kind": "question", "body": "短い質問", "createdAt": "2026-11-13T09:00:00Z" } ]
    },
    "standaloneInquiries": [ { "id": "s1", "status": "pending", "kind": "question", "body": "長い問い合わせの先頭", "createdAt": "2026-11-13T09:30:00Z" } ],
    "notifications": { "g1": [ { "id": "n1", "kind": "blocked", "body": "長い通知の先頭", "recordedAt": "2026-11-13T09:45:00Z" } ] },
    "shortened": {
      "takt_list_goal_questions:g1": ["questions[].body"],
      "takt_list_goal_questions:g2": ["questions[].options"],
      "takt_list_standalone_inquiries": { "paths": [], "omittedDetails": 3 },
      "takt_get_goal:g1": ["items[].body"]
    }
  }))
  .await;

  let body = |id: &str| {
    snapshot
      .inquiries
      .iter()
      .find(|inquiry| inquiry.inquiry_id.as_str() == id)
      .expect("問い合わせがあります")
      .body
      .clone()
  };
  assert!(body("q1").truncated, "questions[].body が短くなったページの本文は切ってあります");
  assert!(!body("q2").truncated, "ほかの項目だけが短くなったページの本文は切っていません");
  assert!(body("s1").truncated, "書き切れない印（omittedDetails）があれば、本文を切ってあるとみなします");
  assert_eq!(body("q1").text, "長い質問の先頭", "本文そのものには省略記号を混ぜません");
  assert_eq!(snapshot.notifications[0].body, "長い通知の先頭…", "通知の本文は末尾に省略記号を付けます");
}

#[tokio::test]
async fn goal_detail_marks_shortened_questions_and_work_log() {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(
    directory.path(),
    json!({
      "goalDetails": { "g1": {
        "summary": { "id": "g1", "status": "created", "executionStatus": "active", "branch": "takt/g1", "integrationBranch": "main" },
        "objective": "目的",
        "workLog": [ { "id": "w1", "recordedAt": "2026-11-13T09:00:00Z", "summary": "長い作業記録の先頭" } ],
        "questions": [ { "id": "q1", "kind": "question", "status": "answered", "body": "長い質問の先頭", "answer": { "text": "短い回答" }, "createdAt": "2026-11-13T09:00:00Z" } ]
      } },
      "shortened": {
        "takt_list_goal_work_log:g1": ["workLog[].summary"],
        "takt_list_goal_questions:g1": ["questions[].body"]
      }
    }),
  );
  let mut client = mock_client(mock_config(directory.path(), &state, None));

  let detail = tool_set(&mut client, directory.path())
    .read_goal_detail(&takt_dash_backend::domain::GoalId::try_from("g1").expect("有効です"))
    .await
    .expect("詳細を読めます");
  client.shutdown().await;

  assert_eq!(detail.work_log[0].summary, "長い作業記録の先頭…");
  assert_eq!(detail.questions[0].body, "長い質問の先頭…");
  assert_eq!(detail.questions[0].answer_text.as_deref(), Some("短い回答"), "短くなっていない回答はそのままです");
}

fn truncated_goal(id: &str) -> GoalEntry {
  GoalEntry {
    objective: PreviewText { text: "土台と完成申請の宛先は、プルリク".to_owned(), truncated: true },
    ..merge_goal(id, 12)
  }
}

fn truncated_inquiry(id: &str) -> InquiryEntry {
  InquiryEntry {
    body: PreviewText { text: "長い問い合わせの先頭".to_owned(), truncated: true },
    ..inquiry(id, false, time("2026-11-13T09:00:00Z"))
  }
}

#[tokio::test]
async fn grpc_carries_the_marks_on_goals_and_pending_decisions() {
  let harness = GrpcHarness::builder()
    .single_project(project("takt-dash", Path::new("/abs/takt-dash")), server_meta(), json!({}))
    .build();
  let snapshot = ProjectSnapshot {
    goals: vec![truncated_goal("g1"), merge_goal("g2", 13)],
    tasks: vec![task_for_goal("merge", "g1", TaskStatus::Completed, true)],
    inquiries: vec![truncated_inquiry("q1"), inquiry("q2", false, time("2026-11-13T09:00:00Z"))],
    ..ProjectSnapshot::empty()
  };
  harness.store.record_success("takt-dash", snapshot, time("2026-11-13T12:00:00Z"));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let goals = client.get_goals(GetGoalsRequest {}).await.expect("ゴール一覧を取得できます").into_inner().goals;
  let g1 = goals.iter().find(|goal| goal.goal_id == "g1").expect("g1 があります");
  assert!(g1.name_truncated, "切ってある目的の印を運びます");
  assert_eq!(g1.name, "土台と完成申請の宛先は、プルリク", "name には省略記号を混ぜません");
  assert!(!goals.iter().find(|goal| goal.goal_id == "g2").expect("g2 があります").name_truncated);

  let decisions = client
    .get_pending_decisions(GetPendingDecisionsRequest {})
    .await
    .expect("判断待ちを取得できます")
    .into_inner()
    .state
    .expect("状態があります")
    .pending_decisions;
  let merge = |goal_id: &str| {
    decisions
      .iter()
      .find(|decision| {
        decision.r#type == PendingDecisionType::AwaitingMerge as i32 && decision.goal_id.as_deref() == Some(goal_id)
      })
      .expect("マージ待ちがあります")
  };
  assert!(merge("g1").body_truncated, "マージ待ちの本文は目的の印を運びます");
  assert_eq!(merge("g1").body, "土台と完成申請の宛先は、プルリク", "body には省略記号を混ぜません");
  assert!(!merge("g2").body_truncated);
  let inquiry = |inquiry_id: &str| {
    decisions.iter().find(|decision| decision.inquiry_id == inquiry_id).expect("問い合わせがあります")
  };
  assert!(inquiry("q1").body_truncated, "問い合わせの本文の印を運びます");
  assert!(!inquiry("q2").body_truncated);

  server.stop().await;
}

#[test]
fn activity_quotes_truncated_text_with_a_single_ellipsis() {
  let now = time("2026-11-13T10:00:00Z");
  let prev = snapshot(vec![], vec![], vec![], vec![], vec![]);
  let next = snapshot(
    vec![truncated_goal("g1"), goal("g2", GoalStatus::Created, ExecutionStatus::Active)],
    vec![],
    vec![truncated_inquiry("q1")],
    vec![],
    vec![],
  );

  let events = detect("mac-mini", "takt-dash", Some(&prev), &next, None, now);

  let bodies: Vec<&str> = events.iter().map(|event| event.body.as_str()).collect();
  assert!(
    bodies.contains(&"ゴール「土台と完成申請の宛先は、プルリク…」が登録されました。"),
    "切ってある目的は「」の中の末尾に省略記号を付けます: {bodies:?}"
  );
  assert!(bodies.contains(&"ゴール「ゴール g2」が登録されました。"), "切っていない目的はそのままです: {bodies:?}");
  assert!(
    bodies.contains(&"問い合わせ「長い問い合わせの先頭…」を受け付けました。"),
    "切ってある問い合わせの本文にも省略記号を付けます: {bodies:?}"
  );
  assert!(events.iter().any(|event| event.kind == EventKind::Inquiry));
  assert!(bodies.iter().all(|body| !body.contains("……")), "省略記号は 1 つだけです: {bodies:?}");
}

//! MCP のフィールド別省略情報を、判断待ちと保存済み活動の本文で観測する。

mod common;
mod grpc_harness;

use std::path::Path;

use common::{mock_client, mock_config, project, run_logs, server_meta, tool_set};
use grpc_harness::{client, start_server, GrpcHarness};
use serde_json::{json, Value};
use takt_dash_backend::{
  poller::Poller,
  takt_dash::v1::{GetActivityRequest, GetGoalDetailRequest, GetPendingDecisionsRequest},
};

async fn inquiry_and_activity(omissions: Value) -> (String, String) {
  let harness = GrpcHarness::builder()
    .single_project(project("takt-dash", Path::new("/abs/takt-dash")), server_meta(), json!({}))
    .build();
  let mut poller = Poller::new(
    harness.config_path().to_path_buf(),
    harness.activity.clone(),
    harness.store.clone(),
    harness.changes.clone(),
    harness.registry.clone(),
  );
  poller.read_cycle_once().await.expect("活動の比較元を読み取れます");
  harness.rewrite_state(json!({
    "responses": {
      "takt_list_standalone_inquiries": {
        "inquiries": [{
          "id": "q1", "kind": "question", "status": "pending", "body": "確認。",
          "options": ["はい"], "createdAt": "2026-11-19T00:00:00Z"
        }],
        "collection": "inquiries", "totalCount": 1, "returnedCount": 1,
        "omittedCount": 0, "offset": 0, "nextOffset": null,
        "truncated": true, "omissions": omissions
      }
    }
  }));
  poller.read_cycle_once().await.expect("質問を読み取れます");
  poller.shutdown().await;
  let server = start_server(&harness).await;
  let mut api = client(server.address).await;
  let decisions =
    api.get_pending_decisions(GetPendingDecisionsRequest {}).await.expect("判断待ちを取得できます").into_inner();
  let activity = api.get_activity(GetActivityRequest {}).await.expect("保存済み活動を取得できます").into_inner();
  server.stop().await;
  let state = decisions.state.expect("状態があります");
  assert_eq!(state.pending_decisions.len(), 1);
  let event = activity.events.iter().find(|event| event.body.contains("確認。")).expect("質問の活動があります");
  (state.pending_decisions[0].body.clone(), event.body.clone())
}

#[tokio::test]
async fn standalone_body_omission_is_visible_in_decisions_and_saved_activity() {
  let (body, event) = inquiry_and_activity(json!([
    { "path": "inquiries[].body", "totalBytes": 90, "returnedBytes": 9 }
  ]))
  .await;

  assert!(body.starts_with("確認。"));
  assert!(body.contains('…') || body.contains("省略"), "句点で終わる短い本文でも省略を明示します: {body}");
  assert!(event.contains(&body), "保存済み活動にも確定した質問本文を渡します: {event}");
}

#[tokio::test]
async fn options_omission_does_not_mark_the_question_body() {
  let (body, event) = inquiry_and_activity(json!([
    { "path": "inquiries[].options[]", "totalBytes": 90, "returnedBytes": 9 }
  ]))
  .await;

  assert_eq!(body, "確認。");
  assert_eq!(event, "問い合わせ「確認。」を受け付けました。");
}

#[tokio::test]
async fn complete_question_body_is_not_inferred_to_be_truncated() {
  let (body, event) = inquiry_and_activity(json!([])).await;

  assert_eq!(body, "確認。");
  assert_eq!(event, "問い合わせ「確認。」を受け付けました。");
}

#[tokio::test]
async fn goal_question_body_omission_reads_the_selected_record_to_the_last_page() {
  let full = format!("確認。{}末尾", "あ".repeat(3000));
  let question = json!({
    "id": "22222222-2222-4222-8222-222222222222", "kind": "question", "status": "pending", "body": full,
    "options": [], "createdAt": "2026-11-19T00:00:00Z"
  });
  let mut preview = question.clone();
  preview["body"] = json!("確認。");
  let harness = GrpcHarness::builder()
    .single_project(
      project("takt-dash", Path::new("/abs/takt-dash")),
      server_meta(),
      json!({
        "goals": [{ "id": "11111111-1111-4111-8111-111111111111", "status": "created", "executionStatus": "active", "objective": "目的" }],
        "goalDetails": { "11111111-1111-4111-8111-111111111111": {
          "summary": { "id": "11111111-1111-4111-8111-111111111111", "status": "created", "executionStatus": "active" },
          "objective": "目的", "questions": [question]
        } },
        "responses": { "takt_list_goal_questions": {
          "questions": [preview], "nextOffset": null, "truncated": true,
          "omissions": [{ "path": "questions[].body", "totalBytes": full.len(), "returnedBytes": 9 }]
        }}
      }),
    )
    .build();
  let mut poller = Poller::new(
    harness.config_path().to_path_buf(),
    harness.activity.clone(),
    harness.store.clone(),
    harness.changes.clone(),
    harness.registry.clone(),
  );
  poller.read_cycle_once().await.expect("省略された質問を読み取れます");
  poller.shutdown().await;
  let server = start_server(&harness).await;
  let response = client(server.address)
    .await
    .get_pending_decisions(GetPendingDecisionsRequest {})
    .await
    .expect("判断待ちを取得できます")
    .into_inner();
  let pending_calls = harness.recorded_calls();
  let detail = client(server.address)
    .await
    .get_goal_detail(GetGoalDetailRequest { goal_id: "11111111-1111-4111-8111-111111111111".to_owned() })
    .await
    .expect("ゴール詳細の質問を取得できます")
    .into_inner();
  server.stop().await;

  let state = response.state.expect("状態があります");
  assert_eq!(state.pending_decisions.len(), 1);
  assert_eq!(state.pending_decisions[0].body, full);
  assert_eq!(detail.questions[0].body, full);
  let calls = harness.recorded_calls();
  for stage in [&pending_calls[..], &calls[pending_calls.len()..]] {
    let pages: Vec<_> =
      stage.iter().filter(|call| call.name == "takt_get_goal" && call.arguments["recordId"].is_string()).collect();
    assert_eq!(pages.len(), 3);
    for (page, offset) in pages.iter().zip([0, 4095, 8190]) {
      assert_eq!(page.arguments["goalId"], "11111111-1111-4111-8111-111111111111");
      assert_eq!(page.arguments["recordId"], "22222222-2222-4222-8222-222222222222");
      assert_eq!(page.arguments["path"], "questions.body");
      assert_eq!(page.arguments["textOffset"], offset);
    }
  }
}

#[tokio::test]
async fn record_body_mock_rejects_each_invalid_uuid_before_response_overrides() {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = directory.path().join("state.json");
  std::fs::write(
    &state,
    json!({ "responses": { "takt_get_goal": {
      "content": "本文", "nextOffset": null
    }}})
    .to_string(),
  )
  .expect("状態を書けます");
  let mut mcp = mock_client(mock_config(directory.path(), &state, None));
  let mut cases: Vec<(String, bool)> = [
    ("11111111-1111-4111-8111-111111111111", true),
    ("33333333-3333-4333-8333-333333333333", true),
    ("g1", false),
    ("q1", false),
    ("n1", false),
    ("11111111-1111-4111-7111-111111111111", false),
    ("22222222-2222-4222-7222-222222222222", false),
    ("33333333-3333-4333-7333-333333333333", false),
    ("22222222-2222-9222-8222-222222222222", false),
    ("00000000-0000-0000-0000-000000000000", true),
    ("00000000-0000-0000-0000-000000000001", false),
    ("ffffffff-ffff-ffff-ffff-ffffffffffff", true),
    ("Ffffffff-ffff-ffff-ffff-ffffffffffff", false),
    ("ABCDEFAB-CDEF-4ABC-ABCD-ABCDEFABCDEF", true),
  ]
  .into_iter()
  .map(|(value, valid)| (value.to_owned(), valid))
  .collect();
  cases.extend((1..=8).map(|version| (format!("22222222-2222-{version}222-8222-222222222222"), true)));
  for (path, record_id) in [
    ("questions.body", "22222222-2222-4222-8222-222222222222"),
    ("notifications.body", "33333333-3333-4333-8333-333333333333"),
  ] {
    for field in ["goalId", "recordId"] {
      for (candidate, valid) in &cases {
        let mut arguments = json!({
          "cwd": directory.path(), "goalId": "11111111-1111-4111-8111-111111111111", "recordId": record_id,
          "path": path, "textOffset": 0
        });
        arguments[field] = json!(candidate);
        let result = mcp.call_tool("takt_get_goal", arguments).await;
        if *valid {
          let page: Value = serde_json::from_str(&result.expect("有効なUUIDで本文を読めます")).expect("JSONです");
          assert_eq!(page["content"], "本文");
        } else {
          assert!(
            matches!(
              result,
              Err(takt_dash_backend::mcp::CallError::Transport(takt_dash_backend::mcp::TransportError::Rpc {
                code: -32602,
                ..
              }))
            ),
            "{path} の {field}={candidate}: {result:?}"
          );
        }
      }
    }
  }
  mcp.shutdown().await;
}

#[tokio::test]
async fn standalone_pages_keep_both_questions_and_mark_only_the_omitted_body() {
  for field in ["inquiries[].body", "inquiries[].options[]"] {
    let question = |id| json!({ "id": id, "kind": "question", "status": "pending", "body": "確認。" });
    let harness = GrpcHarness::builder()
      .single_project(project("takt-dash", Path::new("/abs/takt-dash")), server_meta(), json!({}))
      .build();
    let mut poller = Poller::new(
      harness.config_path().to_path_buf(),
      harness.activity.clone(),
      harness.store.clone(),
      harness.changes.clone(),
      harness.registry.clone(),
    );
    poller.read_cycle_once().await.expect("比較元を読み取れます");
    harness.rewrite_state(json!({ "standalonePages": {
      "0:50": { "inquiries": [question("q1")], "nextOffset": 1,
        "omissions": [{ "path": field, "totalBytes": 90, "returnedBytes": 9 }] },
      "1:50": { "inquiries": [question("q2")], "nextOffset": null }
    }}));
    poller.read_cycle_once().await.expect("両ページを読み取れます");
    poller.shutdown().await;
    let server = start_server(&harness).await;
    let mut api = client(server.address).await;
    let decisions = api
      .get_pending_decisions(GetPendingDecisionsRequest {})
      .await
      .expect("判断待ちを取得できます")
      .into_inner()
      .state
      .expect("状態があります");
    let activity = api.get_activity(GetActivityRequest {}).await.expect("活動を取得できます").into_inner();
    server.stop().await;
    let first = if field == "inquiries[].body" { "確認。…" } else { "確認。" };
    assert_eq!(decisions.pending_decisions.len(), 2);
    for (id, expected) in [("q1", first), ("q2", "確認。")] {
      assert_eq!(
        decisions.pending_decisions.iter().find(|q| q.inquiry_id == id).expect("対象IDがあります").body,
        expected
      );
    }
    let events: Vec<_> = activity
      .events
      .iter()
      .filter(|event| event.kind == takt_dash_backend::takt_dash::v1::ActivityKind::Inquiry as i32)
      .collect();
    assert_eq!(events.len(), 2);
    let expected_count = if first == "確認。" { 2 } else { 1 };
    assert_eq!(
      events.iter().filter(|event| event.body == "問い合わせ「確認。」を受け付けました。").count(),
      expected_count
    );
    if first != "確認。" {
      assert_eq!(events.iter().filter(|event| event.body == "問い合わせ「確認。…」を受け付けました。").count(), 1);
    }
    let calls: Vec<_> =
      harness.recorded_calls().into_iter().filter(|call| call.name == "takt_list_standalone_inquiries").collect();
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[1].arguments["offset"], 0);
    assert_eq!(calls[2].arguments["offset"], 1);
    assert!(calls.iter().all(|call| call.arguments.get("limit").is_none()));
  }
}

#[tokio::test]
async fn failed_goal_body_restore_keeps_the_previous_question() {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let path = directory.path().join("state.json");
  std::fs::write(
    &path,
    json!({
      "goals": [{ "id": "11111111-1111-4111-8111-111111111111", "status": "created", "executionStatus": "active", "objective": "目的" }],
      "responses": { "takt_list_goal_questions": { "questions": [{
        "id": "22222222-2222-4222-8222-222222222222", "kind": "question", "status": "pending", "body": "確認。"
      }], "nextOffset": null, "omissions": [
        { "path": "questions[].body", "totalBytes": 90, "returnedBytes": 9 }
      ] }},
      "isError": ["takt_get_goal"]
    })
    .to_string(),
  )
  .expect("状態を書けます");
  let mut previous_question =
    common::inquiry("22222222-2222-4222-8222-222222222222", false, common::time("2026-11-19T00:00:00Z"));
  previous_question.goal_id =
    Some(takt_dash_backend::domain::GoalId::try_from("11111111-1111-4111-8111-111111111111").expect("有効なIDです"));
  let previous = takt_dash_backend::model::ProjectSnapshot {
    inquiries: vec![previous_question.clone()],
    ..takt_dash_backend::model::ProjectSnapshot::empty()
  };
  let mut client = mock_client(mock_config(directory.path(), &path, None));
  let snapshot = tool_set(&mut client, directory.path())
    .read_snapshot(Some(&previous), &run_logs(directory.path()))
    .await
    .expect("本文取得だけの失敗ではプロジェクトの読取を維持します");
  client.shutdown().await;

  assert_eq!(snapshot.inquiries, vec![previous_question]);
}

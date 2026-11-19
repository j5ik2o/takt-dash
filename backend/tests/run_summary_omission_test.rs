mod common;

use common::{mock_client, mock_config, tool_set};
use serde_json::{json, Value};
use takt_dash_backend::domain::RunSlug;

async fn remote_summary(fields: Value) -> String {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = directory.path().join("state.json");
  let mut run = json!({ "runSlug": "r1", "status": "running" });
  run
    .as_object_mut()
    .expect("run はオブジェクトです")
    .extend(fields.as_object().expect("追加項目はオブジェクトです").clone());
  std::fs::write(&state, json!({ "runs": { "r1": run } }).to_string()).expect("mock 状態を書けます");
  let mut client = mock_client(mock_config(directory.path(), &state, None));
  let run = tool_set(&mut client, directory.path())
    .read_remote_run(&RunSlug::try_from("r1").expect("有効な slug です"))
    .await
    .expect("リモートの run を読めます");
  client.shutdown().await;
  run.summary
}

#[tokio::test]
async fn task_omission_does_not_change_an_independent_complete_summary() {
  let summary = remote_summary(json!({
    "summary": "完了を確認。",
    "taskText": "あ".repeat(3000),
    "taskOmission": { "totalBytes": 9000, "returnedBytes": 0, "truncated": true }
  }))
  .await;

  assert_eq!(summary, "完了を確認。");
}

#[tokio::test]
async fn remote_task_without_summary_is_read_before_summarizing() {
  let task = format!("確認する。\n{}末尾", "あ".repeat(3000));
  let summary = remote_summary(json!({
    "taskText": task,
    "taskOmission": { "totalBytes": task.len(), "returnedBytes": 0, "truncated": true }
  }))
  .await;

  assert!(summary.starts_with("確認する。"), "view task から読んだ本文を要約します: {summary}");
  assert!(summary.ends_with('…'), "後続行を落としたことを表示します: {summary}");
}

#[tokio::test]
async fn remote_snapshot_keeps_summary_loading_separate_from_log_updates() {
  let task = format!("確認する。\n{}末尾", "あ".repeat(3000));
  for summary in [None, Some("完了を確認。")] {
    let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
    let state = directory.path().join("state.json");
    let mut run = json!({ "runSlug": "r1", "status": "running", "taskText": task,
      "taskOmission": { "truncated": true, "totalBytes": task.len(), "returnedBytes": 0 } });
    if let Some(summary) = summary {
      run["summary"] = json!(summary);
    }
    std::fs::write(
      &state,
      json!({
        "tasks": [{ "name": "implement", "status": "running", "runSlug": "r1" }],
        "runs": { "r1": run }
      })
      .to_string(),
    )
    .expect("状態を書けます");
    let mut client = mock_client(mock_config(directory.path(), &state, None));
    let snapshot = tool_set(&mut client, directory.path())
      .read_snapshot(None, &common::run_logs(directory.path()))
      .await
      .expect("一覧を読み取れます");
    client.shutdown().await;

    assert_eq!(snapshot.runs.len(), 1);
    assert_eq!(snapshot.runs[0].summary, summary.unwrap_or("確認する。…"));
  }
}

#[tokio::test]
async fn summary_field_omission_marks_even_a_short_sentence() {
  let summary = remote_summary(json!({
    "summary": "確認。",
    "omissions": [{ "path": "summary", "totalBytes": 90, "returnedBytes": 9 }]
  }))
  .await;

  assert!(summary.starts_with("確認。"));
  assert!(summary.contains('…') || summary.contains("省略"), "summary 自身の省略を明示します: {summary}");
}

#[tokio::test]
async fn remote_task_page_limit_does_not_return_partial_text_as_complete() {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = directory.path().join("state.json");
  let task = "あ".repeat(4096 * 100 / 3 + 2);
  std::fs::write(
    &state,
    json!({ "runs": { "r1": {
      "runSlug": "r1", "status": "running", "taskText": task
    }}})
    .to_string(),
  )
  .expect("状態を書けます");
  let mut client = mock_client(mock_config(directory.path(), &state, None));
  let result = tool_set(&mut client, directory.path())
    .read_remote_task_text(&RunSlug::try_from("r1").expect("有効なslugです"))
    .await;
  client.shutdown().await;

  assert!(matches!(result, Err(takt_dash_backend::mcp::ToolError::Parse(_))));
}

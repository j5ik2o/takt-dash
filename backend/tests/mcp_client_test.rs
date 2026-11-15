//! mock MCP サーバを使い、子プロセスの起動・ハンドシェイク・往復・isError・タイムアウト・
//! プロセス死亡からの再起動・応答の解析を結合的に検証する。

mod common;

use std::io::Write;

use common::*;
use serde_json::json;
use takt_dash_backend::{
  mcp::{CallError, TransportError},
  model::{GoalStatus, TaskStatus},
};
use tokio::time::timeout;

fn transport(error: CallError) -> TransportError {
  match error {
    | CallError::Transport(inner) => inner,
    | CallError::Tool(inner) => panic!("道具エラーではなく輸送エラーを期待しました: {inner}"),
  }
}

fn write_state(directory: &std::path::Path, state: serde_json::Value) -> std::path::PathBuf {
  let path = directory.join("state.json");
  let mut file = std::fs::File::create(&path).expect("状態ファイルを作れます");
  file.write_all(state.to_string().as_bytes()).expect("状態ファイルへ書けます");
  path
}

#[tokio::test]
async fn initializes_and_calls_a_tool_reading_response_text() {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(
    directory.path(),
    json!({ "goals": [ { "id": "g1", "status": "created", "executionStatus": "active", "objective": "目標", "integrationBranch": "takt/g1" } ] }),
  );
  let mut client = mock_client(mock_config(directory.path(), &state, None));

  let text = client.call_tool("takt_list_goals", with_cwd(directory.path(), json!({}))).await.expect("道具を呼べます");

  assert!(text.contains("\"goals\""), "応答の本文テキストを返します: {text}");
  client.shutdown().await;
}

#[tokio::test]
async fn spawn_failure_is_a_transport_error() {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(directory.path(), json!({}));
  let mut config = mock_config(directory.path(), &state, None);
  config.command = "this-command-does-not-exist".to_owned();
  let mut client = mock_client(config);

  let error = transport(
    client.call_tool("takt_list_goals", with_cwd(directory.path(), json!({}))).await.expect_err("起動失敗はエラーです"),
  );

  assert!(matches!(error, TransportError::Spawn(_) | TransportError::Io(_)), "起動失敗は輸送エラーです: {error}");
}

#[tokio::test]
async fn hanging_server_times_out() {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(directory.path(), json!({}));
  let mut client = mock_client(mock_config_with_timeout(directory.path(), &state, Some("hang"), 1));

  let error = transport(
    client
      .call_tool("takt_list_goals", with_cwd(directory.path(), json!({})))
      .await
      .expect_err("応答がなければタイムアウトします"),
  );

  assert!(matches!(error, TransportError::Timeout(_)), "タイムアウトとして扱います: {error}");
}

#[tokio::test]
async fn drip_feed_that_keeps_the_connection_busy_still_times_out_on_the_whole_call() {
  // 応答を細切れに返し続けるサーバーは、行単位のタイムアウトを満たし続ける。
  // 呼び出し全体の期限で打ち切る実装では Timeout になり、行単位だけで判定する実装では
  // 期限を超えても呼び出しが返らない。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(directory.path(), json!({}));
  let mut client = mock_client(mock_config_with_timeout(directory.path(), &state, Some("drip"), 1));

  let started = std::time::Instant::now();
  let error = transport(
    timeout(
      std::time::Duration::from_secs(10),
      client.call_tool("takt_list_goals", with_cwd(directory.path(), json!({}))),
    )
    .await
    .expect("全体の期限を超えたら呼び出しは打ち切られます")
    .expect_err("細切れの応答だけでは完了しません"),
  );

  assert!(matches!(error, TransportError::Timeout(_)), "全体の期限超過はタイムアウトとして扱います: {error}");
  assert!(
    started.elapsed() < std::time::Duration::from_secs(5),
    "呼び出し全体の期限で打ち切ります（経過: {:?}）",
    started.elapsed()
  );
}

#[tokio::test]
async fn invalid_response_is_a_transport_error() {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(directory.path(), json!({}));
  let mut client = mock_client(mock_config(directory.path(), &state, Some("garbage")));

  let error = transport(
    client
      .call_tool("takt_list_goals", with_cwd(directory.path(), json!({})))
      .await
      .expect_err("不正な応答はエラーです"),
  );

  assert!(matches!(error, TransportError::InvalidJson(_)), "不正な応答として扱います: {error}");
}

#[tokio::test]
async fn tool_error_is_distinguished_from_transport_error() {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(directory.path(), json!({ "isError": ["takt_list_tasks"] }));
  let mut client = mock_client(mock_config(directory.path(), &state, None));

  let error =
    client.call_tool("takt_list_tasks", with_cwd(directory.path(), json!({}))).await.expect_err("isError はエラーです");

  assert!(matches!(error, CallError::Tool(_)), "isError は道具エラーとして輸送エラーと区別します: {error}");
}

#[tokio::test]
async fn process_death_then_recovery_on_next_call() {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state_path = directory.path().join("state.json");
  // 状態ファイルを壊すと、mock は応答中にクラッシュして子プロセスが死亡する。
  std::fs::write(&state_path, "{ this is not json").expect("壊れた状態ファイルを書けます");
  let mut client = mock_client(mock_config(directory.path(), &state_path, None));

  let first = client.call_tool("takt_list_tasks", with_cwd(directory.path(), json!({}))).await;
  assert!(first.is_err(), "状態破損で死亡した子プロセスへの呼び出しは失敗します");

  // 状態を復元し、同じクライアントで再度呼び出す。輸送エラーで破棄された子プロセスを
  // 次の呼び出しで起動し直す再起動契約を直接観測する。
  std::fs::write(
    &state_path,
    json!({ "tasks": [ { "name": "a", "status": "running", "runSlug": "r1" } ] }).to_string(),
  )
  .expect("状態ファイルを復元できます");
  let text = client
    .call_tool("takt_list_tasks", with_cwd(directory.path(), json!({})))
    .await
    .expect("同じクライアントが子プロセスを再起動して道具を呼べます");
  assert!(text.contains("tasks"));
  client.shutdown().await;
}

#[tokio::test]
async fn tools_parse_goals_and_tasks_into_typed_snapshot() {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(
    directory.path(),
    json!({
      "goals": [ { "id": "g1", "status": "awaiting_merge", "executionStatus": "active", "objective": "目標", "integrationBranch": "takt/g1", "pullRequest": { "number": 12, "url": "https://example.test/pr/12" } } ],
      "tasks": [
        { "name": "a", "goalId": "g1", "status": "running", "startedAt": "2026-11-13T10:00:00Z", "runSlug": "r1", "currentStep": "implement" },
        { "name": "merge", "goalId": "g1", "status": "completed", "mergeTask": true }
      ],
      "runs": { "r1": { "runSlug": "r1", "currentStep": "implement", "startedAt": "2026-11-13T10:00:00Z", "stepLogs": [ { "step": "implement", "timestamp": "2026-11-13T10:05:00Z" } ] } }
    }),
  );
  let mut client = mock_client(mock_config(directory.path(), &state, None));

  let snapshot = tool_set(&mut client, directory.path()).read_snapshot(None).await.expect("状態を読み取れます");

  assert_eq!(snapshot.goals[0].status, GoalStatus::AwaitingMerge);
  assert_eq!(snapshot.goals[0].pull_request_number, Some(12));
  assert_eq!(snapshot.tasks[0].status, TaskStatus::Running);
  assert!(snapshot.tasks[1].merge_task, "仕上げタスクを読み取ります");
  assert_eq!(snapshot.runs.len(), 1);
  assert_eq!(snapshot.runs[0].last_log_at, time("2026-11-13T10:05:00Z"), "最後のログの時刻は末尾の stepLogs です");
  client.shutdown().await;
}

#[tokio::test]
async fn snapshot_collects_goal_questions_standalone_inquiries_and_notifications() {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(
    directory.path(),
    json!({
      "goals": [ { "id": "g1", "status": "awaiting_merge", "executionStatus": "active", "objective": "目標", "integrationBranch": "takt/g1" } ],
      "goalQuestions": { "g1": [ { "id": "q1", "status": "pending", "kind": "question", "body": "どうしますか", "options": ["はい"], "createdAt": "2026-11-13T09:00:00Z" } ] },
      "standaloneInquiries": [ { "id": "s1", "status": "pending", "kind": "operation_request", "body": "作業をお願いします", "createdAt": "2026-11-13T09:30:00Z" } ],
      "notifications": { "g1": [ { "id": "n1", "kind": "blocked", "body": "依存解決に失敗しました", "recordedAt": "2026-11-13T09:45:00Z" } ] }
    }),
  );
  let mut client = mock_client(mock_config(directory.path(), &state, None));

  let snapshot = tool_set(&mut client, directory.path()).read_snapshot(None).await.expect("状態を読み取れます");

  let inquiry_ids: Vec<&str> = snapshot.inquiries.iter().map(|inquiry| inquiry.inquiry_id.as_str()).collect();
  assert!(inquiry_ids.contains(&"q1"), "ゴールの問い合わせを含みます");
  assert!(inquiry_ids.contains(&"s1"), "ゴールに属さない問い合わせを含みます");
  assert_eq!(snapshot.notifications[0].kind, "blocked");
  client.shutdown().await;
}

#[tokio::test]
async fn awaiting_merge_at_comes_from_the_awaiting_merge_notification_recorded_at() {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(
    directory.path(),
    json!({
      "goals": [ { "id": "g1", "status": "awaiting_merge", "executionStatus": "active", "objective": "目標", "integrationBranch": "takt/g1" } ],
      "tasks": [ { "name": "merge", "goalId": "g1", "status": "completed", "mergeTask": true } ],
      "notifications": { "g1": [
        { "id": "n-progress", "kind": "progress", "body": "進捗", "recordedAt": "2026-11-13T08:00:00Z" },
        { "id": "n-merge-old", "kind": "awaiting_merge", "body": "古い", "recordedAt": "2026-11-13T08:30:00Z" },
        { "id": "n-merge-new", "kind": "awaiting_merge", "body": "新しい", "recordedAt": "2026-11-13T09:00:00Z" }
      ] }
    }),
  );
  let mut client = mock_client(mock_config(directory.path(), &state, None));

  let snapshot = tool_set(&mut client, directory.path()).read_snapshot(None).await.expect("状態を読み取れます");

  assert_eq!(
    snapshot.goals[0].awaiting_merge_at,
    Some(time("2026-11-13T09:00:00Z")),
    "マージ待ちの起点は最も新しい awaiting_merge 通知の recordedAt です"
  );
  client.shutdown().await;
}

#[tokio::test]
async fn goal_without_awaiting_merge_notification_keeps_no_wait_start() {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(
    directory.path(),
    json!({
      "goals": [ { "id": "g1", "status": "awaiting_merge", "executionStatus": "active", "objective": "目標", "integrationBranch": "takt/g1" } ],
      "notifications": { "g1": [ { "id": "n-progress", "kind": "progress", "body": "進捗", "recordedAt": "2026-11-13T08:00:00Z" } ] }
    }),
  );
  let mut client = mock_client(mock_config(directory.path(), &state, None));

  let snapshot = tool_set(&mut client, directory.path()).read_snapshot(None).await.expect("状態を読み取れます");

  assert_eq!(snapshot.goals[0].awaiting_merge_at, None, "awaiting_merge 通知がないゴールは起点を持ちません");
  client.shutdown().await;
}

#[tokio::test]
async fn goal_questions_and_notifications_beyond_the_first_page_are_collected() {
  // 60 ゴールそれぞれに1件の質問を持たせ、ページサイズ 50 を超えて 2 ページ目に回る質問を集める。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let goals: Vec<serde_json::Value> = (0..60)
    .map(|index| json!({ "id": format!("g{index}"), "status": "created", "executionStatus": "active", "objective": format!("目標 {index}"), "integrationBranch": format!("takt/g{index}") }))
    .collect();
  let mut questions = serde_json::Map::new();
  for index in 0..60 {
    questions.insert(
      format!("g{index}"),
      json!([ { "id": format!("q{index}"), "status": "pending", "kind": "question", "body": format!("質問 {index}"), "createdAt": "2026-11-13T09:00:00Z" } ]),
    );
  }
  // 51 件目の通知を awaiting_merge にして、2 ページ目から起点を採れることを観測する。
  let mut notifications = serde_json::Map::new();
  let mut list = Vec::new();
  for index in 0..50 {
    list.push(
      json!({ "id": format!("n{index}"), "kind": "progress", "body": "進捗", "recordedAt": "2026-11-13T08:00:00Z" }),
    );
  }
  list.push(
    json!({ "id": "n-merge", "kind": "awaiting_merge", "body": "マージ待ち", "recordedAt": "2026-11-13T07:00:00Z" }),
  );
  notifications.insert("g0".to_owned(), serde_json::Value::Array(list));
  let state = write_state(
    directory.path(),
    json!({ "goals": goals, "goalQuestions": serde_json::Value::Object(questions), "notifications": serde_json::Value::Object(notifications) }),
  );
  let mut client = mock_client(mock_config(directory.path(), &state, None));

  let snapshot = tool_set(&mut client, directory.path()).read_snapshot(None).await.expect("状態を読み取れます");

  assert_eq!(snapshot.goals.len(), 60, "50 件を超えるゴールも全件集めます");
  assert_eq!(snapshot.inquiries.len(), 60, "50 件を超えるゴール質問も全件集めます");
  let last = snapshot.inquiries.iter().find(|inquiry| inquiry.inquiry_id == "q59").expect("2 ページ目の質問 g59");
  assert_eq!(last.goal_id.as_deref(), Some("g59"));
  assert_eq!(
    snapshot.goals.iter().find(|goal| goal.goal_id == "g0").and_then(|goal| goal.awaiting_merge_at),
    Some(time("2026-11-13T07:00:00Z")),
    "2 ページ目の awaiting_merge 通知の recordedAt を起点に採ります"
  );
  client.shutdown().await;
}

#[tokio::test]
async fn calling_a_tool_without_cwd_is_rejected_by_the_mock_as_invalid_params() {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(directory.path(), json!({ "goals": [] }));
  let mut client = mock_client(mock_config(directory.path(), &state, None));

  // cwd を欠いた引数は実契約と同じく拒否され、輸送エラーとして失敗する。
  let error =
    transport(client.call_tool("takt_list_goals", json!({ "offset": 0 })).await.expect_err("cwd がないと拒否されます"));
  assert!(matches!(error, TransportError::Rpc { .. }), "不正な引数は JSON-RPC エラーになります: {error}");
  client.shutdown().await;
}

#[tokio::test]
async fn inquiry_without_created_at_is_kept_with_first_observation_time() {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(
    directory.path(),
    json!({
      "goals": [ { "id": "g1", "status": "created", "executionStatus": "active", "objective": "目標", "integrationBranch": "takt/g1" } ],
      "goalQuestions": { "g1": [ { "id": "q-x", "status": "pending", "kind": "question", "body": "どうしますか" } ] }
    }),
  );
  let mut client = mock_client(mock_config(directory.path(), &state, None));

  let snapshot =
    tool_set(&mut client, directory.path()).read_snapshot(None).await.expect("createdAt 欠落でも解析できます");

  assert!(
    snapshot.inquiries.iter().any(|inquiry| inquiry.inquiry_id == "q-x"),
    "createdAt のない質問を判断待ちに含めます"
  );
  client.shutdown().await;
}

#[tokio::test]
async fn inquiry_without_created_at_inherits_previous_created_at() {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(
    directory.path(),
    json!({
      "goals": [ { "id": "g1", "status": "created", "executionStatus": "active", "objective": "目標", "integrationBranch": "takt/g1" } ],
      "goalQuestions": { "g1": [ { "id": "q-x", "status": "pending", "kind": "question", "body": "どうしますか" } ] }
    }),
  );
  let mut client = mock_client(mock_config(directory.path(), &state, None));
  let prev = takt_dash_backend::model::ProjectSnapshot {
    inquiries: vec![takt_dash_backend::model::InquiryEntry {
      inquiry_id: "q-x".to_owned(),
      goal_id:    Some("g1".to_owned()),
      kind:       takt_dash_backend::model::InquiryKind::Question,
      body:       "どうしますか".to_owned(),
      options:    Vec::new(),
      answered:   false,
      created_at: time("2026-11-13T01:00:00Z"),
    }],
    ..takt_dash_backend::model::ProjectSnapshot::empty()
  };

  let snapshot =
    tool_set(&mut client, directory.path()).read_snapshot(Some(&prev)).await.expect("前回値を引き継げます");

  let inquiry = snapshot.inquiries.iter().find(|entry| entry.inquiry_id == "q-x").expect("q-x を含みます");
  assert_eq!(inquiry.created_at, time("2026-11-13T01:00:00Z"), "createdAt 欠落は前回の created_at を引き継ぎます");
  client.shutdown().await;
}

#[tokio::test]
async fn approval_kind_is_skipped_and_unknown_kind_is_rejected() {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(
    directory.path(),
    json!({
      "goals": [ { "id": "g1", "status": "created", "executionStatus": "active", "objective": "目標", "integrationBranch": "takt/g1" } ],
      "goalQuestions": { "g1": [
        { "id": "q-a", "status": "pending", "kind": "approval", "body": "承認", "createdAt": "2026-11-13T09:00:00Z" },
        { "id": "q-q", "status": "pending", "kind": "question", "body": "質問", "createdAt": "2026-11-13T09:00:00Z" }
      ] }
    }),
  );
  let mut client = mock_client(mock_config(directory.path(), &state, None));
  let snapshot = tool_set(&mut client, directory.path()).read_snapshot(None).await.expect("approval を読み飛ばせます");
  assert!(snapshot.inquiries.iter().all(|entry| entry.inquiry_id != "q-a"), "approval は判断待ちに含めません");
  assert!(snapshot.inquiries.iter().any(|entry| entry.inquiry_id == "q-q"), "question は含めます");
  client.shutdown().await;

  let directory2 = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state2 = write_state(
    directory2.path(),
    json!({
      "goals": [ { "id": "g1", "status": "created", "executionStatus": "active", "objective": "目標", "integrationBranch": "takt/g1" } ],
      "goalQuestions": { "g1": [ { "id": "q-u", "status": "pending", "kind": "unknown_kind", "body": "不明", "createdAt": "2026-11-13T09:00:00Z" } ] }
    }),
  );
  let mut client2 = mock_client(mock_config(directory2.path(), &state2, None));
  let error = tool_set(&mut client2, directory2.path()).read_snapshot(None).await.expect_err("未知の種類は拒否します");
  assert!(matches!(error, takt_dash_backend::mcp::ToolError::Parse(_)), "未知の kind は解析エラーになります: {error}");
  client2.shutdown().await;
}

#[tokio::test]
async fn run_with_more_than_one_page_of_step_logs_uses_the_last_step_time() {
  // 51 ステップの run。先頭ページは 50 件で、51 件目は stepLogPage.nextOffset の 2 ページ目に回る。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let mut step_logs = Vec::new();
  for index in 0..51 {
    let minute = index;
    step_logs.push(json!({ "step": format!("step{index}"), "timestamp": format!("2026-11-13T10:{minute:02}:00Z") }));
  }
  let state = write_state(
    directory.path(),
    json!({
      "tasks": [ { "name": "a", "status": "running", "runSlug": "r1", "startedAt": "2026-11-13T10:00:00Z" } ],
      "runs": { "r1": { "runSlug": "r1", "currentStep": "step50", "startedAt": "2026-11-13T10:00:00Z", "stepLogs": step_logs } }
    }),
  );
  let mut client = mock_client(mock_config(directory.path(), &state, None));

  let snapshot = tool_set(&mut client, directory.path()).read_snapshot(None).await.expect("状態を読み取れます");

  let run = &snapshot.runs[0];
  assert_eq!(run.steps.len(), 51, "2 ページ目のステップも集めます");
  assert_eq!(run.last_log_at, time("2026-11-13T10:50:00Z"), "最後のログの時刻は末尾ページの最終ステップです");
  client.shutdown().await;
}

#[tokio::test]
async fn is_error_on_get_run_keeps_previous_runs_without_transport_failure() {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(
    directory.path(),
    json!({
      "tasks": [ { "name": "a", "status": "running", "runSlug": "r1", "startedAt": "2026-11-13T10:00:00Z" } ],
      "isError": ["takt_get_run"]
    }),
  );
  let mut client = mock_client(mock_config(directory.path(), &state, None));
  let prev = takt_dash_backend::model::ProjectSnapshot {
    runs: vec![takt_dash_backend::model::RunEntry {
      run_slug:                "r1".to_owned(),
      session_id:              String::new(),
      task_name:               "a".to_owned(),
      goal_id:                 None,
      summary:                 String::new(),
      status:                  takt_dash_backend::model::RunStatus::Running,
      current_step:            Some("implement".to_owned()),
      started_at:              time("2026-11-13T10:00:00Z"),
      current_step_started_at: None,
      last_log_at:             time("2026-11-13T10:05:00Z"),
      steps:                   vec![takt_dash_backend::model::RunStepEntry {
        step_name:  "implement".to_owned(),
        model:      String::new(),
        started_at: time("2026-11-13T10:05:00Z"),
        ended_at:   None,
      }],
      reports:                 Vec::new(),
    }],
    ..takt_dash_backend::model::ProjectSnapshot::empty()
  };

  let snapshot = tool_set(&mut client, directory.path())
    .read_snapshot(Some(&prev))
    .await
    .expect("get_run の isError で全体は失敗しません");

  assert_eq!(snapshot.runs.len(), 1, "get_run の道具エラー時は前回の run を保持します");
  assert_eq!(snapshot.runs[0].last_log_at, time("2026-11-13T10:05:00Z"));
  client.shutdown().await;
}

#[tokio::test]
async fn one_run_tool_error_keeps_only_that_run_previous_and_updates_the_healthy_sibling() {
  // 2 つの running run のうち runA だけを isError にし、runB は新しい stepLogs を持つ。
  // runA は前回値、runB は今回値（新しい last_log_at）になることを観測する。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(
    directory.path(),
    json!({
      "tasks": [
        { "name": "a", "status": "running", "runSlug": "runA", "startedAt": "2026-11-13T10:00:00Z" },
        { "name": "b", "status": "running", "runSlug": "runB", "startedAt": "2026-11-13T10:00:00Z" }
      ],
      "runs": { "runB": { "runSlug": "runB", "currentStep": "implement", "startedAt": "2026-11-13T10:00:00Z", "stepLogs": [ { "step": "implement", "timestamp": "2026-11-13T10:20:00Z" } ] } },
      "isError": ["takt_get_run:runA"]
    }),
  );
  let mut client = mock_client(mock_config(directory.path(), &state, None));
  let prev = takt_dash_backend::model::ProjectSnapshot {
    runs: vec![takt_dash_backend::model::RunEntry {
      run_slug:                "runA".to_owned(),
      session_id:              String::new(),
      task_name:               "a".to_owned(),
      goal_id:                 None,
      summary:                 String::new(),
      status:                  takt_dash_backend::model::RunStatus::Running,
      current_step:            Some("implement".to_owned()),
      started_at:              time("2026-11-13T10:00:00Z"),
      current_step_started_at: None,
      last_log_at:             time("2026-11-13T10:05:00Z"),
      steps:                   vec![takt_dash_backend::model::RunStepEntry {
        step_name:  "implement".to_owned(),
        model:      String::new(),
        started_at: time("2026-11-13T10:05:00Z"),
        ended_at:   None,
      }],
      reports:                 Vec::new(),
    }],
    ..takt_dash_backend::model::ProjectSnapshot::empty()
  };

  let snapshot = tool_set(&mut client, directory.path())
    .read_snapshot(Some(&prev))
    .await
    .expect("一方の run の道具エラーで全体は失敗しません");

  assert_eq!(snapshot.runs.len(), 2, "エラー run は前回値で残り、健常 run も含めて 2 件です");
  let run_a = snapshot.runs.iter().find(|run| run.run_slug == "runA").expect("runA が残ります");
  assert_eq!(run_a.last_log_at, time("2026-11-13T10:05:00Z"), "道具エラーの runA は前回値です");
  let run_b = snapshot.runs.iter().find(|run| run.run_slug == "runB").expect("runB が残ります");
  assert_eq!(run_b.last_log_at, time("2026-11-13T10:20:00Z"), "健常な runB は今回値へ更新されます");
  client.shutdown().await;
}

#[tokio::test]
async fn goal_question_tool_error_inherits_only_that_goal_and_leaves_the_healthy_goal_current() {
  // 2 ゴールのうち g1 の takt_list_goal_questions だけを isError にする。
  // g1 の質問 q1 は前回値で引き継ぎ、健常な g2 の質問 q2 は今回値を採る。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(
    directory.path(),
    json!({
      "goals": [
        { "id": "g1", "status": "created", "executionStatus": "active", "objective": "ゴール g1", "integrationBranch": "takt/g1" },
        { "id": "g2", "status": "created", "executionStatus": "active", "objective": "ゴール g2", "integrationBranch": "takt/g2" }
      ],
      "goalQuestions": {
        "g1": [ { "id": "q1", "status": "pending", "kind": "question", "body": "g1 の質問", "createdAt": "2026-11-13T09:00:00Z" } ],
        "g2": [ { "id": "q2", "status": "pending", "kind": "question", "body": "g2 の質問", "createdAt": "2026-11-13T09:00:00Z" } ]
      },
      "isError": ["takt_list_goal_questions:g1"]
    }),
  );
  let mut client = mock_client(mock_config(directory.path(), &state, None));
  let prev = takt_dash_backend::model::ProjectSnapshot {
    inquiries: vec![takt_dash_backend::model::InquiryEntry {
      inquiry_id: "q1".to_owned(),
      goal_id:    Some("g1".to_owned()),
      kind:       takt_dash_backend::model::InquiryKind::Question,
      body:       "g1 の質問（前回）".to_owned(),
      options:    Vec::new(),
      answered:   false,
      created_at: time("2026-11-13T01:00:00Z"),
    }],
    ..takt_dash_backend::model::ProjectSnapshot::empty()
  };

  let snapshot = tool_set(&mut client, directory.path())
    .read_snapshot(Some(&prev))
    .await
    .expect("g1 の質問の道具エラーで全体は失敗しません");

  let q1 =
    snapshot.inquiries.iter().find(|entry| entry.inquiry_id == "q1").expect("道具エラーの g1 の質問を引き継ぎます");
  assert_eq!(q1.goal_id.as_deref(), Some("g1"));
  assert_eq!(q1.body, "g1 の質問（前回）", "g1 の質問は前回値を引き継ぎます");
  assert_eq!(q1.created_at, time("2026-11-13T01:00:00Z"), "g1 の質問の created_at も前回値を保ちます");
  let q2 = snapshot.inquiries.iter().find(|entry| entry.inquiry_id == "q2").expect("健常な g2 の質問は今回値です");
  assert_eq!(q2.goal_id.as_deref(), Some("g2"));
  assert_eq!(q2.body, "g2 の質問", "健常なゴールの質問は今回値を採ります");
  client.shutdown().await;
}

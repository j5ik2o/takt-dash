//! mock MCP サーバを使い、子プロセスの起動・ハンドシェイク・往復・isError・タイムアウト・
//! プロセス死亡からの再起動・応答の解析を結合的に検証する。

mod common;

use std::io::Write;

use common::*;
use serde_json::json;
use takt_dash_backend::{
  mcp::{CallError, TransportError},
  model::{
    ExecutionStatus, GoalEntry, GoalStatus, InquiryEntry, InquiryKind, NotificationEntry, ProjectSnapshot, RunEntry,
    RunStatus, RunStepEntry, TaskStatus,
  },
};

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

/// 指定件数の stepLog を持つ running run の状態を作る。
fn running_run_state(step_log_count: usize) -> serde_json::Value {
  let step_logs: Vec<serde_json::Value> = (0..step_log_count)
    .map(|index| {
      json!({
        "step": format!("step{index}"),
        "timestamp": format!("2026-11-13T10:{:02}:{:02}Z", index / 60, index % 60)
      })
    })
    .collect();
  json!({
    "tasks": [ { "name": "a", "status": "running", "runSlug": "r1", "startedAt": "2026-11-13T10:00:00Z" } ],
    "runs": { "r1": { "runSlug": "r1", "currentStep": "step0", "startedAt": "2026-11-13T10:00:00Z", "stepLogs": step_logs } }
  })
}

/// 指定件数の stepLog を持つ running run の状態を作る。`ended` に含まれる index の step だけが
/// 終了時刻と model を持ち、それ以外は継続中として終了時刻を持たない。
fn running_run_state_with_ended(step_log_count: usize, ended: &[usize]) -> serde_json::Value {
  let step_logs: Vec<serde_json::Value> = (0..step_log_count)
    .map(|index| {
      let mut log = json!({
        "step": format!("step{index}"),
        "timestamp": format!("2026-11-13T10:{:02}:{:02}Z", index / 60, index % 60)
      });
      if ended.contains(&index) {
        let object = log.as_object_mut().expect("stepLog は JSON オブジェクトです");
        object.insert("endedAt".to_owned(), json!(format!("2026-11-13T11:{:02}:{:02}Z", index / 60, index % 60)));
        object.insert("model".to_owned(), json!(format!("model-{index}")));
      }
      log
    })
    .collect();
  json!({
    "tasks": [ { "name": "a", "status": "running", "runSlug": "r1", "startedAt": "2026-11-13T10:00:00Z" } ],
    "runs": { "r1": { "runSlug": "r1", "currentStep": "step0", "startedAt": "2026-11-13T10:00:00Z", "stepLogs": step_logs } }
  })
}

/// 前回値として引き継ぐ、step 1 件を持つ running run のスナップショットを作る。
fn previous_run_snapshot(last_log_at: &str) -> ProjectSnapshot {
  ProjectSnapshot {
    runs: vec![RunEntry {
      run_slug:                "r1".to_owned(),
      session_id:              String::new(),
      task_name:               "a".to_owned(),
      goal_id:                 None,
      summary:                 String::new(),
      status:                  RunStatus::Running,
      current_step:            Some("implement".to_owned()),
      started_at:              time("2026-11-13T10:00:00Z"),
      current_step_started_at: None,
      last_log_at:             time(last_log_at),
      steps:                   vec![RunStepEntry {
        step_name:  "implement".to_owned(),
        model:      String::new(),
        started_at: time(last_log_at),
        ended_at:   None,
      }],
      reports:                 Vec::new(),
    }],
    ..ProjectSnapshot::empty()
  }
}

/// 前回値として引き継ぐ、1 ゴールとその問い合わせを持つスナップショットを作る。
fn previous_goal_question_snapshot() -> ProjectSnapshot {
  ProjectSnapshot {
    goals: vec![GoalEntry {
      goal_id:             "g1".to_owned(),
      status:              GoalStatus::Created,
      execution_status:    ExecutionStatus::Active,
      objective:           "ゴール g1".to_owned(),
      integration_branch:  "takt/g1".to_owned(),
      pull_request_number: None,
      pull_request_url:    None,
      awaiting_merge_at:   None,
      paused_reason:       None,
    }],
    inquiries: vec![InquiryEntry {
      inquiry_id: "q1".to_owned(),
      goal_id:    Some("g1".to_owned()),
      kind:       InquiryKind::Question,
      body:       "前回の質問".to_owned(),
      options:    Vec::new(),
      answered:   false,
      created_at: time("2026-11-13T01:00:00Z"),
    }],
    ..ProjectSnapshot::empty()
  }
}

/// 前回値として引き継ぐ、マージ待ちのゴールと通知を持つスナップショットを作る。
fn previous_notification_snapshot() -> ProjectSnapshot {
  ProjectSnapshot {
    goals: vec![GoalEntry {
      goal_id:             "g1".to_owned(),
      status:              GoalStatus::AwaitingMerge,
      execution_status:    ExecutionStatus::Active,
      objective:           "ゴール g1".to_owned(),
      integration_branch:  "takt/g1".to_owned(),
      pull_request_number: Some(5),
      pull_request_url:    Some("https://example.test/pr/5".to_owned()),
      awaiting_merge_at:   Some(time("2026-11-13T02:00:00Z")),
      paused_reason:       None,
    }],
    notifications: vec![NotificationEntry {
      notification_id: "n-merge".to_owned(),
      goal_id:         "g1".to_owned(),
      kind:            "awaiting_merge".to_owned(),
      body:            "前回のマージ待ち".to_owned(),
      recorded_at:     time("2026-11-13T02:00:00Z"),
    }],
    ..ProjectSnapshot::empty()
  }
}

/// 前回値として引き継ぐ、ゴールに属さない問い合わせを持つスナップショットを作る。
fn previous_standalone_inquiry_snapshot() -> ProjectSnapshot {
  ProjectSnapshot {
    inquiries: vec![InquiryEntry {
      inquiry_id: "s1".to_owned(),
      goal_id:    None,
      kind:       InquiryKind::OperationRequest,
      body:       "前回の作業依頼".to_owned(),
      options:    Vec::new(),
      answered:   false,
      created_at: time("2026-11-13T01:00:00Z"),
    }],
    ..ProjectSnapshot::empty()
  }
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

#[tokio::test]
async fn second_cycle_reads_only_new_step_logs_and_preserves_steps_and_last_log_at() {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(directory.path(), running_run_state(120));
  let call_log = directory.path().join("calls.log");
  let mut client = mock_client(mock_config_with_call_log(directory.path(), &state, &call_log));

  let previous = tool_set(&mut client, directory.path()).read_snapshot(None).await.expect("1 周期目を読み取れます");
  assert_eq!(previous.runs[0].steps.len(), 120);
  assert_eq!(previous.runs[0].last_log_at, time("2026-11-13T10:01:59Z"));

  write_state(directory.path(), running_run_state(123));
  clear_recorded_calls(&call_log);
  let next =
    tool_set(&mut client, directory.path()).read_snapshot(Some(&previous)).await.expect("2 周期目を読み取れます");
  let calls = recorded_calls(&call_log);
  let get_run_calls = calls.iter().filter(|call| call.as_str() == "takt_get_run").count();

  assert_eq!(next.runs[0].steps.len(), 123, "前回の steps に新しい stepLog を追記します");
  assert_eq!(next.runs[0].last_log_at, time("2026-11-13T10:02:02Z"), "最後のログの時刻は新しい末尾です");
  assert_eq!(get_run_calls, 2, "2 周期目は初回応答と前回 offset から先の 1 ページだけを読みます: {calls:?}");

  let full_directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let full_state = write_state(full_directory.path(), running_run_state(123));
  let mut full_client = mock_client(mock_config(full_directory.path(), &full_state, None));
  let full = tool_set(&mut full_client, full_directory.path()).read_snapshot(None).await.expect("フル読みできます");

  assert_eq!(next.runs[0].steps, full.runs[0].steps, "差分読みの steps はフル読みと一致します");
  assert_eq!(next.runs[0].last_log_at, full.runs[0].last_log_at, "差分読みの last_log_at はフル読みと一致します");

  client.shutdown().await;
  full_client.shutdown().await;
}

#[tokio::test]
async fn incremental_read_refreshes_known_steps_below_first_page_size() {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(directory.path(), running_run_state_with_ended(2, &[]));
  let call_log = directory.path().join("calls.log");
  let mut client = mock_client(mock_config_with_call_log(directory.path(), &state, &call_log));

  let previous = tool_set(&mut client, directory.path()).read_snapshot(None).await.expect("1 周期目を読み取れます");
  assert_eq!(previous.runs[0].steps.len(), 2);
  assert_eq!(previous.runs[0].steps[1].ended_at, None, "完了前の step は終了時刻を持ちません");

  write_state(directory.path(), running_run_state_with_ended(3, &[1]));
  clear_recorded_calls(&call_log);
  let next =
    tool_set(&mut client, directory.path()).read_snapshot(Some(&previous)).await.expect("2 周期目を読み取れます");
  let calls = recorded_calls(&call_log);
  let get_run_calls = calls.iter().filter(|call| call.as_str() == "takt_get_run").count();

  assert_eq!(get_run_calls, 1, "先頭ページに収まる増加は追加ページを読みません: {calls:?}");
  assert_eq!(next.runs[0].steps.len(), 3, "既知 step を置き換えて新しい stepLog を追記します");
  assert_eq!(
    next.runs[0].steps[1].ended_at,
    Some(time("2026-11-13T11:00:01Z")),
    "完了した step の終了時刻は応答値になります"
  );
  assert_eq!(next.runs[0].steps[1].model, "model-1", "完了した step の model も応答値になります");
  assert_eq!(next.runs[0].steps[2].step_name, "step2", "先頭ページの残りを末尾に足します");

  let full_directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let full_state = write_state(full_directory.path(), running_run_state_with_ended(3, &[1]));
  let mut full_client = mock_client(mock_config(full_directory.path(), &full_state, None));
  let full = tool_set(&mut full_client, full_directory.path()).read_snapshot(None).await.expect("フル読みできます");
  assert_eq!(next.runs[0].steps, full.runs[0].steps, "差分読みの steps はフル読みと一致します");

  client.shutdown().await;
  full_client.shutdown().await;
}

#[tokio::test]
async fn incremental_read_refreshes_known_steps_without_extra_page() {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(directory.path(), running_run_state_with_ended(50, &[]));
  let call_log = directory.path().join("calls.log");
  let mut client = mock_client(mock_config_with_call_log(directory.path(), &state, &call_log));

  let previous = tool_set(&mut client, directory.path()).read_snapshot(None).await.expect("1 周期目を読み取れます");
  assert_eq!(previous.runs[0].steps.len(), 50);
  assert_eq!(previous.runs[0].steps[49].ended_at, None, "完了前の step は終了時刻を持ちません");

  write_state(directory.path(), running_run_state_with_ended(50, &[49]));
  clear_recorded_calls(&call_log);
  let next =
    tool_set(&mut client, directory.path()).read_snapshot(Some(&previous)).await.expect("2 周期目を読み取れます");
  let calls = recorded_calls(&call_log);
  let get_run_calls = calls.iter().filter(|call| call.as_str() == "takt_get_run").count();

  assert_eq!(get_run_calls, 1, "既知件数が総件数と同じなら追加ページを読みません: {calls:?}");
  assert_eq!(next.runs[0].steps.len(), 50, "既知 step の置き換えで件数は変わりません");
  assert_eq!(
    next.runs[0].steps[49].ended_at,
    Some(time("2026-11-13T11:00:49Z")),
    "完了した step の終了時刻は応答値になります"
  );
  assert_eq!(next.runs[0].steps[49].model, "model-49", "完了した step の model も応答値になります");

  let full_directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let full_state = write_state(full_directory.path(), running_run_state_with_ended(50, &[49]));
  let mut full_client = mock_client(mock_config(full_directory.path(), &full_state, None));
  let full = tool_set(&mut full_client, full_directory.path()).read_snapshot(None).await.expect("フル読みできます");
  assert_eq!(next.runs[0].steps, full.runs[0].steps, "差分読みの steps はフル読みと一致します");

  client.shutdown().await;
  full_client.shutdown().await;
}

#[tokio::test]
async fn incremental_read_refreshes_known_steps_and_follows_extra_page() {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(directory.path(), running_run_state_with_ended(50, &[]));
  let call_log = directory.path().join("calls.log");
  let mut client = mock_client(mock_config_with_call_log(directory.path(), &state, &call_log));

  let previous = tool_set(&mut client, directory.path()).read_snapshot(None).await.expect("1 周期目を読み取れます");
  assert_eq!(previous.runs[0].steps.len(), 50);

  write_state(directory.path(), running_run_state_with_ended(51, &[49]));
  clear_recorded_calls(&call_log);
  let next =
    tool_set(&mut client, directory.path()).read_snapshot(Some(&previous)).await.expect("2 周期目を読み取れます");
  let calls = recorded_calls(&call_log);
  let get_run_calls = calls.iter().filter(|call| call.as_str() == "takt_get_run").count();

  assert_eq!(get_run_calls, 2, "増えた stepLog は初回応答と追加の 1 ページで読みます: {calls:?}");
  assert_eq!(next.runs[0].steps.len(), 51, "既知 step を置き換えて新しい stepLog を追記します");
  assert_eq!(
    next.runs[0].steps[49].ended_at,
    Some(time("2026-11-13T11:00:49Z")),
    "完了した step の終了時刻は応答値になります"
  );
  assert_eq!(next.runs[0].steps[49].model, "model-49", "完了した step の model も応答値になります");
  assert_eq!(next.runs[0].steps[50].step_name, "step50", "追加ページの step を末尾に足します");

  let full_directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let full_state = write_state(full_directory.path(), running_run_state_with_ended(51, &[49]));
  let mut full_client = mock_client(mock_config(full_directory.path(), &full_state, None));
  let full = tool_set(&mut full_client, full_directory.path()).read_snapshot(None).await.expect("フル読みできます");
  assert_eq!(next.runs[0].steps, full.runs[0].steps, "差分読みの steps はフル読みと一致します");

  client.shutdown().await;
  full_client.shutdown().await;
}

#[tokio::test]
async fn incremental_read_refreshes_known_tail_step_beyond_first_page() {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(directory.path(), running_run_state_with_ended(120, &[]));
  let call_log = directory.path().join("calls.log");
  let mut client = mock_client(mock_config_with_call_log(directory.path(), &state, &call_log));

  let previous = tool_set(&mut client, directory.path()).read_snapshot(None).await.expect("1 周期目を読み取れます");
  assert_eq!(previous.runs[0].steps.len(), 120);
  assert_eq!(previous.runs[0].steps[119].ended_at, None, "完了前の末尾 step は終了時刻を持ちません");

  write_state(directory.path(), running_run_state_with_ended(120, &[119]));
  clear_recorded_calls(&call_log);
  let next =
    tool_set(&mut client, directory.path()).read_snapshot(Some(&previous)).await.expect("2 周期目を読み取れます");
  let calls = recorded_calls(&call_log);
  let get_run_calls = calls.iter().filter(|call| call.as_str() == "takt_get_run").count();

  assert_eq!(get_run_calls, 2, "既知の末尾 step を含むページだけを追加で読みます: {calls:?}");
  assert_eq!(next.runs[0].steps.len(), 120, "先頭ページ外の既知 step の置き換えで件数は変わりません");
  assert_eq!(
    next.runs[0].steps[119].ended_at,
    Some(time("2026-11-13T11:01:59Z")),
    "完了した末尾 step の終了時刻は応答値になります"
  );
  assert_eq!(next.runs[0].steps[119].model, "model-119", "完了した末尾 step の model も応答値になります");
  assert_eq!(next.runs[0].last_log_at, previous.runs[0].last_log_at, "last_log_at は変わりません");

  let full_directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let full_state = write_state(full_directory.path(), running_run_state_with_ended(120, &[119]));
  let mut full_client = mock_client(mock_config(full_directory.path(), &full_state, None));
  let full = tool_set(&mut full_client, full_directory.path()).read_snapshot(None).await.expect("フル読みできます");
  assert_eq!(next.runs[0].steps, full.runs[0].steps, "差分読みの steps はフル読みと一致します");

  client.shutdown().await;
  full_client.shutdown().await;
}

#[tokio::test]
async fn incremental_read_refreshes_known_tail_step_and_appends_new_steps() {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(directory.path(), running_run_state_with_ended(120, &[]));
  let call_log = directory.path().join("calls.log");
  let mut client = mock_client(mock_config_with_call_log(directory.path(), &state, &call_log));

  let previous = tool_set(&mut client, directory.path()).read_snapshot(None).await.expect("1 周期目を読み取れます");
  assert_eq!(previous.runs[0].steps.len(), 120);
  assert_eq!(previous.runs[0].steps[119].ended_at, None, "完了前の末尾 step は終了時刻を持ちません");

  write_state(directory.path(), running_run_state_with_ended(123, &[119]));
  clear_recorded_calls(&call_log);
  let next =
    tool_set(&mut client, directory.path()).read_snapshot(Some(&previous)).await.expect("2 周期目を読み取れます");
  let calls = recorded_calls(&call_log);
  let get_run_calls = calls.iter().filter(|call| call.as_str() == "takt_get_run").count();

  assert_eq!(get_run_calls, 2, "既知の末尾 step を含むページで更新と追記をまとめて読みます: {calls:?}");
  assert_eq!(next.runs[0].steps.len(), 123, "既知 step を置き換えて新しい stepLog を追記します");
  assert_eq!(
    next.runs[0].steps[119].ended_at,
    Some(time("2026-11-13T11:01:59Z")),
    "完了した末尾 step の終了時刻は応答値になります"
  );
  assert_eq!(next.runs[0].steps[119].model, "model-119", "完了した末尾 step の model も応答値になります");
  let appended: Vec<&str> = next.runs[0].steps[120..].iter().map(|step| step.step_name.as_str()).collect();
  assert_eq!(appended, vec!["step120", "step121", "step122"], "先頭ページ外の新しい step を末尾に足します");

  let full_directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let full_state = write_state(full_directory.path(), running_run_state_with_ended(123, &[119]));
  let mut full_client = mock_client(mock_config(full_directory.path(), &full_state, None));
  let full = tool_set(&mut full_client, full_directory.path()).read_snapshot(None).await.expect("フル読みできます");
  assert_eq!(next.runs[0].steps, full.runs[0].steps, "差分読みの steps はフル読みと一致します");
  assert_eq!(next.runs[0].last_log_at, full.runs[0].last_log_at, "差分読みの last_log_at はフル読みと一致します");

  client.shutdown().await;
  full_client.shutdown().await;
}

#[tokio::test]
async fn steady_cycle_reads_known_tail_page_and_keeps_steps_and_last_log_at() {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(directory.path(), running_run_state(120));
  let call_log = directory.path().join("calls.log");
  let mut client = mock_client(mock_config_with_call_log(directory.path(), &state, &call_log));

  let previous = tool_set(&mut client, directory.path()).read_snapshot(None).await.expect("1 周期目を読み取れます");
  assert_eq!(previous.runs[0].steps.len(), 120);
  let original_steps = previous.runs[0].steps.clone();
  let original_last_log_at = previous.runs[0].last_log_at;

  clear_recorded_calls(&call_log);
  let next =
    tool_set(&mut client, directory.path()).read_snapshot(Some(&previous)).await.expect("2 周期目を読み取れます");
  let calls = recorded_calls(&call_log);
  let get_run_calls = calls.iter().filter(|call| call.as_str() == "takt_get_run").count();

  assert_eq!(get_run_calls, 2, "stepLog が増えていない周期も既知の末尾 step を含むページを読みます: {calls:?}");
  assert_eq!(next.runs[0].steps.len(), 120, "定常周期の steps 件数は変わりません");
  assert_eq!(next.runs[0].steps, original_steps, "定常周期の steps は前回と同じです");
  assert_eq!(next.runs[0].last_log_at, original_last_log_at, "定常周期の last_log_at は変わりません");

  client.shutdown().await;
}

#[tokio::test]
async fn step_log_reset_replaces_steps_instead_of_appending() {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(directory.path(), running_run_state(120));
  let mut client = mock_client(mock_config(directory.path(), &state, None));

  let previous = tool_set(&mut client, directory.path()).read_snapshot(None).await.expect("1 周期目を読み取れます");
  assert_eq!(previous.runs[0].steps.len(), 120);

  // stepLog が 3 件に減った状態は、前回 steps の追記ではなく全読みで置き換える。
  write_state(directory.path(), running_run_state(3));
  let next =
    tool_set(&mut client, directory.path()).read_snapshot(Some(&previous)).await.expect("2 周期目を読み取れます");

  let step_names: Vec<&str> = next.runs[0].steps.iter().map(|step| step.step_name.as_str()).collect();
  assert_eq!(step_names, vec!["step0", "step1", "step2"], "リセットされた steps で置き換えます");
  assert_eq!(next.runs[0].last_log_at, time("2026-11-13T10:00:02Z"), "最後のログの時刻も新しい末尾です");
  client.shutdown().await;
}

#[tokio::test]
async fn timeout_on_goal_questions_keeps_project_connected_and_previous_inquiries() {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(
    directory.path(),
    json!({
      "goals": [ { "id": "g1", "status": "created", "executionStatus": "active", "objective": "ゴール g1", "integrationBranch": "takt/g1" } ],
      "goalQuestions": { "g1": [ { "id": "q1", "status": "pending", "kind": "question", "body": "今回の質問", "createdAt": "2026-11-13T09:00:00Z" } ] },
      "hang": ["takt_list_goal_questions:g1"]
    }),
  );
  let mut client = mock_client(mock_config_with_timeout(directory.path(), &state, None, 1));
  let previous = previous_goal_question_snapshot();

  let snapshot = tool_set(&mut client, directory.path())
    .read_snapshot(Some(&previous))
    .await
    .expect("ゴール質問のタイムアウトはプロジェクト全体を失敗にしません");

  let inquiry =
    snapshot.inquiries.iter().find(|entry| entry.inquiry_id == "q1").expect("前回の問い合わせを引き継ぎます");
  assert_eq!(inquiry.body, "前回の質問", "タイムアウトしたゴールの問い合わせは前回値です");
  client.shutdown().await;
}

#[tokio::test]
async fn timeout_on_notifications_keeps_previous_notifications_and_awaiting_merge() {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(
    directory.path(),
    json!({
      "goals": [ { "id": "g1", "status": "awaiting_merge", "executionStatus": "active", "objective": "ゴール g1", "integrationBranch": "takt/g1", "pullRequest": { "number": 5, "url": "https://example.test/pr/5" } } ],
      "notifications": { "g1": [ { "id": "n-merge", "kind": "awaiting_merge", "body": "今回のマージ待ち", "recordedAt": "2026-11-13T02:30:00Z" } ] },
      "hang": ["takt_get_goal:g1"]
    }),
  );
  let mut client = mock_client(mock_config_with_timeout(directory.path(), &state, None, 1));
  let previous = previous_notification_snapshot();

  let snapshot = tool_set(&mut client, directory.path())
    .read_snapshot(Some(&previous))
    .await
    .expect("通知のタイムアウトはプロジェクト全体を失敗にしません");

  let notification =
    snapshot.notifications.iter().find(|entry| entry.notification_id == "n-merge").expect("前回の通知を引き継ぎます");
  assert_eq!(notification.body, "前回のマージ待ち", "タイムアウトした通知は前回値です");
  assert_eq!(
    snapshot.goals[0].awaiting_merge_at,
    Some(time("2026-11-13T02:00:00Z")),
    "タイムアウトした通知のマージ待ち起点は前回値です"
  );
  client.shutdown().await;
}

#[tokio::test]
async fn timeout_on_get_run_keeps_previous_run() {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(
    directory.path(),
    json!({
      "tasks": [ { "name": "a", "status": "running", "runSlug": "r1", "startedAt": "2026-11-13T10:00:00Z" } ],
      "hang": ["takt_get_run:r1"]
    }),
  );
  let mut client = mock_client(mock_config_with_timeout(directory.path(), &state, None, 1));
  let previous = previous_run_snapshot("2026-11-13T10:05:00Z");

  let snapshot = tool_set(&mut client, directory.path())
    .read_snapshot(Some(&previous))
    .await
    .expect("run のタイムアウトはプロジェクト全体を失敗にしません");

  assert_eq!(snapshot.runs.len(), 1, "前回の run を保持します");
  assert_eq!(snapshot.runs[0], previous.runs[0], "タイムアウトした run は前回値です");
  client.shutdown().await;
}

#[tokio::test]
async fn timeout_on_step_log_page_keeps_previous_run() {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(directory.path(), running_run_state(120));
  let mut client = mock_client(mock_config_with_timeout(directory.path(), &state, None, 1));

  let previous = tool_set(&mut client, directory.path()).read_snapshot(None).await.expect("1 周期目を読み取れます");
  assert_eq!(previous.runs[0].steps.len(), 120);

  let mut next_state = running_run_state(123);
  next_state["hang"] = json!(["takt_get_run:r1@119"]);
  write_state(directory.path(), next_state);
  let next = tool_set(&mut client, directory.path())
    .read_snapshot(Some(&previous))
    .await
    .expect("追加ページのタイムアウトはプロジェクト全体を失敗にしません");

  assert_eq!(next.runs.len(), 1, "前回の run を保持します");
  assert_eq!(next.runs[0], previous.runs[0], "追加ページのタイムアウトで中途半端な steps を残さず前回値へ戻します");
  client.shutdown().await;
}

#[tokio::test]
async fn timeout_on_standalone_inquiries_keeps_project_success() {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(
    directory.path(),
    json!({
      "goals": [],
      "standaloneInquiries": [ { "id": "s1", "status": "pending", "kind": "operation_request", "body": "今回の作業依頼", "createdAt": "2026-11-13T09:00:00Z" } ],
      "hang": ["takt_list_standalone_inquiries"]
    }),
  );
  let mut client = mock_client(mock_config_with_timeout(directory.path(), &state, None, 1));
  let previous = previous_standalone_inquiry_snapshot();

  let snapshot = tool_set(&mut client, directory.path())
    .read_snapshot(Some(&previous))
    .await
    .expect("ゴールに属さない問い合わせのタイムアウトはプロジェクト全体を失敗にしません");

  let inquiry = snapshot
    .inquiries
    .iter()
    .find(|entry| entry.inquiry_id == "s1")
    .expect("前回のゴールに属さない問い合わせを引き継ぎます");
  assert_eq!(inquiry.body, "前回の作業依頼", "タイムアウトした問い合わせは前回値です");
  client.shutdown().await;
}

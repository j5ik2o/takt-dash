//! mock MCP サーバを使い、子プロセスの起動・ハンドシェイク・往復・isError・タイムアウト・
//! プロセス死亡からの再起動・応答の解析を結合的に検証する。

mod common;

use std::io::Write;

use common::*;
use serde_json::{json, Value};
use takt_dash_backend::{
  mcp::{CallError, ToolError, TransportError},
  model::{
    ExecutionStatus, GoalEntry, GoalStatus, InquiryEntry, InquiryKind, NotificationEntry, ProjectSnapshot, RunEntry,
    RunStatus, RunStepEntry, TaskStatus,
  },
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
      current_step_number:     None,
      total_steps:             None,
      current_iteration:       None,
      started_at:              Some(time("2026-11-13T10:00:00Z")),
      current_step_started_at: None,
      last_log_at:             Some(time(last_log_at)),
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
async fn write_tools_time_out_finitely_when_the_server_hangs() {
  // 書き込み系の5道具が応答しないサーバでも、有限のタイムアウトで輸送エラーとして終わる。
  // これが unary の書き込み RPC の有限性の根拠です。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(directory.path(), json!({}));
  let cwd = directory.path();
  let mut errors: Vec<(&str, ToolError)> = Vec::new();

  {
    let mut client = mock_client(mock_config_with_timeout(cwd, &state, Some("hang"), 1));
    let error = tool_set(&mut client, cwd)
      .answer_goal_question("3f9c1a54-2b41-4d1e-9d2e-2c1a5b8f7e11", "q1", json!({ "text": "はい" }))
      .await
      .map(|_| ())
      .expect_err("応答がなければタイムアウトします");
    client.shutdown().await;
    errors.push(("takt_answer_goal_question", error));
  }
  {
    let mut client = mock_client(mock_config_with_timeout(cwd, &state, Some("hang"), 1));
    let error = tool_set(&mut client, cwd)
      .list_goal_registration_proposals()
      .await
      .map(|_| ())
      .expect_err("応答がなければタイムアウトします");
    client.shutdown().await;
    errors.push(("takt_list_goal_registration_proposals", error));
  }
  {
    let mut client = mock_client(mock_config_with_timeout(cwd, &state, Some("hang"), 1));
    let error = tool_set(&mut client, cwd)
      .approve_goal_registration(3)
      .await
      .map(|_| ())
      .expect_err("応答がなければタイムアウトします");
    client.shutdown().await;
    errors.push(("takt_approve_goal_registration", error));
  }
  {
    let mut client = mock_client(mock_config_with_timeout(cwd, &state, Some("hang"), 1));
    let error = tool_set(&mut client, cwd)
      .reject_goal_registration(3)
      .await
      .map(|_| ())
      .expect_err("応答がなければタイムアウトします");
    client.shutdown().await;
    errors.push(("takt_reject_goal_registration", error));
  }
  {
    let mut client = mock_client(mock_config_with_timeout(cwd, &state, Some("hang"), 1));
    let error = tool_set(&mut client, cwd)
      .merge_goal_pr("g1", 42, "merge")
      .await
      .map(|_| ())
      .expect_err("応答がなければタイムアウトします");
    client.shutdown().await;
    errors.push(("takt_merge_goal_pr", error));
  }

  for (name, error) in errors {
    assert!(
      matches!(error, ToolError::Transport(TransportError::Timeout(_))),
      "{name} は有限時間でタイムアウトとして終わる必要があります: {error}"
    );
  }
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
  assert_eq!(
    snapshot.runs[0].last_log_at,
    Some(time("2026-11-13T10:05:00Z")),
    "最後のログの時刻は末尾の stepLogs です"
  );
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
async fn goal_without_awaiting_merge_notification_gets_first_observed_wait_start() {
  // 初回読み取り（prev=None）でも、awaiting_merge 通知が無いゴールは観測時刻を起点に確定する。
  // 起点を持たせないと、以降の投影で待ち時間が読み取りごとに現在時刻へリセットされ滞留しなくなる。
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

  assert!(
    snapshot.goals[0].awaiting_merge_at.is_some(),
    "初回読み取りでも awaiting_merge 通知がないゴールは観測時刻を起点に確定します"
  );
  client.shutdown().await;
}

#[tokio::test]
async fn awaiting_merge_goal_without_notification_mechanism_keeps_previous_wait_start() {
  // awaiting_merge 通知が無いゴールでも、前回の起点時刻を引き継ぐ。都度読み取り時刻へ
  // 起点を既定化すると、待ち時間が毎サイクル 0 に戻り滞留しなくなる。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(
    directory.path(),
    json!({
      "goals": [ { "id": "g1", "status": "awaiting_merge", "executionStatus": "active", "objective": "目標", "integrationBranch": "takt/g1" } ],
      "notifications": { "g1": [ { "id": "n-progress", "kind": "progress", "body": "進捗", "recordedAt": "2026-11-13T08:00:00Z" } ] }
    }),
  );
  let mut client = mock_client(mock_config(directory.path(), &state, None));
  let mut previous_goal = goal("g1", GoalStatus::AwaitingMerge, takt_dash_backend::model::ExecutionStatus::Active);
  previous_goal.awaiting_merge_at = Some(time("2026-11-13T01:00:00Z"));
  let prev = takt_dash_backend::model::ProjectSnapshot {
    goals: vec![previous_goal],
    ..takt_dash_backend::model::ProjectSnapshot::empty()
  };

  let snapshot = tool_set(&mut client, directory.path()).read_snapshot(Some(&prev)).await.expect("状態を読み取れます");

  assert_eq!(
    snapshot.goals[0].awaiting_merge_at,
    Some(time("2026-11-13T01:00:00Z")),
    "awaiting_merge 通知が無いゴールは前回の起点時刻を引き継ぎます"
  );
  client.shutdown().await;
}

#[tokio::test]
async fn awaiting_merge_goal_new_to_awaiting_merge_gets_a_first_observed_start() {
  // 前回は awaiting_merge でなかったゴールが今回 awaiting_merge に入り、通知も無い場合は、
  // 初回観測の時刻を起点にする。起点なし（None）で滞留判定を保留にはしない。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(
    directory.path(),
    json!({
      "goals": [ { "id": "g1", "status": "awaiting_merge", "executionStatus": "active", "objective": "目標", "integrationBranch": "takt/g1" } ]
    }),
  );
  let mut client = mock_client(mock_config(directory.path(), &state, None));
  let prev = takt_dash_backend::model::ProjectSnapshot {
    goals: vec![goal("g1", GoalStatus::Created, takt_dash_backend::model::ExecutionStatus::Active)],
    ..takt_dash_backend::model::ProjectSnapshot::empty()
  };

  let snapshot = tool_set(&mut client, directory.path()).read_snapshot(Some(&prev)).await.expect("状態を読み取れます");

  assert!(
    snapshot.goals[0].awaiting_merge_at.is_some(),
    "通知のない新しい人のマージ待ちは初回観測の時刻を起点にします"
  );
  client.shutdown().await;
}

#[tokio::test]
async fn awaiting_merge_wait_start_is_resolved_per_goal_without_reusing_another_goals_start() {
  // 前回の起点はゴール ID で識別する。前回 snapshot に同じゴール g2 の起点があればそれを引き継ぎ、
  // 別ゴール g1 の起点は使わない。不等式だけでは任意の起点を返す誤実装を検出できないため、
  // g2 の起点を等値で観測する。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(
    directory.path(),
    json!({
      "goals": [ { "id": "g2", "status": "awaiting_merge", "executionStatus": "active", "objective": "目標 g2", "integrationBranch": "takt/g2" } ]
    }),
  );
  let mut client = mock_client(mock_config(directory.path(), &state, None));
  let mut g1_start = goal("g1", GoalStatus::AwaitingMerge, takt_dash_backend::model::ExecutionStatus::Active);
  g1_start.awaiting_merge_at = Some(time("2026-11-13T01:00:00Z"));
  let mut g2_start = goal("g2", GoalStatus::AwaitingMerge, takt_dash_backend::model::ExecutionStatus::Active);
  g2_start.awaiting_merge_at = Some(time("2026-11-13T02:00:00Z"));
  let prev = takt_dash_backend::model::ProjectSnapshot {
    goals: vec![g1_start, g2_start],
    ..takt_dash_backend::model::ProjectSnapshot::empty()
  };

  let snapshot = tool_set(&mut client, directory.path()).read_snapshot(Some(&prev)).await.expect("状態を読み取れます");

  let g2 = snapshot.goals.iter().find(|goal| goal.goal_id == "g2").expect("g2 を含みます");
  assert_eq!(g2.awaiting_merge_at, Some(time("2026-11-13T02:00:00Z")), "g2 は同じゴール ID の前回起点を引き継ぎます");
  assert_ne!(g2.awaiting_merge_at, Some(time("2026-11-13T01:00:00Z")), "g1 の起点時刻を g2 へ使い回しません");
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
async fn write_tool_responses_missing_required_fields_are_parse_errors() {
  // 応答の必須値が欠けていても既定値で補完せず、解析エラーにすることを、各書き込み道具で観測する。
  async fn expect_parse(
    tool_name: &str,
    behavior: impl AsyncFnOnce(takt_dash_backend::mcp::ToolSet<'_>) -> Result<(), ToolError>,
  ) -> Result<(), ToolError> {
    let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
    let state = write_state(directory.path(), json!({ "responses": { tool_name: {} } }));
    let mut client = mock_client(mock_config(directory.path(), &state, None));
    let result = behavior(tool_set(&mut client, directory.path())).await;
    client.shutdown().await;
    result
  }

  let error = expect_parse("takt_approve_goal_registration", async |mut tools| {
    tools.approve_goal_registration(3).await.map(|_| ())
  })
  .await
  .expect_err("応答に revision が無ければ解析エラーです");
  assert!(matches!(error, ToolError::Parse(_)), "takt_approve_goal_registration の欠落は解析エラー: {error}");

  let error = expect_parse("takt_reject_goal_registration", async |mut tools| {
    tools.reject_goal_registration(3).await.map(|_| ())
  })
  .await
  .expect_err("応答に revision が無ければ解析エラーです");
  assert!(matches!(error, ToolError::Parse(_)), "takt_reject_goal_registration の欠落は解析エラー: {error}");

  let error =
    expect_parse("takt_merge_goal_pr", async |mut tools| tools.merge_goal_pr("g1", 42, "merge").await.map(|_| ()))
      .await
      .expect_err("応答に taskName が無ければ解析エラーです");
  assert!(matches!(error, ToolError::Parse(_)), "takt_merge_goal_pr の欠落は解析エラー: {error}");

  let error = expect_parse("takt_answer_goal_question", async |mut tools| {
    tools
      .answer_goal_question("3f9c1a54-2b41-4d1e-9d2e-2c1a5b8f7e11", "q1", json!({ "text": "はい" }))
      .await
      .map(|_| ())
  })
  .await
  .expect_err("応答に questionId が無ければ解析エラーです");
  assert!(matches!(error, ToolError::Parse(_)), "takt_answer_goal_question の欠落は解析エラー: {error}");

  let error = expect_parse("takt_list_goal_registration_proposals", async |mut tools| {
    tools.list_goal_registration_proposals().await.map(|_| ())
  })
  .await
  .expect_err("応答に proposals が無ければ解析エラーです");
  assert!(matches!(error, ToolError::Parse(_)), "takt_list_goal_registration_proposals の欠落は解析エラー: {error}");
}

#[tokio::test]
async fn summary_object_is_composed_into_the_body() {
  // 実 takt-mcp の応答（order.md の ac7-evidence）を模し、summary がオブジェクトでも提案を
  // 1件取得でき、本文に各項目の値が入り、未知キーは本文へ漏れないことを観測する。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(
    directory.path(),
    json!({
      "responses": {
        "takt_list_goal_registration_proposals": {
          "proposals": [
            {
              "revision": 6,
              "repositoryDirectory": "/abs/takt-dash",
              "summary": {
                "objective": "稼働状況を一覧できるようにする",
                "outOfScope": ["認証", "ネイティブ版"],
                "acceptanceCriteria": ["判断待ちに表示される"],
                "constraints": ["proto を変えない"],
                "startBranch": "main",
                "integrationBranch": "takt/goal-6",
                "unexpectedKey": "本文に漏れてはいけない値"
              }
            }
          ]
        }
      }
    }),
  );
  let mut client = mock_client(mock_config(directory.path(), &state, None));

  let proposals = tool_set(&mut client, directory.path())
    .list_goal_registration_proposals()
    .await
    .expect("summary がオブジェクトでも提案を取得できます");

  assert_eq!(proposals.len(), 1, "提案を1件取得します");
  let proposal = &proposals[0];
  assert_eq!(proposal.revision, 6, "revision を写します");
  assert_eq!(proposal.repository_directory, "/abs/takt-dash", "repositoryDirectory を写します");
  for expected in [
    "稼働状況を一覧できるようにする",
    "認証",
    "ネイティブ版",
    "判断待ちに表示される",
    "proto を変えない",
    "main",
    "takt/goal-6",
  ] {
    assert!(proposal.summary.contains(expected), "本文に存在する項目 {expected} を含みます: {}", proposal.summary);
  }
  assert!(!proposal.summary.contains("本文に漏れてはいけない値"), "未知のキーは本文へ含めません: {}", proposal.summary);
  client.shutdown().await;
}

#[tokio::test]
async fn summary_object_with_missing_and_unknown_keys_is_accepted() {
  // 任意項目の欠落と未知キーがあっても失敗させず、objective だけで本文を組み立てる。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(
    directory.path(),
    json!({
      "responses": {
        "takt_list_goal_registration_proposals": {
          "proposals": [
            {
              "revision": 7,
              "repositoryDirectory": "/abs/takt-dash",
              "summary": { "objective": "目標だけを持つ提案", "unexpectedKey": "無視する値" }
            }
          ]
        }
      }
    }),
  );
  let mut client = mock_client(mock_config(directory.path(), &state, None));

  let proposals = tool_set(&mut client, directory.path())
    .list_goal_registration_proposals()
    .await
    .expect("任意項目の欠落と未知キーでは失敗しません");

  assert_eq!(proposals.len(), 1, "提案を1件取得します");
  assert!(proposals[0].summary.contains("目標だけを持つ提案"), "本文に objective を含みます: {}", proposals[0].summary);
  assert!(!proposals[0].summary.contains("無視する値"), "未知のキーは本文へ含めません: {}", proposals[0].summary);
  client.shutdown().await;
}

#[tokio::test]
async fn summary_string_is_kept_as_the_body() {
  // summary が文字列の既存応答は、その文字列をそのまま本文にする。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(
    directory.path(),
    json!({
      "responses": {
        "takt_list_goal_registration_proposals": {
          "proposals": [
            { "revision": 8, "repositoryDirectory": "/abs/takt-dash", "summary": "文字列の要約" }
          ]
        }
      }
    }),
  );
  let mut client = mock_client(mock_config(directory.path(), &state, None));

  let proposals = tool_set(&mut client, directory.path())
    .list_goal_registration_proposals()
    .await
    .expect("summary が文字列でも提案を取得できます");

  assert_eq!(proposals.len(), 1, "提案を1件取得します");
  assert_eq!(proposals[0].summary, "文字列の要約", "文字列の summary はそのまま本文になります");
  client.shutdown().await;
}

#[tokio::test]
async fn malformed_proposal_summary_keeps_previous_proposals() {
  // 提案1件の解析に失敗しても snapshot 全体を落とさず、前回の提案を保持する。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(
    directory.path(),
    json!({
      "responses": {
        "takt_list_goal_registration_proposals": {
          "proposals": [
            { "revision": 6, "repositoryDirectory": "/abs/takt-dash", "summary": 123 }
          ]
        }
      }
    }),
  );
  let mut client = mock_client(mock_config(directory.path(), &state, None));
  let prev = takt_dash_backend::model::ProjectSnapshot {
    proposals: vec![takt_dash_backend::model::RegistrationProposalEntry {
      revision:             5,
      repository_directory: "/abs/takt-dash".to_owned(),
      summary:              "前回の提案".to_owned(),
    }],
    ..takt_dash_backend::model::ProjectSnapshot::empty()
  };

  let snapshot = tool_set(&mut client, directory.path())
    .read_snapshot(Some(&prev))
    .await
    .expect("提案の解析失敗は前回の提案へ縮退します");

  assert_eq!(snapshot.proposals, prev.proposals, "前回の提案を保持します");
  client.shutdown().await;
}

#[tokio::test]
async fn malformed_proposal_summary_without_previous_yields_empty_proposals() {
  // 前回の提案が無いときの解析失敗は、提案だけを空にして周期を失敗させない。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(
    directory.path(),
    json!({
      "responses": {
        "takt_list_goal_registration_proposals": {
          "proposals": [
            { "revision": 6, "repositoryDirectory": "/abs/takt-dash", "summary": 123 }
          ]
        }
      }
    }),
  );
  let mut client = mock_client(mock_config(directory.path(), &state, None));

  let snapshot = tool_set(&mut client, directory.path())
    .read_snapshot(None)
    .await
    .expect("前回の提案が無くても解析失敗で snapshot を落としません");

  assert!(snapshot.proposals.is_empty(), "前回の提案が無ければ空へ縮退します");
  client.shutdown().await;
}

#[tokio::test]
async fn proposal_transport_error_is_not_degraded_to_the_previous_proposals() {
  // 解析失敗だけを縮退させ、輸送エラーは従来どおり伝播させることを観測する。
  // 子プロセスが提案の取得中に応答せず終了する状態で、前回の提案へ置き換えないことを確かめる。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(directory.path(), json!({ "transportErrors": ["takt_list_goal_registration_proposals"] }));
  let mut client = mock_client(mock_config(directory.path(), &state, None));
  let prev = takt_dash_backend::model::ProjectSnapshot {
    proposals: vec![takt_dash_backend::model::RegistrationProposalEntry {
      revision:             5,
      repository_directory: "/abs/takt-dash".to_owned(),
      summary:              "前回の提案".to_owned(),
    }],
    ..takt_dash_backend::model::ProjectSnapshot::empty()
  };

  let error = tool_set(&mut client, directory.path())
    .read_snapshot(Some(&prev))
    .await
    .expect_err("輸送エラーは縮退せず伝播します");

  assert!(matches!(error, ToolError::Transport(_)), "輸送エラーは解析失敗と区別します: {error}");
  client.shutdown().await;
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
  assert_eq!(run.last_log_at, Some(time("2026-11-13T10:50:00Z")), "最後のログの時刻は末尾ページの最終ステップです");
  client.shutdown().await;
}

#[tokio::test]
async fn read_snapshot_reads_start_time_only_run() {
  // 実行中の run は開始時刻を `startTime` で返す。一覧の読み取りが `startedAt` だけを読むと
  // parse エラーで一覧全体が失敗するため、`startTime` でも run を取り込めることを観測する。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(
    directory.path(),
    json!({
      "tasks": [ { "name": "a", "status": "running", "runSlug": "r1" } ],
      "runs": { "r1": { "runSlug": "r1", "currentStep": "implement", "startTime": "2026-11-13T10:00:00Z", "stepLogs": [ { "step": "implement", "timestamp": "2026-11-13T10:20:00Z" } ] } }
    }),
  );
  let mut client = mock_client(mock_config(directory.path(), &state, None));

  let snapshot = tool_set(&mut client, directory.path()).read_snapshot(None).await.expect("状態を読み取れます");

  assert_eq!(snapshot.runs.len(), 1, "startTime だけを持つ run も一覧に含みます");
  assert_eq!(snapshot.runs[0].started_at, Some(time("2026-11-13T10:00:00Z")), "開始時刻は startTime の値です");
  assert_eq!(snapshot.runs[0].last_log_at, Some(time("2026-11-13T10:20:00Z")), "最後のログの時刻はステップの時刻です");
  client.shutdown().await;
}

#[tokio::test]
async fn read_snapshot_keeps_a_run_without_any_start_or_log_time() {
  // 実行中の run が開始時刻も最後のログの時刻も持たない場合でも、一覧の読み取りが失敗せず
  // run が snapshot に含まれることを観測する。一覧が解析エラーで全体失敗すると run が状態
  // ストアに現れず、詳細取得の入口 find_run_project が成立しない。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(
    directory.path(),
    json!({
      "tasks": [ { "name": "a", "status": "running", "runSlug": "r1" } ],
      "runs": { "r1": { "runSlug": "r1", "currentStep": "implement", "stepLogs": [] } }
    }),
  );
  let mut client = mock_client(mock_config(directory.path(), &state, None));

  let snapshot = tool_set(&mut client, directory.path()).read_snapshot(None).await.expect("状態を読み取れます");

  assert_eq!(snapshot.runs.len(), 1, "開始時刻もログ時刻も無い run も一覧に含みます");
  assert_eq!(snapshot.runs[0].run_slug, "r1");
  assert_eq!(snapshot.runs[0].started_at, None, "開始時刻は未設定として保持します");
  assert_eq!(snapshot.runs[0].last_log_at, None, "最後のログの時刻も未設定として保持します");
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
      current_iteration:       None,
      current_step_number:     None,
      total_steps:             None,
      started_at:              Some(time("2026-11-13T10:00:00Z")),
      current_step_started_at: None,
      last_log_at:             Some(time("2026-11-13T10:05:00Z")),
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
  assert_eq!(snapshot.runs[0].last_log_at, Some(time("2026-11-13T10:05:00Z")));
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
      current_iteration:       None,
      current_step_number:     None,
      total_steps:             None,
      started_at:              Some(time("2026-11-13T10:00:00Z")),
      current_step_started_at: None,
      last_log_at:             Some(time("2026-11-13T10:05:00Z")),
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
  assert_eq!(run_a.last_log_at, Some(time("2026-11-13T10:05:00Z")), "道具エラーの runA は前回値です");
  let run_b = snapshot.runs.iter().find(|run| run.run_slug == "runB").expect("runB が残ります");
  assert_eq!(run_b.last_log_at, Some(time("2026-11-13T10:20:00Z")), "健常な runB は今回値へ更新されます");
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
  assert_eq!(previous.runs[0].last_log_at, Some(time("2026-11-13T10:01:59Z")));

  write_state(directory.path(), running_run_state(123));
  clear_recorded_calls(&call_log);
  let next =
    tool_set(&mut client, directory.path()).read_snapshot(Some(&previous)).await.expect("2 周期目を読み取れます");
  let calls = recorded_calls(&call_log);
  let get_run_calls = calls.iter().filter(|call| call.as_str() == "takt_get_run").count();

  assert_eq!(next.runs[0].steps.len(), 123, "前回の steps に新しい stepLog を追記します");
  assert_eq!(next.runs[0].last_log_at, Some(time("2026-11-13T10:02:02Z")), "最後のログの時刻は新しい末尾です");
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
  assert_eq!(next.runs[0].last_log_at, Some(time("2026-11-13T10:00:02Z")), "最後のログの時刻も新しい末尾です");
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

#[tokio::test]
async fn run_step_progress_is_derived_from_the_workflow_step_list() {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(
    directory.path(),
    json!({
      "tasks": [ { "name": "a", "status": "running", "runSlug": "r1", "startedAt": "2026-11-13T10:00:00Z" } ],
      "runs": { "r1": {
        "runSlug": "r1",
        "workflow": "wf",
        "currentStep": "implement",
        "currentIteration": 2,
        "startedAt": "2026-11-13T10:00:00Z",
        "stepLogs": [ { "step": "implement", "timestamp": "2026-11-13T10:20:00Z" } ]
      } },
      "workflows": { "wf": ["plan", "implement", "review", "fix", "close"] }
    }),
  );
  let mut client = mock_client(mock_config(directory.path(), &state, None));

  let snapshot = tool_set(&mut client, directory.path()).read_snapshot(None).await.expect("状態を読み取れます");

  let run = &snapshot.runs[0];
  assert_eq!(run.current_step_number, Some(2), "現在ステップの位置を workflow の並びから求めます");
  assert_eq!(run.total_steps, Some(5), "workflow の全ステップ数を求めます");
  assert_eq!(run.current_iteration, Some(2), "takt_get_run の currentIteration を読み取ります");
  client.shutdown().await;
}

#[tokio::test]
async fn run_step_position_uses_the_current_step_even_when_the_last_log_is_a_nested_step() {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(
    directory.path(),
    json!({
      "tasks": [ { "name": "a", "status": "running", "runSlug": "r1", "startedAt": "2026-11-13T10:00:00Z" } ],
      "runs": { "r1": {
        "runSlug": "r1",
        "workflow": "wf",
        "currentStep": "develop",
        "currentIteration": 3,
        "startedAt": "2026-11-13T10:00:00Z",
        "stepLogs": [ { "step": "write_tests", "timestamp": "2026-11-13T10:20:00Z" } ]
      } },
      "workflows": { "wf": ["develop", "machine_gate", "fix_machine_gate"] }
    }),
  );
  let mut client = mock_client(mock_config(directory.path(), &state, None));

  let snapshot = tool_set(&mut client, directory.path()).read_snapshot(None).await.expect("状態を読み取れます");

  let run = &snapshot.runs[0];
  assert_eq!(run.current_step_number, Some(1), "入れ子 workflow のログではなく currentStep を位置に使います");
  assert_eq!(run.total_steps, Some(3), "workflow の全ステップ数を求めます");
  assert_eq!(run.current_iteration, Some(3), "takt_get_run の currentIteration を読み取ります");
  assert_eq!(run.current_step.as_deref(), Some("develop"), "今のステップ名は currentStep から読み取ります");
  assert_eq!(
    run.steps.last().map(|step| step.step_name.as_str()),
    Some("write_tests"),
    "stepLogs の末尾は入れ子 workflow の内側の step で、今のトップレベル step とは異なります"
  );
  client.shutdown().await;
}

#[tokio::test]
async fn run_step_position_is_unset_when_the_current_step_is_not_in_the_workflow() {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(
    directory.path(),
    json!({
      "tasks": [ { "name": "a", "status": "running", "runSlug": "r1", "startedAt": "2026-11-13T10:00:00Z" } ],
      "runs": { "r1": {
        "runSlug": "r1",
        "workflow": "wf",
        "currentStep": "implement",
        "currentIteration": 2,
        "startedAt": "2026-11-13T10:00:00Z",
        "stepLogs": [ { "step": "implement", "timestamp": "2026-11-13T10:20:00Z" } ]
      } },
      "workflows": { "wf": ["plan", "review"] }
    }),
  );
  let mut client = mock_client(mock_config(directory.path(), &state, None));

  let snapshot = tool_set(&mut client, directory.path()).read_snapshot(None).await.expect("状態を読み取れます");

  let run = &snapshot.runs[0];
  assert_eq!(run.current_step_number, None, "workflow に無いステップの位置を捏造しません");
  assert_eq!(run.total_steps, None, "位置が決まらない run は総数も設定しません");
  assert_eq!(run.current_iteration, Some(2), "反復回数は位置と独立して保持します");
  client.shutdown().await;
}

#[tokio::test]
async fn workflow_tool_error_keeps_the_run_without_step_progress() {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(
    directory.path(),
    json!({
      "tasks": [ { "name": "a", "status": "running", "runSlug": "r1", "startedAt": "2026-11-13T10:00:00Z" } ],
      "runs": { "r1": {
        "runSlug": "r1",
        "workflow": "wf",
        "currentStep": "implement",
        "currentIteration": 2,
        "startedAt": "2026-11-13T10:00:00Z",
        "stepLogs": [ { "step": "implement", "timestamp": "2026-11-13T10:20:00Z" } ]
      } },
      "workflows": { "wf": ["plan", "implement"] },
      "isError": ["takt_get_workflow:wf"]
    }),
  );
  let mut client = mock_client(mock_config(directory.path(), &state, None));

  let snapshot = tool_set(&mut client, directory.path())
    .read_snapshot(None)
    .await
    .expect("workflow の道具エラーで run 一覧全体を失敗させません");

  assert_eq!(snapshot.runs.len(), 1, "workflow が読めなくても run は一覧に残ります");
  let run = &snapshot.runs[0];
  assert_eq!(run.current_step_number, None, "workflow が読めない run は位置を設定しません");
  assert_eq!(run.total_steps, None, "workflow が読めない run は総数を設定しません");
  assert_eq!(run.current_iteration, Some(2), "反復回数は workflow の成否と独立して保持します");
  client.shutdown().await;
}

// ----------------------------------------------------------------------------
// takt から受け取る整数の範囲検査
// ----------------------------------------------------------------------------

#[tokio::test]
async fn revision_at_the_i32_maximum_is_accepted() {
  // i32 の上限はそのまま受理し、切り詰めません。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(
    directory.path(),
    json!({
      "responses": {
        "takt_list_goal_registration_proposals": {
          "proposals": [
            { "revision": 2147483647, "repositoryDirectory": "/abs/takt-dash", "summary": "上限の提案" }
          ]
        }
      }
    }),
  );
  let mut client = mock_client(mock_config(directory.path(), &state, None));

  let proposals = tool_set(&mut client, directory.path())
    .list_goal_registration_proposals()
    .await
    .expect("i32 の上限の revision は受理します");

  assert_eq!(proposals.len(), 1);
  assert_eq!(proposals[0].revision, i32::MAX, "i32 の上限をそのまま返します");
  client.shutdown().await;
}

#[tokio::test]
async fn revision_beyond_i32_is_a_parse_error() {
  // 4294967297 を `as i32` で切り詰めると 1 になる。切り詰めずに解析エラーにする。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(
    directory.path(),
    json!({
      "responses": {
        "takt_list_goal_registration_proposals": {
          "proposals": [
            { "revision": 4294967297i64, "repositoryDirectory": "/abs/takt-dash", "summary": "範囲外の提案" }
          ]
        }
      }
    }),
  );
  let mut client = mock_client(mock_config(directory.path(), &state, None));

  let error = tool_set(&mut client, directory.path())
    .list_goal_registration_proposals()
    .await
    .expect_err("i32 の範囲外の revision は受理しません");

  assert!(matches!(error, ToolError::Parse(_)), "範囲外の revision は解析エラーです: {error}");
  client.shutdown().await;
}

#[tokio::test]
async fn instruction_id_beyond_i32_is_a_parse_error_and_is_not_sent_truncated() {
  // run の指示書が i32 の範囲外の instructionId を持つ場合、2 に切り詰めて
  // takt_get_run を呼ばずに解析エラーにする。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(
    directory.path(),
    json!({
      "tasks": [ { "name": "a", "status": "running", "runSlug": "r1", "startedAt": "2026-11-13T10:00:00Z" } ],
      "runs": {
        "r1": {
          "runSlug": "r1",
          "startedAt": "2026-11-13T10:00:00Z",
          "instructions": [
            { "instructionId": 4294967298i64, "issuedAt": "2026-11-13T10:05:00Z", "state": "issued", "content": "指示書" }
          ]
        }
      }
    }),
  );
  let calls_path = directory.path().join("tool-calls.jsonl");
  let mut client = mock_client(mock_config_with_calls(directory.path(), &state, &calls_path));

  let error = tool_set(&mut client, directory.path())
    .read_run_detail("r1")
    .await
    .expect_err("i32 の範囲外の instructionId は受理しません");

  assert!(matches!(error, ToolError::Parse(_)), "範囲外の instructionId は解析エラーです: {error}");

  let recorded: Vec<Value> = match std::fs::read_to_string(&calls_path) {
    | Ok(text) => {
      text.lines().filter(|line| !line.trim().is_empty()).filter_map(|line| serde_json::from_str(line).ok()).collect()
    },
    | Err(_) => Vec::new(),
  };
  assert!(
    recorded
      .iter()
      .all(|call| !(call["name"] == json!("takt_get_run") && call["arguments"]["instructionId"] == json!(2))),
    "instructionId を 2 に切り詰めた takt_get_run を記録しません: {recorded:?}"
  );
  client.shutdown().await;
}

#[tokio::test]
async fn 一覧の最終ログ時刻は応答の最終ログ時刻を最優先する() {
  // 応答の lastLogAt がステップ記録と現在ステップの開始時刻より新しいとき、一覧の
  // last_log_at は lastLogAt を採ります。同じ状態の詳細も同じ値になることを観測します。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(
    directory.path(),
    json!({
      "tasks": [ { "name": "implement", "goalId": "g1", "status": "running", "startedAt": "2026-11-13T11:30:00Z", "runSlug": "r1", "currentStep": "implement" } ],
      "runs": { "r1": {
        "runSlug": "r1",
        "startedAt": "2026-11-13T11:30:00Z",
        "currentStep": "implement",
        "currentStepStartedAt": "2026-11-13T11:50:00Z",
        "lastLogAt": "2026-11-13T11:59:00Z",
        "stepLogs": [ { "step": "plan", "status": "completed", "timestamp": "2026-11-13T11:40:00Z" } ]
      } }
    }),
  );
  let mut client = mock_client(mock_config(directory.path(), &state, None));

  let snapshot = tool_set(&mut client, directory.path()).read_snapshot(None).await.expect("状態を読み取れます");
  let detail = tool_set(&mut client, directory.path()).read_run_detail("r1").await.expect("run 詳細を読み取れます");

  assert_eq!(detail.last_log_at, Some(time("2026-11-13T11:59:00Z")), "詳細は応答の lastLogAt を最終ログ時刻に使います");
  assert_eq!(
    snapshot.runs[0].last_log_at,
    Some(time("2026-11-13T11:59:00Z")),
    "一覧の最終ログ時刻は応答の lastLogAt を最優先します"
  );
  client.shutdown().await;
}

#[tokio::test]
async fn 最終ログ時刻がないときは現在ステップの開始時刻を採る() {
  // lastLogAt が無いとき、古い完了ステップの記録ではなく、現在ステップの開始時刻が
  // 新しければそちらを採ります。一覧と詳細の無音判定が食い違わないことを観測します。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(
    directory.path(),
    json!({
      "tasks": [ { "name": "implement", "goalId": "g1", "status": "running", "startedAt": "2026-11-13T11:30:00Z", "runSlug": "r1", "currentStep": "implement" } ],
      "runs": { "r1": {
        "runSlug": "r1",
        "startedAt": "2026-11-13T11:30:00Z",
        "currentStep": "implement",
        "currentStepStartedAt": "2026-11-13T11:59:00Z",
        "stepLogs": [ { "step": "plan", "status": "completed", "timestamp": "2026-11-13T11:40:00Z" } ]
      } }
    }),
  );
  let mut client = mock_client(mock_config(directory.path(), &state, None));

  let snapshot = tool_set(&mut client, directory.path()).read_snapshot(None).await.expect("状態を読み取れます");
  let detail = tool_set(&mut client, directory.path()).read_run_detail("r1").await.expect("run 詳細を読み取れます");

  assert_eq!(
    detail.last_log_at,
    Some(time("2026-11-13T11:59:00Z")),
    "詳細は lastLogAt が無ければ現在ステップの開始時刻を使います"
  );
  assert_eq!(
    snapshot.runs[0].last_log_at,
    Some(time("2026-11-13T11:59:00Z")),
    "一覧も現在ステップの開始時刻を使います"
  );
  client.shutdown().await;
}

#[tokio::test]
async fn 一覧と詳細の最終ログ時刻はステップ記録と現在ステップの開始時刻の新しい方で一致する() {
  // lastLogAt が無く、ステップ記録が現在ステップの開始時刻より新しいとき、一覧と詳細の
  // 最終ログ時刻が同じ値になることを観測します。片方だけが現在ステップの開始時刻に落ちると
  // 無音判定が食い違います。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(
    directory.path(),
    json!({
      "tasks": [ { "name": "implement", "goalId": "g1", "status": "running", "startedAt": "2026-11-13T10:00:00Z", "runSlug": "r1", "currentStep": "implement" } ],
      "runs": { "r1": {
        "runSlug": "r1",
        "startedAt": "2026-11-13T10:00:00Z",
        "currentStep": "implement",
        "currentStepStartedAt": "2026-11-13T10:02:00Z",
        "stepLogs": [ { "step": "implement", "timestamp": "2026-11-13T10:05:00Z" } ]
      } }
    }),
  );
  let mut client = mock_client(mock_config(directory.path(), &state, None));

  let snapshot = tool_set(&mut client, directory.path()).read_snapshot(None).await.expect("状態を読み取れます");
  let detail = tool_set(&mut client, directory.path()).read_run_detail("r1").await.expect("run 詳細を読み取れます");

  assert_eq!(
    snapshot.runs[0].last_log_at,
    Some(time("2026-11-13T10:05:00Z")),
    "一覧は現在ステップの開始時刻より新しいステップ記録を最終ログ時刻に採ります"
  );
  assert_eq!(
    detail.last_log_at,
    Some(time("2026-11-13T10:05:00Z")),
    "詳細も同じステップ記録を最終ログ時刻に採り、一覧と一致します"
  );
  client.shutdown().await;
}

#[tokio::test]
async fn 一覧と詳細の最終ログ時刻は複数ページの末尾ステップで一致する() {
  // 51 ステップの run。lastLogAt と currentStepStartedAt が無いとき、先頭ページだけでなく
  // 2 ページ目の末尾ステップの時刻を一覧と詳細の両方が採ることを観測します。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let mut step_logs = Vec::new();
  for index in 0..51 {
    let minute = index;
    step_logs.push(json!({ "step": format!("step{index}"), "timestamp": format!("2026-11-13T10:{minute:02}:00Z") }));
  }
  let state = write_state(
    directory.path(),
    json!({
      "tasks": [ { "name": "implement", "goalId": "g1", "status": "running", "startedAt": "2026-11-13T10:00:00Z", "runSlug": "r1", "currentStep": "step50" } ],
      "runs": { "r1": {
        "runSlug": "r1",
        "startedAt": "2026-11-13T10:00:00Z",
        "currentStep": "step50",
        "stepLogs": step_logs
      } }
    }),
  );
  let mut client = mock_client(mock_config(directory.path(), &state, None));

  let snapshot = tool_set(&mut client, directory.path()).read_snapshot(None).await.expect("状態を読み取れます");
  let detail = tool_set(&mut client, directory.path()).read_run_detail("r1").await.expect("run 詳細を読み取れます");

  assert_eq!(
    snapshot.runs[0].last_log_at,
    Some(time("2026-11-13T10:50:00Z")),
    "一覧は末尾ページの最終ステップを最終ログ時刻に採ります"
  );
  assert_eq!(
    detail.last_log_at,
    Some(time("2026-11-13T10:50:00Z")),
    "詳細も末尾ページの最終ステップを採り、一覧と一致します"
  );
  client.shutdown().await;
}

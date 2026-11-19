//! mock MCP サーバを使い、子プロセスの起動・ハンドシェイク・往復・isError・タイムアウト・
//! プロセス死亡からの再起動・応答の解析を結合的に検証する。

mod common;

use std::io::Write;

use common::*;
use serde_json::{json, Value};
use takt_dash_backend::{
  domain::{GoalId, InquiryId, MergeUntil, PullRequestNumber, RegistrationRevision},
  mcp::{CallError, ToolError, TransportError},
  model::{
    ExecutionStatus, GoalEntry, GoalStatus, InquiryEntry, InquiryKind, NotificationEntry, ProjectSnapshot, TaskStatus,
  },
};
use tokio::time::timeout;

fn goal_id(value: &str) -> GoalId {
  GoalId::try_from(value).expect("テストのゴール ID は有効です")
}

fn inquiry_id(value: &str) -> InquiryId {
  InquiryId::try_from(value).expect("テストの inquiry ID は有効です")
}

fn revision(value: i32) -> RegistrationRevision {
  RegistrationRevision::try_from(value).expect("テストの revision は正です")
}

fn pull_request(value: i64) -> PullRequestNumber {
  PullRequestNumber::try_from(value).expect("テストの PR 番号は正です")
}

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

/// 前回値として引き継ぐ、1 ゴールとその問い合わせを持つスナップショットを作る。
fn previous_goal_question_snapshot() -> ProjectSnapshot {
  ProjectSnapshot {
    goals: vec![GoalEntry {
      goal_id:             GoalId::try_from("g1").expect("テストのゴール ID は有効です"),
      status:              GoalStatus::Created,
      execution_status:    ExecutionStatus::Active,
      objective:           "ゴール g1".to_owned(),
      objective_truncated: false,
      integration_branch:  "takt/g1".to_owned(),
      pull_request_number: None,
      pull_request_url:    None,
      awaiting_merge_at:   None,
      paused_reason:       None,
    }],
    inquiries: vec![InquiryEntry {
      inquiry_id: InquiryId::try_from("q1").expect("テストの inquiry ID は有効です"),
      goal_id:    Some(GoalId::try_from("g1").expect("テストのゴール ID は有効です")),
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
      goal_id:             GoalId::try_from("g1").expect("テストのゴール ID は有効です"),
      status:              GoalStatus::AwaitingMerge,
      execution_status:    ExecutionStatus::Active,
      objective:           "ゴール g1".to_owned(),
      objective_truncated: false,
      integration_branch:  "takt/g1".to_owned(),
      pull_request_number: Some(PullRequestNumber::try_from(5_i64).expect("テストのプルリクエスト番号は有効です")),
      pull_request_url:    Some("https://example.test/pr/5".to_owned()),
      awaiting_merge_at:   Some(time("2026-11-13T02:00:00Z")),
      paused_reason:       None,
    }],
    notifications: vec![NotificationEntry {
      notification_id: "n-merge".to_owned(),
      goal_id:         GoalId::try_from("g1").expect("テストのゴール ID は有効です"),
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
      inquiry_id: InquiryId::try_from("s1").expect("テストの inquiry ID は有効です"),
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
      .answer_goal_question(
        &goal_id("3f9c1a54-2b41-4d1e-9d2e-2c1a5b8f7e11"),
        &inquiry_id("q1"),
        json!({ "text": "はい" }),
      )
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
      .approve_goal_registration(revision(3))
      .await
      .map(|_| ())
      .expect_err("応答がなければタイムアウトします");
    client.shutdown().await;
    errors.push(("takt_approve_goal_registration", error));
  }
  {
    let mut client = mock_client(mock_config_with_timeout(cwd, &state, Some("hang"), 1));
    let error = tool_set(&mut client, cwd)
      .reject_goal_registration(revision(3))
      .await
      .map(|_| ())
      .expect_err("応答がなければタイムアウトします");
    client.shutdown().await;
    errors.push(("takt_reject_goal_registration", error));
  }
  {
    let mut client = mock_client(mock_config_with_timeout(cwd, &state, Some("hang"), 1));
    let error = tool_set(&mut client, cwd)
      .merge_goal_pr(&goal_id("g1"), pull_request(42), MergeUntil::Merge)
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
      ]
    }),
  );
  let mut client = mock_client(mock_config(directory.path(), &state, None));

  let snapshot = tool_set(&mut client, directory.path())
    .read_snapshot(None, &run_logs(directory.path()))
    .await
    .expect("状態を読み取れます");

  assert_eq!(snapshot.goals[0].status, GoalStatus::AwaitingMerge);
  assert_eq!(snapshot.goals[0].pull_request_number, Some(pull_request(12)));
  assert_eq!(snapshot.tasks[0].status, TaskStatus::Running);
  assert!(snapshot.tasks[1].merge_task, "仕上げタスクを読み取ります");
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

  let snapshot = tool_set(&mut client, directory.path())
    .read_snapshot(None, &run_logs(directory.path()))
    .await
    .expect("状態を読み取れます");

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

  let snapshot = tool_set(&mut client, directory.path())
    .read_snapshot(None, &run_logs(directory.path()))
    .await
    .expect("状態を読み取れます");

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

  let snapshot = tool_set(&mut client, directory.path())
    .read_snapshot(None, &run_logs(directory.path()))
    .await
    .expect("状態を読み取れます");

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

  let snapshot = tool_set(&mut client, directory.path())
    .read_snapshot(Some(&prev), &run_logs(directory.path()))
    .await
    .expect("状態を読み取れます");

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

  let snapshot = tool_set(&mut client, directory.path())
    .read_snapshot(Some(&prev), &run_logs(directory.path()))
    .await
    .expect("状態を読み取れます");

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

  let snapshot = tool_set(&mut client, directory.path())
    .read_snapshot(Some(&prev), &run_logs(directory.path()))
    .await
    .expect("状態を読み取れます");

  let g2 = snapshot.goals.iter().find(|goal| goal.goal_id.as_str() == "g2").expect("g2 を含みます");
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

  let snapshot = tool_set(&mut client, directory.path())
    .read_snapshot(None, &run_logs(directory.path()))
    .await
    .expect("状態を読み取れます");

  assert_eq!(snapshot.goals.len(), 60, "50 件を超えるゴールも全件集めます");
  assert_eq!(snapshot.inquiries.len(), 60, "50 件を超えるゴール質問も全件集めます");
  let last =
    snapshot.inquiries.iter().find(|inquiry| inquiry.inquiry_id.as_str() == "q59").expect("2 ページ目の質問 g59");
  assert_eq!(last.goal_id.as_ref().map(GoalId::as_str), Some("g59"));
  assert_eq!(
    snapshot.goals.iter().find(|goal| goal.goal_id.as_str() == "g0").and_then(|goal| goal.awaiting_merge_at),
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

  let snapshot = tool_set(&mut client, directory.path())
    .read_snapshot(None, &run_logs(directory.path()))
    .await
    .expect("createdAt 欠落でも解析できます");

  assert!(
    snapshot.inquiries.iter().any(|inquiry| inquiry.inquiry_id.as_str() == "q-x"),
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
      inquiry_id: inquiry_id("q-x"),
      goal_id:    Some(goal_id("g1")),
      kind:       takt_dash_backend::model::InquiryKind::Question,
      body:       "どうしますか".to_owned(),
      options:    Vec::new(),
      answered:   false,
      created_at: time("2026-11-13T01:00:00Z"),
    }],
    ..takt_dash_backend::model::ProjectSnapshot::empty()
  };

  let snapshot = tool_set(&mut client, directory.path())
    .read_snapshot(Some(&prev), &run_logs(directory.path()))
    .await
    .expect("前回値を引き継げます");

  let inquiry = snapshot.inquiries.iter().find(|entry| entry.inquiry_id.as_str() == "q-x").expect("q-x を含みます");
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
  let snapshot = tool_set(&mut client, directory.path())
    .read_snapshot(None, &run_logs(directory.path()))
    .await
    .expect("approval を読み飛ばせます");
  assert!(snapshot.inquiries.iter().all(|entry| entry.inquiry_id.as_str() != "q-a"), "approval は判断待ちに含めません");
  assert!(snapshot.inquiries.iter().any(|entry| entry.inquiry_id.as_str() == "q-q"), "question は含めます");
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
  let error = tool_set(&mut client2, directory2.path())
    .read_snapshot(None, &run_logs(directory2.path()))
    .await
    .expect_err("未知の種類は拒否します");
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
    tools.approve_goal_registration(revision(3)).await.map(|_| ())
  })
  .await
  .expect_err("応答に revision が無ければ解析エラーです");
  assert!(matches!(error, ToolError::Parse(_)), "takt_approve_goal_registration の欠落は解析エラー: {error}");

  let error = expect_parse("takt_reject_goal_registration", async |mut tools| {
    tools.reject_goal_registration(revision(3)).await.map(|_| ())
  })
  .await
  .expect_err("応答に revision が無ければ解析エラーです");
  assert!(matches!(error, ToolError::Parse(_)), "takt_reject_goal_registration の欠落は解析エラー: {error}");

  let error = expect_parse("takt_merge_goal_pr", async |mut tools| {
    tools.merge_goal_pr(&goal_id("g1"), pull_request(42), MergeUntil::Merge).await.map(|_| ())
  })
  .await
  .expect_err("応答に taskName が無ければ解析エラーです");
  assert!(matches!(error, ToolError::Parse(_)), "takt_merge_goal_pr の欠落は解析エラー: {error}");

  let error = expect_parse("takt_answer_goal_question", async |mut tools| {
    tools
      .answer_goal_question(
        &goal_id("3f9c1a54-2b41-4d1e-9d2e-2c1a5b8f7e11"),
        &inquiry_id("q1"),
        json!({ "text": "はい" }),
      )
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
async fn write_tool_responses_convert_task_name_and_inquiry_id() {
  // takt 応答の taskName と questionId を専用の型へ変換し、有効値は保持し、
  // 空・空白のみ・ASCII 制御文字は解析エラーにすることを観測する。
  async fn with_responses(
    responses: Value,
    behavior: impl AsyncFnOnce(takt_dash_backend::mcp::ToolSet<'_>) -> Result<(), ToolError>,
  ) -> Result<(), ToolError> {
    let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
    let state = write_state(directory.path(), json!({ "responses": responses }));
    let mut client = mock_client(mock_config(directory.path(), &state, None));
    let result = behavior(tool_set(&mut client, directory.path())).await;
    client.shutdown().await;
    result
  }

  with_responses(
    json!({ "takt_merge_goal_pr": { "taskName": "merge", "workflow": "merge-goal-pr" } }),
    async |mut tools| {
      let result = tools.merge_goal_pr(&goal_id("g1"), pull_request(42), MergeUntil::Merge).await?;
      assert_eq!(result.task_name.as_str(), "merge", "有効な taskName を保持します");
      Ok(())
    },
  )
  .await
  .expect("有効な taskName は受理します");

  for invalid in ["", "  ", "a\n"] {
    let error = with_responses(
      json!({ "takt_merge_goal_pr": { "taskName": invalid, "workflow": "merge-goal-pr" } }),
      async |mut tools| tools.merge_goal_pr(&goal_id("g1"), pull_request(42), MergeUntil::Merge).await.map(|_| ()),
    )
    .await
    .expect_err("不正な taskName は解析エラーです");
    assert!(matches!(error, ToolError::Parse(_)), "taskName {invalid:?} は解析エラー: {error}");
  }

  with_responses(json!({ "takt_answer_goal_question": { "questionId": "q-1" } }), async |mut tools| {
    let question_id = tools
      .answer_goal_question(
        &goal_id("3f9c1a54-2b41-4d1e-9d2e-2c1a5b8f7e11"),
        &inquiry_id("q-1"),
        json!({ "text": "回答" }),
      )
      .await?;
    assert_eq!(question_id.as_str(), "q-1", "有効な questionId を保持します");
    Ok(())
  })
  .await
  .expect("有効な questionId は受理します");

  for invalid in ["", "  ", "q\n1"] {
    let error = with_responses(json!({ "takt_answer_goal_question": { "questionId": invalid } }), async |mut tools| {
      tools
        .answer_goal_question(
          &goal_id("3f9c1a54-2b41-4d1e-9d2e-2c1a5b8f7e11"),
          &inquiry_id("q-1"),
          json!({ "text": "回答" }),
        )
        .await
        .map(|_| ())
    })
    .await
    .expect_err("不正な questionId は解析エラーです");
    assert!(matches!(error, ToolError::Parse(_)), "questionId {invalid:?} は解析エラー: {error}");
  }
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
  assert_eq!(proposal.revision.get(), 6, "revision を写します");
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
      revision:             revision(5),
      repository_directory: "/abs/takt-dash".to_owned(),
      summary:              "前回の提案".to_owned(),
    }],
    ..takt_dash_backend::model::ProjectSnapshot::empty()
  };

  let snapshot = tool_set(&mut client, directory.path())
    .read_snapshot(Some(&prev), &run_logs(directory.path()))
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
    .read_snapshot(None, &run_logs(directory.path()))
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
      revision:             revision(5),
      repository_directory: "/abs/takt-dash".to_owned(),
      summary:              "前回の提案".to_owned(),
    }],
    ..takt_dash_backend::model::ProjectSnapshot::empty()
  };

  let error = tool_set(&mut client, directory.path())
    .read_snapshot(Some(&prev), &run_logs(directory.path()))
    .await
    .expect_err("輸送エラーは縮退せず伝播します");

  assert!(matches!(error, ToolError::Transport(_)), "輸送エラーは解析失敗と区別します: {error}");
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
      inquiry_id: inquiry_id("q1"),
      goal_id:    Some(goal_id("g1")),
      kind:       takt_dash_backend::model::InquiryKind::Question,
      body:       "g1 の質問（前回）".to_owned(),
      options:    Vec::new(),
      answered:   false,
      created_at: time("2026-11-13T01:00:00Z"),
    }],
    ..takt_dash_backend::model::ProjectSnapshot::empty()
  };

  let snapshot = tool_set(&mut client, directory.path())
    .read_snapshot(Some(&prev), &run_logs(directory.path()))
    .await
    .expect("g1 の質問の道具エラーで全体は失敗しません");

  let q1 = snapshot
    .inquiries
    .iter()
    .find(|entry| entry.inquiry_id.as_str() == "q1")
    .expect("道具エラーの g1 の質問を引き継ぎます");
  assert_eq!(q1.goal_id.as_ref().map(GoalId::as_str), Some("g1"));
  assert_eq!(q1.body, "g1 の質問（前回）", "g1 の質問は前回値を引き継ぎます");
  assert_eq!(q1.created_at, time("2026-11-13T01:00:00Z"), "g1 の質問の created_at も前回値を保ちます");
  let q2 =
    snapshot.inquiries.iter().find(|entry| entry.inquiry_id.as_str() == "q2").expect("健常な g2 の質問は今回値です");
  assert_eq!(q2.goal_id.as_ref().map(GoalId::as_str), Some("g2"));
  assert_eq!(q2.body, "g2 の質問", "健常なゴールの質問は今回値を採ります");
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
    .read_snapshot(Some(&previous), &run_logs(directory.path()))
    .await
    .expect("ゴール質問のタイムアウトはプロジェクト全体を失敗にしません");

  let inquiry =
    snapshot.inquiries.iter().find(|entry| entry.inquiry_id.as_str() == "q1").expect("前回の問い合わせを引き継ぎます");
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
    .read_snapshot(Some(&previous), &run_logs(directory.path()))
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
    .read_snapshot(Some(&previous), &run_logs(directory.path()))
    .await
    .expect("ゴールに属さない問い合わせのタイムアウトはプロジェクト全体を失敗にしません");

  let inquiry = snapshot
    .inquiries
    .iter()
    .find(|entry| entry.inquiry_id.as_str() == "s1")
    .expect("前回のゴールに属さない問い合わせを引き継ぎます");
  assert_eq!(inquiry.body, "前回の作業依頼", "タイムアウトした問い合わせは前回値です");
  client.shutdown().await;
}

/// running の r1 タスクと、takt_get_workflow が返す workflow `wf` の step 名一覧を持つ状態。
fn running_task_with_workflow(workflow_steps: &[&str]) -> serde_json::Value {
  json!({
    "tasks": [ { "name": "a", "status": "running", "runSlug": "r1", "startedAt": "2026-11-13T10:00:00Z" } ],
    "workflows": { "wf": workflow_steps }
  })
}

/// プロジェクトの cwd に、対応表に r1 を載せた実行中の run の保存場所を作る。
fn write_running_run(project_cwd: &std::path::Path, current_step: &str, current_iteration: i64, records: &[Value]) {
  let fixture = RunFixture::new(project_cwd);
  fixture.write_tasks(&[RunTaskLine { run_slug: Some("r1"), source_run_slug: None }]);
  fixture.write_meta("r1", &running_meta(current_step, current_iteration, "wf", "2026-11-13T10:00:00Z"));
  let mut all = vec![workflow_start_record("wf")];
  all.extend_from_slice(records);
  fixture.append_records("r1", &all);
}

#[tokio::test]
async fn run_step_progress_is_derived_from_the_workflow_step_list() {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state =
    write_state(directory.path(), running_task_with_workflow(&["plan", "implement", "review", "fix", "close"]));
  write_running_run(directory.path(), "implement", 2, &[step_complete_record(
    "plan",
    "architect",
    1,
    "2026-11-13T10:20:00Z",
  )]);
  let mut client = mock_client(mock_config(directory.path(), &state, None));

  let snapshot = tool_set(&mut client, directory.path())
    .read_snapshot(None, &run_logs(directory.path()))
    .await
    .expect("状態を読み取れます");

  let run = &snapshot.runs[0];
  assert_eq!(run.current_step_number, Some(2), "現在ステップの位置を workflow の並びから求めます");
  assert_eq!(run.total_steps, Some(5), "workflow の全ステップ数を求めます");
  assert_eq!(run.current_iteration, Some(2), "meta.json の currentIteration を読み取ります");
  client.shutdown().await;
}

#[tokio::test]
async fn run_step_position_uses_the_current_step_even_when_the_last_log_is_a_nested_step() {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state =
    write_state(directory.path(), running_task_with_workflow(&["develop", "machine_gate", "fix_machine_gate"]));
  write_running_run(directory.path(), "develop", 3, &[step_complete_record(
    "write_tests",
    "coder",
    3,
    "2026-11-13T10:20:00Z",
  )]);
  let mut client = mock_client(mock_config(directory.path(), &state, None));

  let snapshot = tool_set(&mut client, directory.path())
    .read_snapshot(None, &run_logs(directory.path()))
    .await
    .expect("状態を読み取れます");

  let run = &snapshot.runs[0];
  assert_eq!(run.current_step_number, Some(1), "入れ子 workflow のログではなく currentStep を位置に使います");
  assert_eq!(run.total_steps, Some(3), "workflow の全ステップ数を求めます");
  assert_eq!(run.current_iteration, Some(3), "meta.json の currentIteration を読み取ります");
  assert_eq!(run.current_step.as_deref(), Some("develop"), "今のステップ名は currentStep から読み取ります");
  assert_eq!(
    run.steps.last().map(|step| step.step_name.as_str()),
    Some("write_tests"),
    "ステップの記録の末尾は入れ子 workflow の内側の step で、今のトップレベル step とは異なります"
  );
  client.shutdown().await;
}

#[tokio::test]
async fn run_step_position_is_unset_when_the_current_step_is_not_in_the_workflow() {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(directory.path(), running_task_with_workflow(&["plan", "review"]));
  write_running_run(directory.path(), "implement", 2, &[]);
  let mut client = mock_client(mock_config(directory.path(), &state, None));

  let snapshot = tool_set(&mut client, directory.path())
    .read_snapshot(None, &run_logs(directory.path()))
    .await
    .expect("状態を読み取れます");

  let run = &snapshot.runs[0];
  assert_eq!(run.current_step_number, None, "workflow に無いステップの位置を捏造しません");
  assert_eq!(run.total_steps, None, "位置が決まらない run は総数も設定しません");
  assert_eq!(run.current_iteration, Some(2), "反復回数は位置と独立して保持します");
  client.shutdown().await;
}

#[tokio::test]
async fn workflow_timeout_keeps_the_previous_run_without_failing_the_project() {
  // ステップ位置を補う takt_get_workflow がタイムアウトしても、プロジェクトの読み取りは成功し、
  // その run だけ前回値になる。補完の失敗をそのまま返す実装は Timeout の輸送エラーで落ちる。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let mut state = running_task_with_workflow(&["plan", "implement"]);
  state["hang"] = json!(["takt_get_workflow"]);
  let state = write_state(directory.path(), state);
  write_running_run(directory.path(), "implement", 2, &[]);
  let mut client = mock_client(mock_config_with_timeout(directory.path(), &state, None, 1));
  let previous =
    ProjectSnapshot { runs: vec![run("r1", "a", time("2026-11-13T09:30:00Z"))], ..ProjectSnapshot::empty() };

  let snapshot = tool_set(&mut client, directory.path())
    .read_snapshot(Some(&previous), &run_logs(directory.path()))
    .await
    .expect("workflow のタイムアウトでプロジェクト全体を失敗させません");

  assert_eq!(snapshot.runs, previous.runs, "タイムアウトした run は前回値です");
  client.shutdown().await;
}

#[tokio::test]
async fn workflow_timeout_without_a_previous_run_leaves_the_run_out() {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let mut state = running_task_with_workflow(&["plan", "implement"]);
  state["hang"] = json!(["takt_get_workflow"]);
  let state = write_state(directory.path(), state);
  write_running_run(directory.path(), "implement", 2, &[]);
  let mut client = mock_client(mock_config_with_timeout(directory.path(), &state, None, 1));

  let snapshot = tool_set(&mut client, directory.path())
    .read_snapshot(None, &run_logs(directory.path()))
    .await
    .expect("workflow のタイムアウトでプロジェクト全体を失敗させません");

  assert!(snapshot.runs.is_empty(), "前回値の無い run は一覧から外れます: {:?}", snapshot.runs);
  client.shutdown().await;
}

#[tokio::test]
async fn workflow_tool_error_keeps_the_run_without_step_progress() {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let mut state = running_task_with_workflow(&["plan", "implement"]);
  state["isError"] = json!(["takt_get_workflow:wf"]);
  let state = write_state(directory.path(), state);
  write_running_run(directory.path(), "implement", 2, &[]);
  let mut client = mock_client(mock_config(directory.path(), &state, None));

  let snapshot = tool_set(&mut client, directory.path())
    .read_snapshot(None, &run_logs(directory.path()))
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
  assert_eq!(proposals[0].revision.get(), i32::MAX, "i32 の上限をそのまま返します");
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
async fn a_run_meta_without_a_status_leaves_the_run_out_instead_of_listing_it_as_running() {
  // meta.json に status が無い run は Running に丸めず、その run
  // だけ一覧に入れない（前回値も無い）。 run の読み取りの失敗はプロジェクトの失敗にしない。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(directory.path(), running_task_with_workflow(&["implement"]));
  let fixture = RunFixture::new(directory.path());
  fixture.write_tasks(&[RunTaskLine { run_slug: Some("r1"), source_run_slug: None }]);
  let mut meta = running_meta("implement", 1, "wf", "2026-11-13T10:00:00Z");
  meta.as_object_mut().expect("meta はオブジェクトです").remove("status");
  fixture.write_meta("r1", &meta);
  let mut client = mock_client(mock_config(directory.path(), &state, None));

  let snapshot = tool_set(&mut client, directory.path())
    .read_snapshot(None, &run_logs(directory.path()))
    .await
    .expect("run の状態の欠落でプロジェクト全体は失敗しません");

  assert!(snapshot.runs.is_empty(), "欠落した run の状態を Running に丸めて一覧へ入れません: {:?}", snapshot.runs);
  client.shutdown().await;
}

#[tokio::test]
async fn a_run_meta_with_an_unknown_status_leaves_the_run_out() {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(directory.path(), running_task_with_workflow(&["implement"]));
  let fixture = RunFixture::new(directory.path());
  fixture.write_tasks(&[RunTaskLine { run_slug: Some("r1"), source_run_slug: None }]);
  let mut meta = running_meta("implement", 1, "wf", "2026-11-13T10:00:00Z");
  meta["status"] = json!("queued");
  fixture.write_meta("r1", &meta);
  let mut client = mock_client(mock_config(directory.path(), &state, None));

  let snapshot = tool_set(&mut client, directory.path())
    .read_snapshot(None, &run_logs(directory.path()))
    .await
    .expect("未知の run の状態でプロジェクト全体は失敗しません");

  assert!(snapshot.runs.is_empty(), "未知の run の状態を Running に丸めて一覧へ入れません: {:?}", snapshot.runs);
  client.shutdown().await;
}

#[tokio::test]
async fn a_non_positive_pull_request_number_is_a_parse_error() {
  for number in [0, -1] {
    let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
    let state = write_state(
      directory.path(),
      json!({
        "goals": [ {
          "id": "g1",
          "status": "awaiting_merge",
          "executionStatus": "active",
          "objective": "目標",
          "integrationBranch": "takt/g1",
          "pullRequest": { "number": number, "url": "https://example.test/pr/1" }
        } ]
      }),
    );
    let mut client = mock_client(mock_config(directory.path(), &state, None));

    let error = tool_set(&mut client, directory.path())
      .read_snapshot(None, &run_logs(directory.path()))
      .await
      .expect_err("0 以下のプルリクエスト番号は解析エラーです");

    assert!(
      matches!(error, ToolError::Parse(_)),
      "プルリクエスト番号 {number} は正の整数でなければなりません: {error}"
    );
    client.shutdown().await;
  }
}

#[tokio::test]
async fn slow_initialize_is_not_bound_by_the_request_timeout() {
  // initialize の応答が呼び出しの期限より遅くても、初期化の期限（60秒）の内側なら
  // 接続できる。呼び出しの期限で初期化を打ち切ると、高負荷のときに起動が終わらない。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(directory.path(), json!({ "goals": [] }));
  let mut config = mock_config_with_timeout(directory.path(), &state, None, 2);
  config.args.push("--initialize-delay-ms".to_owned());
  config.args.push("5000".to_owned());
  let mut client = mock_client(config);

  let text = client
    .call_tool("takt_list_goals", with_cwd(directory.path(), json!({})))
    .await
    .expect("初期化が遅くても道具を呼べます");

  assert!(text.contains("\"goals\""), "応答の本文テキストを返します: {text}");
  client.shutdown().await;
}

#[tokio::test]
async fn concurrent_spawns_stay_within_the_cap() {
  // 4本のクライアントが同時に起動しても、起動と初期化が重なるのは3本まで。
  // spawn-log の start から ready までの区間が重なる最大数を数える。
  // セマフォはプロセス全体で共有されるため、並行して走る他のテストも同じ3枠を使う。
  // 本数を増やしたり遅延を長くしたりすると、他のテストの起動が待たされて時間の
  // 表明が崩れるため、本数と遅延は小さく保つ。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(directory.path(), json!({ "goals": [] }));
  let spawn_log = directory.path().join("spawn.log");
  let mut handles = Vec::new();
  for _ in 0..4 {
    let mut config = mock_config_with_timeout(directory.path(), &state, None, 10);
    config.args.push("--initialize-delay-ms".to_owned());
    config.args.push("1500".to_owned());
    config.args.push("--spawn-log".to_owned());
    config.args.push(spawn_log.display().to_string());
    let cwd = directory.path().to_path_buf();
    handles.push(tokio::spawn(async move {
      let mut client = mock_client(config);
      client.call_tool("takt_list_goals", with_cwd(&cwd, json!({}))).await.expect("道具を呼べます");
      client.shutdown().await;
    }));
  }
  for handle in handles {
    handle.await.expect("起動のタスクが完了します");
  }

  let log = std::fs::read_to_string(&spawn_log).expect("起動の記録を読めます");
  let mut starts = std::collections::BTreeMap::new();
  let mut intervals = Vec::new();
  for line in log.lines() {
    let parts: Vec<&str> = line.split_whitespace().collect();
    assert_eq!(parts.len(), 3, "起動の記録は3列です: {line}");
    let at: i64 = parts[2].parse().expect("時刻はミリ秒です");
    match parts[0] {
      | "start" => {
        starts.insert(parts[1].to_owned(), at);
      },
      | "ready" => {
        let start = starts.remove(parts[1]).expect("ready に対応する start があります");
        intervals.push((start, at));
      },
      | other => panic!("未知の起動記録です: {other}"),
    }
  }
  assert_eq!(intervals.len(), 4, "4本とも初期化を終えます: {log}");
  let mut events = Vec::new();
  for (start, ready) in &intervals {
    events.push((*start, 1));
    events.push((*ready, -1));
  }
  events.sort();
  let mut current = 0;
  let mut maximum = 0;
  for (_, delta) in events {
    current += delta;
    maximum = maximum.max(current);
  }
  assert!(maximum <= 3, "同時に初期化が進むのは3本までです: 最大 {maximum}");
}

#[tokio::test]
async fn failed_spawn_backs_off_instead_of_respawning_immediately() {
  // 起動に失敗した直後の呼び出しは、子プロセスを起動せずに抑制のエラーを返す。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(directory.path(), json!({}));
  let spawn_log = directory.path().join("spawn.log");
  let mut config = mock_config(directory.path(), &state, Some("exit"));
  config.args.push("--spawn-log".to_owned());
  config.args.push(spawn_log.display().to_string());
  let mut client = mock_client(config);

  let first = transport(
    client.call_tool("takt_list_goals", with_cwd(directory.path(), json!({}))).await.expect_err("起動失敗はエラーです"),
  );
  assert!(!matches!(first, TransportError::Backoff(_)), "最初の失敗は抑制ではありません: {first}");

  let started = std::time::Instant::now();
  let second = transport(
    client.call_tool("takt_list_goals", with_cwd(directory.path(), json!({}))).await.expect_err("抑制中もエラーです"),
  );
  assert!(matches!(second, TransportError::Backoff(_)), "2回目は抑制のエラーです: {second}");
  assert!(started.elapsed() < std::time::Duration::from_secs(2), "抑制は待たずに返ります");

  let spawned = std::fs::read_to_string(&spawn_log).expect("起動の記録を読めます");
  assert_eq!(spawned.lines().count(), 1, "抑制中は子プロセスを起動しません: {spawned}");
  client.shutdown().await;
}

#[test]
fn client_config_gives_the_list_reads_a_longer_timeout() {
  // 一覧読み取り（ゴール・タスク）だけが全体の期限（15秒）より長い60秒の期限を持ち、
  // それ以外の道具には個別の期限が付かないことを、実測に基づく設定として固定する。
  let server = mcp_server();
  let config = takt_dash_backend::mcp::client_config(&server, std::path::Path::new("/tmp"));

  assert_eq!(config.request_timeout, std::time::Duration::from_secs(15), "全体の期限は15秒のままです");
  assert_eq!(
    config.tool_timeouts.get("takt_list_goals"),
    Some(&std::time::Duration::from_secs(60)),
    "ゴール一覧の期限は60秒です"
  );
  assert_eq!(
    config.tool_timeouts.get("takt_list_tasks"),
    Some(&std::time::Duration::from_secs(60)),
    "タスク一覧の期限は60秒です"
  );
  assert_eq!(config.tool_timeouts.len(), 2, "期限を広げるのは一覧読み取りだけです");
}

#[tokio::test]
async fn per_tool_timeout_applies_to_the_configured_tool_only() {
  // 道具ごとの期限を設定した道具は、全体の期限より遅い応答でも成功する。
  // 設定していない道具は全体の期限で時間切れする。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(
    directory.path(),
    json!({
      "goals": [ { "id": "g1", "status": "created", "executionStatus": "active", "objective": "目標", "integrationBranch": "takt/g1" } ],
      "delays": { "takt_list_goals": 1500, "takt_list_tasks": 1500 }
    }),
  );
  let mut config = mock_config_with_timeout(directory.path(), &state, None, 1);
  config.tool_timeouts.insert("takt_list_goals".to_owned(), std::time::Duration::from_secs(3));
  let mut client = mock_client(config);

  let text = client
    .call_tool("takt_list_goals", with_cwd(directory.path(), json!({})))
    .await
    .expect("道具ごとの期限があれば全体の期限より遅い応答でも成功します");
  assert!(text.contains("\"goals\""), "応答の本文テキストを返します: {text}");

  let error = transport(
    client
      .call_tool("takt_list_tasks", with_cwd(directory.path(), json!({})))
      .await
      .expect_err("道具ごとの期限が無い道具は全体の期限で時間切れします"),
  );
  assert!(matches!(error, TransportError::Timeout(1)), "全体の期限（1秒）で時間切れします: {error}");
  client.shutdown().await;
}

#[tokio::test]
async fn read_snapshot_retries_the_core_lists_once_after_a_timeout() {
  // コア読み取り（ゴール・タスクの一覧）が1回だけ時間切れしても、作り直した
  // 子プロセスでもう1回読み直して snapshot を組み立てる。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(
    directory.path(),
    json!({
      "goals": [ { "id": "g1", "status": "created", "executionStatus": "active", "objective": "目標", "integrationBranch": "takt/g1" } ],
      "tasks": [ { "name": "a", "goalId": "g1", "status": "pending" } ],
      "hangOnce": ["takt_list_goals", "takt_list_tasks"]
    }),
  );
  let mut client = mock_client(mock_config_with_timeout(directory.path(), &state, None, 1));

  let snapshot = tool_set(&mut client, directory.path())
    .read_snapshot(None, &run_logs(directory.path()))
    .await
    .expect("コア読み取りの1回目の時間切れは再試行で回復します");

  assert_eq!(snapshot.goals.len(), 1, "再試行でゴールを読めます");
  assert_eq!(snapshot.tasks.len(), 1, "再試行でタスクを読めます");
  client.shutdown().await;
}

#[tokio::test]
async fn read_snapshot_degrades_questions_and_notifications_on_timeout() {
  // 補助読み取り（ゴールに属さない問い合わせ・ゴールの質問・通知）が輸送の期限切れで
  // 失敗しても、その項目だけ前回値を引き継ぎ、snapshot 全体は成功する。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let state = write_state(
    directory.path(),
    json!({
      "goals": [ { "id": "g1", "status": "created", "executionStatus": "active", "objective": "目標", "integrationBranch": "takt/g1" } ],
      "standaloneInquiries": [ { "id": "s1", "status": "pending", "kind": "question", "body": "今回の単独の問い合わせ", "createdAt": "2026-11-13T09:00:00Z" } ],
      "goalQuestions": { "g1": [ { "id": "q1", "status": "pending", "kind": "question", "body": "今回の質問", "createdAt": "2026-11-13T09:00:00Z" } ] },
      "notifications": { "g1": [ { "id": "n1", "kind": "progress", "body": "今回の通知", "recordedAt": "2026-11-13T09:00:00Z" } ] },
      "delays": { "takt_list_standalone_inquiries": 2500, "takt_list_goal_questions": 2500, "takt_get_goal": 2500 }
    }),
  );
  let mut client = mock_client(mock_config_with_timeout(directory.path(), &state, None, 1));
  let prev = takt_dash_backend::model::ProjectSnapshot {
    inquiries: vec![
      takt_dash_backend::model::InquiryEntry {
        inquiry_id: inquiry_id("s1"),
        goal_id:    None,
        kind:       takt_dash_backend::model::InquiryKind::Question,
        body:       "前回の単独の問い合わせ".to_owned(),
        options:    Vec::new(),
        answered:   false,
        created_at: time("2026-11-13T01:00:00Z"),
      },
      takt_dash_backend::model::InquiryEntry {
        inquiry_id: inquiry_id("q1"),
        goal_id:    Some(goal_id("g1")),
        kind:       takt_dash_backend::model::InquiryKind::Question,
        body:       "前回の質問".to_owned(),
        options:    Vec::new(),
        answered:   false,
        created_at: time("2026-11-13T01:00:00Z"),
      },
    ],
    notifications: vec![notification("n1", "progress", time("2026-11-13T01:00:00Z"))],
    ..takt_dash_backend::model::ProjectSnapshot::empty()
  };

  let snapshot = tool_set(&mut client, directory.path())
    .read_snapshot(Some(&prev), &run_logs(directory.path()))
    .await
    .expect("補助読み取りの期限切れで全体は失敗しません");

  assert_eq!(snapshot.goals.len(), 1, "ゴールは今回値を読みます");
  let s1 =
    snapshot.inquiries.iter().find(|entry| entry.inquiry_id.as_str() == "s1").expect("単独の問い合わせを引き継ぎます");
  assert_eq!(s1.body, "前回の単独の問い合わせ", "期限切れの単独の問い合わせは前回値を引き継ぎます");
  let q1 =
    snapshot.inquiries.iter().find(|entry| entry.inquiry_id.as_str() == "q1").expect("ゴールの質問を引き継ぎます");
  assert_eq!(q1.body, "前回の質問", "期限切れのゴールの質問は前回値を引き継ぎます");
  assert_eq!(snapshot.notifications.len(), 1, "通知を引き継ぎます");
  assert_eq!(snapshot.notifications[0].body, "通知 n1", "期限切れの通知は前回値を引き継ぎます");
  client.shutdown().await;
}

#[tokio::test]
async fn read_snapshot_degrades_a_timed_out_run_to_the_previous_value() {
  // running な run の補助読み取りが期限切れでも、その run だけ前回値を引き継ぎ、
  // snapshot 全体は成功する。running な run は保存場所から読み、takt_get_run を呼ばないため、
  // ステップ位置を補う takt_get_workflow の応答を期限より遅らせて期限切れを起こす。
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let mut state = running_task_with_workflow(&["plan", "implement"]);
  state["delays"] = json!({ "takt_get_workflow": 2500 });
  let state = write_state(directory.path(), state);
  write_running_run(directory.path(), "implement", 2, &[step_complete_record(
    "plan",
    "architect",
    1,
    "2026-11-13T10:20:00Z",
  )]);
  let mut client = mock_client(mock_config_with_timeout(directory.path(), &state, None, 1));
  let prev = takt_dash_backend::model::ProjectSnapshot {
    runs: vec![run("r1", "a", time("2026-11-13T10:05:00Z"))],
    ..takt_dash_backend::model::ProjectSnapshot::empty()
  };

  let snapshot = tool_set(&mut client, directory.path())
    .read_snapshot(Some(&prev), &run_logs(directory.path()))
    .await
    .expect("run の期限切れで全体は失敗しません");

  assert_eq!(snapshot.runs.len(), 1, "期限切れの run は前回値で残ります");
  assert_eq!(snapshot.runs[0].last_log_at, Some(time("2026-11-13T10:05:00Z")), "期限切れの run は前回値です");
  client.shutdown().await;
}

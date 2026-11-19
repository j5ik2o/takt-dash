//! gRPC の GetRunUpdates が、run 詳細の取っ手（GetRunDetail と共有する、要求をまたいで差分を読む
//! 取っ手）から、利用者が送った続きの位置（世代と通し番号）より後の本体ログの行だけを返すことを
//! 検証する。続きを示せないとき（番号を送らない、世代が違う、workflow_start
//! で行が初期化された）は、 全体を置き換える指示（reset）と、GetRunDetail の log_tail
//! と同じ範囲の行を返す。 セッションログ（`logs/phase1-execution/*.
//! jsonl`）からの活動の印（直近の道具名、道具の呼び出しの 件数、更新時刻）も同じ応答で返す。
//! ローカルで読める run では takt-mcp は起動も呼び出しもしない。ローカルで続きを読めない
//! run は、takt_get_run の stepLogs から行を組み立てて返す。

mod common;
mod grpc_harness;

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use common::*;
use grpc_harness::{client, start_server, GrpcHarness};
use serde_json::{json, Value};
use takt_dash_backend::{
  domain::GoalId,
  model::{ExecutionStatus, GoalStatus, ProjectSnapshot},
  takt_dash::v1::{
    takt_dash_service_client::TaktDashServiceClient, GetOverviewRequest, GetRunDetailRequest, GetRunUpdatesRequest,
    GetRunUpdatesResponse,
  },
};
use tonic::transport::Channel;

const WORKFLOW: &str = "development-core";

fn project_dir() -> &'static Path {
  Path::new("/abs/takt-dash")
}

fn mcp_state() -> Value {
  json!({
    "tasks": [
      { "name": "implement", "goalId": "g1", "status": "running", "startedAt": "2026-11-13T08:30:00Z", "runSlug": "run-1", "currentStep": "implement" }
    ]
  })
}

/// 状態ストアに run-1 を持つ 1 プロジェクト分の土台を作る。
fn seeded_harness() -> GrpcHarness {
  let harness =
    GrpcHarness::builder().single_project(project("takt-dash", project_dir()), server_meta(), mcp_state()).build();
  let mut stored_run = run("run-1", "implement", time("2026-11-13T09:06:00Z"));
  stored_run.goal_id = Some(GoalId::try_from("g1").expect("テストのゴール ID は有効です"));
  let snapshot = ProjectSnapshot {
    goals: vec![goal("g1", GoalStatus::Created, ExecutionStatus::Active)],
    tasks: vec![],
    inquiries: vec![],
    runs: vec![stored_run],
    notifications: vec![],
    proposals: vec![],
    ..ProjectSnapshot::empty()
  };
  harness.store.record_success("takt-dash", snapshot, time("2026-11-13T12:00:00Z"));
  harness
}

/// 実行中の run-1 を、workflow_start、plan の step_complete、implement の step_start で作る。
fn running_run_files(harness: &GrpcHarness) -> RunFixture {
  let fixture = RunFixture::new(harness.project_dir());
  fixture.write_tasks(&[RunTaskLine { run_slug: Some("run-1"), source_run_slug: None }]);
  fixture.write_meta("run-1", &running_meta("implement", 1, WORKFLOW, "2026-11-13T09:00:00Z"));
  fixture.append_records("run-1", &[
    workflow_start_record(WORKFLOW),
    step_complete_record("plan", "architect", 1, "2026-11-13T09:01:00Z"),
    step_start_record("implement", WORKFLOW, 1, "2026-11-13T09:05:00Z", &["implement"]),
  ]);
  fixture
}

fn phase1_dir(fixture: &RunFixture, run_slug: &str) -> PathBuf {
  let directory = fixture.run_dir(run_slug).join("logs").join("phase1-execution");
  std::fs::create_dir_all(&directory).expect("phase1-execution を作れます");
  directory
}

/// takt のセッションログの道具の呼び出しの1行（実物の形に合わせた合成データ）。
fn tool_use_line(tool: &str) -> String {
  format!(
    "{}\n",
    json!({ "kind": "provider_event", "event": { "type": "tool_use", "data": { "tool": tool, "input": { "command": "ls" } } } })
  )
}

/// 道具の呼び出しではないセッションログの1行。
fn text_line(text: &str) -> String {
  format!("{}\n", json!({ "kind": "provider_event", "event": { "type": "text", "data": { "text": text } } }))
}

fn append_text(path: &Path, text: &str) {
  use std::io::Write;
  let mut file = std::fs::OpenOptions::new().append(true).create(true).open(path).expect("ファイルを開けます");
  file.write_all(text.as_bytes()).expect("ファイルへ追記できます");
}

fn set_modified(path: &Path, modified: DateTime<Utc>) {
  let file = std::fs::File::options().write(true).open(path).expect("ファイルを開けます");
  file.set_modified(std::time::SystemTime::from(modified)).expect("更新時刻を設定できます");
}

fn proto_timestamp(time: DateTime<Utc>) -> prost_types::Timestamp {
  prost_types::Timestamp { seconds: time.timestamp(), nanos: time.timestamp_subsec_nanos() as i32 }
}

fn request(run_slug: &str, cursor: Option<(i64, i64)>) -> GetRunUpdatesRequest {
  GetRunUpdatesRequest {
    run_slug:       run_slug.to_owned(),
    epoch:          cursor.map(|(epoch, _)| epoch),
    after_sequence: cursor.map(|(_, sequence)| sequence),
  }
}

#[tokio::test]
async fn run_responses_keep_marked_logs_and_tool_names_without_prompt_values() {
  for length in [511, 512, 513] {
    let harness = seeded_harness();
    let fixture = running_run_files(&harness);
    let step = "あ".repeat(length);
    fixture.append_records("run-1", &[json!({ "type": "step_complete", "step": step, "iteration": 1,
        "nested": [{ "systemPrompt": "秘密A", "content": "秘密B" }] })]);
    append_text(&phase1_dir(&fixture, "run-1").join("implement.jsonl"), &tool_use_line(&step));
    let mut poller = takt_dash_backend::poller::Poller::new(
      harness.config_path().to_path_buf(),
      harness.activity.clone(),
      harness.store.clone(),
      harness.changes.clone(),
      harness.registry.clone(),
    );
    poller.read_cycle_once().await.expect("本体ログを一覧の状態へ取り込みます");
    poller.shutdown().await;
    let server = start_server(&harness).await;
    let mut api = client(server.address).await;
    let update = updates(&mut api, "run-1", None).await;
    let detail = api
      .get_run_detail(GetRunDetailRequest { run_slug: "run-1".to_owned() })
      .await
      .expect("詳細を取得できます")
      .into_inner();
    let listed = api.get_overview(GetOverviewRequest {}).await.expect("一覧を取得できます").into_inner();
    server.stop().await;

    let expected = if length > 512 { format!("{}…", "あ".repeat(511)) } else { step };
    assert_eq!(listed.runs[0].steps.last().expect("一覧のステップがあります").step_name, expected);
    for bytes in [
      prost::Message::encode_to_vec(&listed),
      prost::Message::encode_to_vec(&detail),
      prost::Message::encode_to_vec(&update),
    ] {
      for secret in ["秘密A", "秘密B"] {
        assert!(!bytes.windows(secret.len()).any(|window| window == secret.as_bytes()));
      }
    }
    assert_eq!(update.activity.expect("活動があります").latest_tool.as_deref(), Some(expected.as_str()));
    assert!(update.lines.iter().any(|line| line.text == format!("step_complete {expected}")));
    assert!(detail.log_tail.contains(&format!("step_complete {expected}")));
    assert_eq!(detail.step_logs.last().expect("ステップがあります").step_name, expected);
    for line in update.lines {
      assert!(!line.text.contains("秘密A") && !line.text.contains("秘密B"));
    }
    assert!(!detail.log_tail.contains("秘密A") && !detail.log_tail.contains("秘密B"));
  }
}

async fn updates(
  client: &mut TaktDashServiceClient<Channel>,
  run_slug: &str,
  cursor: Option<(i64, i64)>,
) -> GetRunUpdatesResponse {
  client.get_run_updates(request(run_slug, cursor)).await.expect("run の追記分を取得できます").into_inner()
}

fn texts(response: &GetRunUpdatesResponse) -> Vec<String> {
  response.lines.iter().map(|line| line.text.clone()).collect()
}

/// 応答の最後の行の位置。行が無い応答では、送った位置をそのまま続きに使う。
fn next_cursor(response: &GetRunUpdatesResponse, previous: Option<(i64, i64)>) -> (i64, i64) {
  match response.lines.last() {
    | Some(line) => (response.epoch, line.sequence),
    | None => previous.expect("行の無い応答の続きには前回の位置を使います"),
  }
}

#[tokio::test]
async fn first_request_without_a_cursor_resets_with_the_same_lines_as_the_run_detail_log_tail() {
  // 続きの位置を送らない最初の要求は reset で、行は GetRunDetail の log_tail
  // と同じ範囲・同じ文字列です。
  let harness = seeded_harness();
  running_run_files(&harness);
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let detail = client
    .get_run_detail(GetRunDetailRequest { run_slug: "run-1".to_owned() })
    .await
    .expect("run 詳細を取得できます")
    .into_inner();
  let first = updates(&mut client, "run-1", None).await;
  server.stop().await;

  assert!(first.reset, "続きの位置が無い要求は全体を置き換える指示を返します");
  assert_eq!(first.run_slug, "run-1");
  assert!(!first.logs_removed);
  assert_eq!(
    texts(&first),
    detail.log_tail.lines().map(str::to_owned).collect::<Vec<_>>(),
    "reset の行は GetRunDetail の log_tail と同じ範囲です"
  );
  assert_eq!(texts(&first), vec![
    "workflow_start".to_owned(),
    "2026-11-13T09:01:00Z step_complete plan".to_owned(),
    "2026-11-13T09:05:00Z step_start implement".to_owned(),
  ]);
  let sequences: Vec<i64> = first.lines.iter().map(|line| line.sequence).collect();
  assert!(sequences.windows(2).all(|pair| pair[0] < pair[1]), "行の通し番号は増えていきます: {sequences:?}");
}

#[tokio::test]
async fn request_after_the_last_sequence_returns_only_the_appended_record() {
  // 追記した1件だけが返り、既に返した行を重複させない。毎回全件を返す実装は3行を返して落ちる。
  let harness = seeded_harness();
  let fixture = running_run_files(&harness);
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let first = updates(&mut client, "run-1", None).await;
  let cursor = next_cursor(&first, None);
  fixture.append_records("run-1", &[step_complete_record("implement", "coder", 1, "2026-11-13T09:20:00Z")]);
  let second = updates(&mut client, "run-1", Some(cursor)).await;
  server.stop().await;

  assert!(!second.reset, "続きを示せる要求は差分です");
  assert_eq!(second.epoch, cursor.0, "世代は変わりません");
  assert_eq!(texts(&second), vec!["2026-11-13T09:20:00Z step_complete implement".to_owned()]);
  assert!(second.lines[0].sequence > cursor.1, "追記した行の番号は前回の最後より大きい値です");
}

#[tokio::test]
async fn repeating_the_same_cursor_without_new_records_returns_no_lines() {
  // 追記が無ければ、同じ位置で何度要求しても行は返らない。
  let harness = seeded_harness();
  running_run_files(&harness);
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let first = updates(&mut client, "run-1", None).await;
  let cursor = next_cursor(&first, None);
  let second = updates(&mut client, "run-1", Some(cursor)).await;
  let third = updates(&mut client, "run-1", Some(cursor)).await;
  server.stop().await;

  assert!(!second.reset);
  assert!(second.lines.is_empty(), "追記が無ければ行を返しません: {:?}", texts(&second));
  assert!(!third.reset);
  assert!(third.lines.is_empty(), "同じ位置での2回目も行を返しません: {:?}", texts(&third));
}

#[tokio::test]
async fn record_read_first_by_get_run_detail_is_still_returned_to_the_next_updates_request() {
  // 取っ手はプロジェクトで1つです。追記の後に GetRunDetail が先に本体ログを読んでも、利用者の位置
  // より後の行として返す。共有の読み位置で新しく読んだ分だけを返す実装は0行を返して落ちる。
  let harness = seeded_harness();
  let fixture = running_run_files(&harness);
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let first = updates(&mut client, "run-1", None).await;
  let cursor = next_cursor(&first, None);
  fixture.append_records("run-1", &[step_complete_record("implement", "coder", 1, "2026-11-13T09:20:00Z")]);
  client
    .get_run_detail(GetRunDetailRequest { run_slug: "run-1".to_owned() })
    .await
    .expect("間に run 詳細を取得できます");
  let second = updates(&mut client, "run-1", Some(cursor)).await;
  server.stop().await;

  assert!(!second.reset);
  assert_eq!(texts(&second), vec!["2026-11-13T09:20:00Z step_complete implement".to_owned()]);
}

#[tokio::test]
async fn two_viewers_with_different_cursors_each_receive_the_lines_after_their_own_cursor() {
  // 2つのタブが別々の位置を持つ。片方が先に追記分を受け取っても、もう片方にも同じ行が返る。
  let harness = seeded_harness();
  let fixture = running_run_files(&harness);
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let tab_a = next_cursor(&updates(&mut client, "run-1", None).await, None);
  let tab_b = next_cursor(&updates(&mut client, "run-1", None).await, None);
  fixture.append_records("run-1", &[step_complete_record("implement", "coder", 1, "2026-11-13T09:20:00Z")]);
  let from_a = updates(&mut client, "run-1", Some(tab_a)).await;
  let from_b = updates(&mut client, "run-1", Some(tab_b)).await;
  server.stop().await;

  assert_eq!(texts(&from_a), vec!["2026-11-13T09:20:00Z step_complete implement".to_owned()]);
  assert_eq!(texts(&from_b), texts(&from_a), "先に受け取ったタブがあっても、別のタブにも同じ行が返ります");
}

#[tokio::test]
async fn a_different_epoch_resets_with_the_tail_lines() {
  // 世代が違う位置（backend の再起動や集計の作り直しの前の位置）は差分として扱わない。
  let harness = seeded_harness();
  running_run_files(&harness);
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let first = updates(&mut client, "run-1", None).await;
  let (epoch, sequence) = next_cursor(&first, None);
  let stale = updates(&mut client, "run-1", Some((epoch.wrapping_add(1), sequence))).await;
  server.stop().await;

  assert!(stale.reset, "世代が違う位置は全体を置き換える指示を返します");
  assert_eq!(stale.epoch, epoch, "応答の世代は現在の世代です");
  assert_eq!(texts(&stale), texts(&first), "reset では末尾の行を返します");
}

#[tokio::test]
async fn a_sequence_beyond_the_latest_line_resets() {
  // 最新より後の番号は、この世代の番号として成り立たないため reset にする。
  let harness = seeded_harness();
  running_run_files(&harness);
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let first = updates(&mut client, "run-1", None).await;
  let (epoch, sequence) = next_cursor(&first, None);
  let ahead = updates(&mut client, "run-1", Some((epoch, sequence + 100))).await;
  server.stop().await;

  assert!(ahead.reset, "最新より後の番号は全体を置き換える指示を返します");
  assert_eq!(texts(&ahead), texts(&first));
}

#[tokio::test]
async fn a_workflow_start_appended_after_the_cursor_resets_with_the_new_lines_only() {
  // workflow_start で行が初期化されたら、古い位置との比較で差分を返さず、新しい世代で置き換える。
  let harness = seeded_harness();
  let fixture = running_run_files(&harness);
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let first = updates(&mut client, "run-1", None).await;
  let cursor = next_cursor(&first, None);
  fixture.append_records("run-1", &[
    workflow_start_record(WORKFLOW),
    step_start_record("review", WORKFLOW, 1, "2026-11-13T10:00:00Z", &["review"]),
  ]);
  let restarted = updates(&mut client, "run-1", Some(cursor)).await;
  server.stop().await;

  assert!(restarted.reset, "workflow_start の後は全体を置き換える指示を返します");
  assert_ne!(restarted.epoch, cursor.0, "行を初期化したら世代を変えます");
  assert_eq!(texts(&restarted), vec!["workflow_start".to_owned(), "2026-11-13T10:00:00Z step_start review".to_owned()]);
}

#[tokio::test]
async fn reset_returns_at_most_the_run_detail_log_tail_count_of_lines() {
  // 行が多い run の reset は、GetRunDetail の log_tail と同じ末尾の20行です。
  let harness = seeded_harness();
  let fixture = running_run_files(&harness);
  let records: Vec<Value> = (0..30)
    .map(|index| step_complete_record(&format!("step-{index:02}"), "coder", 1, "2026-11-13T09:10:00Z"))
    .collect();
  fixture.append_records("run-1", &records);
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let detail = client
    .get_run_detail(GetRunDetailRequest { run_slug: "run-1".to_owned() })
    .await
    .expect("run 詳細を取得できます")
    .into_inner();
  let first = updates(&mut client, "run-1", None).await;
  server.stop().await;

  assert!(first.reset);
  assert_eq!(first.lines.len(), 20, "reset の行は20行です");
  assert_eq!(texts(&first), detail.log_tail.lines().map(str::to_owned).collect::<Vec<_>>());
  assert_eq!(texts(&first).last().map(String::as_str), Some("2026-11-13T09:10:00Z step_complete step-29"));
}

#[tokio::test]
async fn returns_the_session_log_activity_with_the_latest_tool_count_and_update_time() {
  // 活動の印は、更新時刻が最も新しいセッションログの最後の道具名、全ファイルの道具の呼び出しの
  // 件数、セッションログの更新時刻の最大値です。ファイル名の順で最後のファイルを採る実装は "Read"
  // を返して落ちる。
  let harness = seeded_harness();
  let fixture = running_run_files(&harness);
  let phase1 = phase1_dir(&fixture, "run-1");
  let older = phase1.join("z-plan.jsonl");
  let newer = phase1.join("a-implement.jsonl");
  append_text(&older, &format!("{}{}", tool_use_line("Read"), text_line("考えています")));
  append_text(&newer, &format!("{}{}{}", tool_use_line("Grep"), text_line("調べます"), tool_use_line("Bash")));
  set_modified(&older, time("2026-11-13T09:10:00Z"));
  set_modified(&newer, time("2026-11-13T09:12:00Z"));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let response = updates(&mut client, "run-1", None).await;
  server.stop().await;

  let activity = response.activity.expect("活動の印を返します");
  assert_eq!(activity.latest_tool.as_deref(), Some("Bash"), "更新時刻が最新のファイルの最後の道具です");
  assert_eq!(activity.tool_use_count, 3, "道具の呼び出しの件数で、文字の出力は数えません");
  assert_eq!(activity.updated_at, Some(proto_timestamp(time("2026-11-13T09:12:00Z"))));
}

#[tokio::test]
async fn session_activity_follows_tool_uses_appended_between_requests() {
  // 同じサーバーで、2回の要求の間にセッションログへ追記すると、件数と直近の道具名が追記に追従する。
  // 追記の前に数えた分を二重に数える実装は 3 を返して落ちる。
  let harness = seeded_harness();
  let fixture = running_run_files(&harness);
  let session = phase1_dir(&fixture, "run-1").join("implement-1.jsonl");
  append_text(&session, &tool_use_line("Read"));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let first = updates(&mut client, "run-1", None).await;
  let cursor = next_cursor(&first, None);
  append_text(&session, &tool_use_line("Edit"));
  let second = updates(&mut client, "run-1", Some(cursor)).await;
  server.stop().await;

  let before = first.activity.expect("1回目の活動の印を返します");
  assert_eq!(before.latest_tool.as_deref(), Some("Read"));
  assert_eq!(before.tool_use_count, 1);
  let after = second.activity.clone().expect("2回目の活動の印を返します");
  assert_eq!(after.latest_tool.as_deref(), Some("Edit"), "追記した道具が直近の道具になります");
  assert_eq!(after.tool_use_count, 2, "追記分だけを足し、既に数えた分を二重に数えません");
  assert!(second.lines.is_empty(), "セッションログの追記は本体ログの行にしません: {:?}", texts(&second));
}

#[tokio::test]
async fn run_without_session_logs_returns_an_empty_activity() {
  let harness = seeded_harness();
  running_run_files(&harness);
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let response = updates(&mut client, "run-1", None).await;
  server.stop().await;

  let activity = response.activity.expect("セッションログが無くても活動の印の欄を返します");
  assert_eq!(activity.latest_tool, None);
  assert_eq!(activity.tool_use_count, 0);
  assert_eq!(activity.updated_at, None);
}

#[tokio::test]
async fn get_run_updates_neither_starts_nor_calls_takt_mcp() {
  let harness = seeded_harness();
  running_run_files(&harness);
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let first = updates(&mut client, "run-1", None).await;
  updates(&mut client, "run-1", Some(next_cursor(&first, None))).await;
  server.stop().await;

  assert!(harness.recorded_processes().is_empty(), "GetRunUpdates は takt-mcp を起動しません");
  assert!(harness.recorded_calls().is_empty(), "GetRunUpdates は takt-mcp の道具を呼びません");
}

#[tokio::test]
async fn get_run_updates_reports_logs_removed_for_a_cleaned_worktree() {
  // 保存場所が残っていない run はエラーにせず、logs_removed を立てた空の応答を返す。
  let harness = seeded_harness();
  let worktree = harness
    .project_dir()
    .parent()
    .expect("プロジェクトの cwd には親があります")
    .join("takt-worktrees")
    .join("wt-cleaned");
  RunFixture::new(harness.project_dir()).write_worktree_tasks(&[WorktreeTaskLine {
    run_slug:        Some("run-1"),
    source_run_slug: None,
    worktree_path:   &worktree,
  }]);
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let response = updates(&mut client, "run-1", None).await;
  server.stop().await;

  assert!(response.logs_removed, "ログが残っていないことを logs_removed で返します");
  assert!(response.lines.is_empty());
}

#[tokio::test]
async fn get_run_updates_returns_not_found_for_an_unknown_run_slug() {
  let harness = seeded_harness();
  running_run_files(&harness);
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let status =
    client.get_run_updates(request("does-not-exist", None)).await.expect_err("状態ストアにない runSlug はエラーです");
  server.stop().await;

  assert_eq!(status.code(), tonic::Code::NotFound, "見つからない runSlug は Not Found です: {status:?}");
}

#[tokio::test]
async fn get_run_updates_rejects_an_invalid_run_slug_and_an_invalid_cursor() {
  // 続きの位置は世代と番号の両方を送るか、両方を省く。負の値と、片方だけの位置は入力の誤りです。
  let harness = seeded_harness();
  running_run_files(&harness);
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;
  let epoch_only =
    GetRunUpdatesRequest { run_slug: "run-1".to_owned(), epoch: Some(1), after_sequence: None };
  let sequence_only =
    GetRunUpdatesRequest { run_slug: "run-1".to_owned(), epoch: None, after_sequence: Some(0) };

  let mut statuses = Vec::new();
  for invalid in [
    request("", None),
    request("runs/1", None),
    request("run-1", Some((-1, 0))),
    request("run-1", Some((1, -1))),
    epoch_only,
    sequence_only,
  ] {
    let label = format!("{invalid:?}");
    let status = client.get_run_updates(invalid).await.expect_err("不正な要求は拒否します");
    statuses.push((label, status.code()));
  }
  server.stop().await;

  for (label, code) in statuses {
    assert_eq!(code, tonic::Code::InvalidArgument, "{label} は入力の誤りとして拒否します");
  }
}

/// takt_get_run が run-1 をリモートのサーバーの値で返す mock の状態。stepLogs は2件で、
/// ローカルの保存場所の値と区別できる固有のステップ名にする。
fn remote_run_state() -> Value {
  let mut state = mcp_state();
  state["runs"] = json!({
    "run-1": {
      "runSlug": "run-1",
      "status": "running",
      "workflow": "remote-workflow",
      "currentStep": "remote-implement",
      "currentIteration": 4,
      "phase": 2,
      "startedAt": "2026-11-13T09:00:00Z",
      "currentStepStartedAt": "2026-11-13T09:20:00Z",
      "currentStepExecutionCount": 2,
      "stepLogs": [
        { "step": "remote-plan", "persona": "architect", "status": "done", "timestamp": "2026-11-13T09:10:00Z", "iteration": 1 },
        { "step": "remote-implement", "persona": "coder", "status": "done", "timestamp": "2026-11-13T09:30:00Z", "iteration": 1 }
      ],
      "reports": [ { "filename": "remote-plan.md", "sizeBytes": 12 } ]
    }
  });
  state
}

/// 状態ストアに run-1 を持ち、ローカルに run の保存場所が無い 1 プロジェクト分の土台を作る。
fn seeded_remote_harness() -> GrpcHarness {
  let harness = GrpcHarness::builder()
    .single_project(project("takt-dash", project_dir()), server_meta(), remote_run_state())
    .build();
  let mut stored_run = run("run-1", "implement", time("2026-11-13T09:06:00Z"));
  stored_run.goal_id = Some(GoalId::try_from("g1").expect("テストのゴール ID は有効です"));
  let snapshot = ProjectSnapshot {
    goals: vec![goal("g1", GoalStatus::Created, ExecutionStatus::Active)],
    tasks: vec![],
    inquiries: vec![],
    runs: vec![stored_run],
    notifications: vec![],
    proposals: vec![],
    ..ProjectSnapshot::empty()
  };
  harness.store.record_success("takt-dash", snapshot, time("2026-11-13T12:00:00Z"));
  harness
}

#[tokio::test]
async fn remote_updates_do_not_read_task_text_for_a_missing_summary() {
  for truncated in [false, true] {
    let harness = seeded_remote_harness();
    let mut state = remote_run_state();
    state["runs"]["run-1"]["taskOmission"] = json!({ "truncated": truncated, "totalBytes": 70000 });
    harness.rewrite_state(state);
    let server = start_server(&harness).await;
    let mut api = client(server.address).await;
    let first = updates(&mut api, "run-1", None).await;
    let cursor = next_cursor(&first, None);
    let second = updates(&mut api, "run-1", Some(cursor)).await;
    server.stop().await;

    assert!(first.reset);
    assert_eq!(texts(&first), vec![
      "2026-11-13T09:10:00Z step_complete remote-plan",
      "2026-11-13T09:30:00Z step_complete remote-implement",
    ]);
    assert_eq!(cursor, (first.epoch, 2));
    assert_eq!(second.epoch, first.epoch);
    assert!(!second.reset);
    assert!(second.lines.is_empty());
    let calls = harness.recorded_calls();
    assert_eq!(calls.iter().filter(|call| call.name == "takt_get_run").count(), 2);
    assert!(calls.iter().all(|call| call.arguments["view"] != "task"), "{calls:?}");
  }
}

#[tokio::test]
async fn get_run_updates_reads_a_run_without_a_local_task_table_from_takt_get_run() {
  // リモートのサーバーのプロジェクトは、ローカルに保存場所が無く本体ログの続きを読めない。
  // その run の追記分は takt_get_run の stepLogs から組み立てて返し、エラーにしない。
  // ローカルの読み取りだけに頼る実装は Unavailable で落ちて、画面に取得の警告が残り続ける。
  let harness = seeded_remote_harness();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let detail = client
    .get_run_detail(GetRunDetailRequest { run_slug: "run-1".to_owned() })
    .await
    .expect("run 詳細を取得できます")
    .into_inner();
  let first = updates(&mut client, "run-1", None).await;
  let cursor = next_cursor(&first, None);
  let second = updates(&mut client, "run-1", Some(cursor)).await;
  server.stop().await;

  assert!(first.reset, "リモートの run の最初の追記分は全体を置き換える指示です");
  assert!(!first.logs_removed);
  assert!(
    (0..=9_007_199_254_740_991).contains(&first.epoch),
    "世代は JavaScript の安全な整数（Number.MAX_SAFE_INTEGER）の範囲です: {}",
    first.epoch
  );
  assert_eq!(texts(&first), vec![
    "2026-11-13T09:10:00Z step_complete remote-plan".to_owned(),
    "2026-11-13T09:30:00Z step_complete remote-implement".to_owned(),
  ]);
  assert_eq!(
    texts(&first),
    detail.log_tail.lines().map(str::to_owned).collect::<Vec<_>>(),
    "reset の行は GetRunDetail の log_tail と同じ範囲です"
  );
  assert!(!second.reset, "同じ世代で末尾まで受け取り済みなら追記分は差分です");
  assert_eq!(second.epoch, first.epoch, "行が変わらなければ世代は変わりません");
  assert!(second.lines.is_empty(), "追記が無ければ行を返しません: {:?}", texts(&second));
  let calls = harness.recorded_calls();
  assert!(
    calls.iter().any(|call| call.name == "takt_get_run" && call.arguments["runSlug"] == "run-1"),
    "ローカルで読めない run の追記分は takt_get_run で読みます: {calls:?}"
  );
}

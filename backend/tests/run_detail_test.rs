//! gRPC の GetRunDetail が、run の保存場所（`meta.json` と本体ログ）からステップの記録と状態を、
//! takt_get_run の summary からレポートと指示書を読んで GetRunDetailResponse を組み立てることを
//! 検証する。同じサーバーへの2回目の要求は本体ログの続きだけを読み、stepLogs のページ追従を
//! しないことも観測する。状態ストアにない runSlug は Not Found 相当のステータスで返す。

mod common;
mod grpc_harness;

use std::path::Path;

use chrono::{DateTime, Duration, Utc};
use common::*;
use grpc_harness::{client, start_server, GrpcHarness};
use serde_json::{json, Value};
use takt_dash_backend::{
  domain::{GoalId, RunSlug, TaskName},
  model::{ExecutionStatus, GoalStatus, ProjectSnapshot, TaskEntry, TaskStatus},
  poller::Poller,
  takt_dash::v1::{GetRunDetailRequest, GetRunDetailResponse, RunStatus},
};

const WORKFLOW: &str = "development-core";

fn project_dir() -> &'static Path {
  Path::new("/abs/takt-dash")
}

/// takt-mcp が返す状態。run-1 の summary は、保存場所と食い違う status・currentStep・stepLogs と、
/// 保存場所に無い sessionId・summary・taskText・logTail・レポート・指示書を持つ。
fn mcp_state() -> Value {
  json!({
    "tasks": [
      { "name": "implement", "goalId": "g1", "status": "running", "startedAt": "2026-11-13T08:30:00Z", "runSlug": "run-1", "currentStep": "implement" }
    ],
    "runs": {
      "run-1": {
        "runSlug": "run-1",
        "sessionId": "sess-1",
        "summary": "実装の run",
        "workflow": WORKFLOW,
        "status": "completed",
        "startedAt": "2026-11-13T07:00:00Z",
        "currentStep": "decoy",
        "currentIteration": 99,
        "lastLogAt": "2026-11-13T07:30:00Z",
        "logTail": "最後のログの行です。",
        "taskText": "タスクの本文です。",
        "stepLogs": [
          { "step": "decoy", "persona": "coder", "status": "done", "content": "応答の本文", "timestamp": "2026-11-13T07:30:00Z", "iteration": 1 }
        ],
        "reports": [ { "filename": "plan.md", "content": "計画のレポート", "sizeBytes": 120 } ],
        "instructions": [
          { "instructionId": 7, "issuedAt": "2026-11-13T08:50:00Z", "state": "applied", "content": "指示の本文" }
        ]
      }
    }
  })
}

/// 状態ストアに run-1 を持つ 1 プロジェクト分の土台を作る。
fn seeded_harness(state: Value) -> GrpcHarness {
  let harness =
    GrpcHarness::builder().single_project(project("takt-dash", project_dir()), server_meta(), state).build();
  let snapshot = ProjectSnapshot {
    goals:         vec![goal("g1", GoalStatus::Created, ExecutionStatus::Active)],
    tasks:         vec![],
    inquiries:     vec![],
    runs:          vec![run("run-1", "implement", time("2026-11-13T09:06:00Z"))],
    notifications: vec![],
    proposals:     vec![],
  };
  harness.store.record_success("takt-dash", snapshot, time("2026-11-13T12:00:00Z"));
  harness
}

/// 土台のプロジェクトの cwd に、対応表に run-1 だけを載せた run の保存場所を作る。
fn run_files(harness: &GrpcHarness) -> RunFixture {
  let fixture = RunFixture::new(harness.project_dir());
  fixture.write_tasks(&[RunTaskLine { run_slug: Some("run-1"), source_run_slug: None }]);
  fixture
}

/// 実行中の run-1 を、plan と implement の step_complete、現在ステップ implement の step_start
/// で作る。本体ログの更新時刻は step より古い 09:00 に固定する。
fn running_run_files(harness: &GrpcHarness) -> RunFixture {
  let fixture = run_files(harness);
  fixture.write_meta("run-1", &running_meta("implement", 3, WORKFLOW, "2026-11-13T09:00:00Z"));
  fixture.append_records("run-1", &[
    workflow_start_record(WORKFLOW),
    step_complete_record("plan", "architect", 1, "2026-11-13T09:01:00Z"),
    step_start_record("implement", WORKFLOW, 3, "2026-11-13T09:05:00Z", &["implement"]),
  ]);
  fixture.set_body_log_modified("run-1", time("2026-11-13T09:00:00Z"));
  fixture
}

async fn request_detail(harness: &GrpcHarness, run_slug: &str) -> Result<GetRunDetailResponse, tonic::Status> {
  let server = start_server(harness).await;
  let mut client = client(server.address).await;
  let result = client.get_run_detail(GetRunDetailRequest { run_slug: run_slug.to_owned() }).await;
  server.stop().await;
  result.map(tonic::Response::into_inner)
}

fn timestamp(text: &str) -> prost_types::Timestamp {
  proto_timestamp(time(text))
}

fn proto_timestamp(time: DateTime<Utc>) -> prost_types::Timestamp {
  prost_types::Timestamp { seconds: time.timestamp(), nanos: time.timestamp_subsec_nanos() as i32 }
}

fn step_log_names(detail: &GetRunDetailResponse) -> Vec<String> {
  detail.step_logs.iter().map(|log| log.step_name.clone()).collect()
}

#[tokio::test]
async fn get_run_detail_assembles_the_response_from_the_run_files_and_the_run_summary() {
  let harness = seeded_harness(mcp_state());
  running_run_files(&harness);

  let detail = request_detail(&harness, "run-1").await.expect("run 詳細を取得できます");

  // 保存場所から読む値。takt_get_run の summary の status・currentStep・stepLogs は使わない。
  assert_eq!(detail.status, RunStatus::Running as i32, "status は meta.json の値です");
  assert_eq!(detail.current_step.as_deref(), Some("implement"));
  assert_eq!(detail.current_iteration, Some(3));
  assert_eq!(detail.phase, Some(1));
  assert_eq!(detail.started_at, Some(timestamp("2026-11-13T09:00:00Z")), "開始時刻は meta.json の startTime です");
  assert_eq!(detail.current_step_started_at, Some(timestamp("2026-11-13T09:05:00Z")));
  assert_eq!(detail.last_log_at, Some(timestamp("2026-11-13T09:05:00Z")));
  assert_eq!(step_log_names(&detail), vec!["plan".to_owned()], "step_logs は本体ログの step_complete です");
  assert_eq!(detail.step_logs[0].persona, "architect");
  assert_eq!(detail.step_logs[0].status, "done");
  assert_eq!(detail.step_logs[0].iteration, 1);
  assert_eq!(detail.step_logs[0].timestamp, Some(timestamp("2026-11-13T09:01:00Z")));
  assert_eq!(detail.step_logs[0].content, "", "ステップの本文は持ちません");

  // takt_get_run の summary と本文の取得から読む値。
  assert_eq!(detail.session_id, "sess-1");
  assert_eq!(detail.summary, "実装の run");
  assert_eq!(detail.task_text, "タスクの本文です。");
  assert_eq!(detail.log_tail, "最後のログの行です。");
  assert_eq!(detail.workflow, WORKFLOW);
  assert_eq!(detail.reports.len(), 1);
  assert_eq!(detail.reports[0].filename, "plan.md");
  assert_eq!(detail.reports[0].content, "計画のレポート");
  assert_eq!(detail.reports[0].size_bytes, 120);
  assert_eq!(detail.instructions.len(), 1);
  assert_eq!(detail.instructions[0].instruction_id, 7);
  assert_eq!(detail.instructions[0].state, "applied");
  assert_eq!(detail.instructions[0].content, "指示の本文");

  // 状態ストアと takt_list_tasks から補う値。
  assert_eq!(detail.run_slug, "run-1");
  assert_eq!(detail.server_id, "mac-mini");
  assert_eq!(detail.project, "takt-dash");
  assert_eq!(detail.goal_id, "g1");
  assert_eq!(detail.task_name, "implement");
  assert!(detail.server_connected, "読めていたプロジェクトは接続ありです");
  assert_eq!(detail.server_last_read_at, Some(timestamp("2026-11-13T12:00:00Z")));
}

#[tokio::test]
async fn get_run_detail_does_not_follow_step_log_pages_of_takt_get_run() {
  // takt_get_run の stepLogs が先頭ページに収まらない run でも、offset 付きの takt_get_run を
  // 呼ばない。ステップの記録は本体ログから読む。
  let mut state = mcp_state();
  let step_logs: Vec<Value> = (0..120)
    .map(|index| json!({ "step": format!("decoy{index}"), "persona": "coder", "status": "done", "timestamp": "2026-11-13T07:30:00Z", "iteration": 1 }))
    .collect();
  state["runs"]["run-1"]["stepLogs"] = Value::Array(step_logs);
  let harness = seeded_harness(state);
  running_run_files(&harness);

  let detail = request_detail(&harness, "run-1").await.expect("run 詳細を取得できます");

  assert_eq!(step_log_names(&detail), vec!["plan".to_owned()]);
  let paged: Vec<Value> = harness
    .recorded_calls()
    .into_iter()
    .filter(|call| call.name == "takt_get_run" && call.arguments.get("offset").is_some())
    .map(|call| call.arguments)
    .collect();
  assert!(paged.is_empty(), "stepLogs のページ追従をしません: {paged:?}");
}

#[tokio::test]
async fn second_run_detail_on_the_same_server_adds_only_the_appended_step_complete() {
  // 同じサーバーへの1回目の要求の後に step_complete を追記し、2回目の要求で追記分が加わる。
  let harness = seeded_harness(mcp_state());
  let fixture = running_run_files(&harness);
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let first = client
    .get_run_detail(GetRunDetailRequest { run_slug: "run-1".to_owned() })
    .await
    .expect("1回目の run 詳細を取得できます")
    .into_inner();
  assert_eq!(step_log_names(&first), vec!["plan".to_owned()]);

  fixture.append_records("run-1", &[step_complete_record("implement", "coder", 3, "2026-11-13T09:20:00Z")]);
  let second = client
    .get_run_detail(GetRunDetailRequest { run_slug: "run-1".to_owned() })
    .await
    .expect("2回目の run 詳細を取得できます")
    .into_inner();
  server.stop().await;

  assert_eq!(
    step_log_names(&second),
    vec!["plan".to_owned(), "implement".to_owned()],
    "追記した step_complete だけが加わり、既読の step を重複させません"
  );
  let paged = harness
    .recorded_calls()
    .into_iter()
    .filter(|call| call.name == "takt_get_run" && call.arguments.get("offset").is_some())
    .count();
  assert_eq!(paged, 0, "どの要求も stepLogs のページ追従をしません");
}

#[tokio::test]
async fn second_run_detail_on_the_same_server_does_not_read_the_already_read_part_again() {
  // 1回目の要求で読んだ範囲を、inode と大きさを変えずに同じバイト長の別の文字列へ書き換える。
  // 要求のたびに先頭から読み直す実装は2回目に "PLAN" を返して落ちる。
  let harness = seeded_harness(mcp_state());
  let fixture = running_run_files(&harness);
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  client
    .get_run_detail(GetRunDetailRequest { run_slug: "run-1".to_owned() })
    .await
    .expect("1回目の run 詳細を取得できます");
  fixture.overwrite_body_log_in_place("run-1", "\"plan\"", "\"PLAN\"");
  let second = client
    .get_run_detail(GetRunDetailRequest { run_slug: "run-1".to_owned() })
    .await
    .expect("2回目の run 詳細を取得できます")
    .into_inner();
  server.stop().await;

  assert_eq!(step_log_names(&second), vec!["plan".to_owned()], "既読範囲は読み直しません");
}

#[tokio::test]
async fn run_detail_on_the_same_server_reads_the_new_cwd_after_the_project_cwd_changes() {
  // 同じサーバーで、旧 cwd の run を読んだ後に設定のプロジェクトの cwd を書き換える。2回目の要求は
  // 新しい cwd の保存場所から読む。旧 cwd の集計を使い続ける実装は2回目も "plan" を返して落ちる。
  let harness = seeded_harness(mcp_state());
  running_run_files(&harness);
  let new_root = tempfile::tempdir().expect("新しい cwd の一時ディレクトリを作れます");
  let new_cwd = std::fs::canonicalize(new_root.path()).expect("実パスを解決できます").join("project");
  std::fs::create_dir_all(&new_cwd).expect("新しい cwd を作れます");
  let new_fixture = RunFixture::new(&new_cwd);
  new_fixture.write_tasks(&[RunTaskLine { run_slug: Some("run-1"), source_run_slug: None }]);
  new_fixture.write_meta("run-1", &running_meta("review", 1, WORKFLOW, "2026-11-13T10:00:00Z"));
  new_fixture.append_records("run-1", &[
    workflow_start_record(WORKFLOW),
    step_complete_record("review", "reviewer", 1, "2026-11-13T10:05:00Z"),
  ]);
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let first = client
    .get_run_detail(GetRunDetailRequest { run_slug: "run-1".to_owned() })
    .await
    .expect("旧 cwd の run 詳細を取得できます")
    .into_inner();
  assert_eq!(step_log_names(&first), vec!["plan".to_owned()]);

  let new_cwd_text = new_cwd.display().to_string();
  harness.rewrite_projects(|projects, _| {
    projects[0]["cwd"] = json!(new_cwd_text);
  });
  let second = client
    .get_run_detail(GetRunDetailRequest { run_slug: "run-1".to_owned() })
    .await
    .expect("新しい cwd の run 詳細を取得できます")
    .into_inner();
  server.stop().await;

  assert_eq!(step_log_names(&second), vec!["review".to_owned()], "書き換え後の cwd の本体ログを読みます");
  assert_eq!(second.current_step.as_deref(), Some("review"), "状態も新しい cwd の meta.json から読みます");
}

#[tokio::test]
async fn get_run_detail_keeps_server_connected_while_the_project_is_delayed() {
  let harness = seeded_harness(mcp_state());
  running_run_files(&harness);
  harness.store.record_failure("takt-dash", time("2026-11-13T12:00:30Z"));

  let detail = request_detail(&harness, "run-1").await.expect("遅延中も run 詳細を組み立てられます");

  assert!(detail.server_connected, "遅延中は接続中として扱います");
  assert_eq!(detail.server_last_read_at, Some(timestamp("2026-11-13T12:00:00Z")), "最後に読めた時刻を返します");
}

#[tokio::test]
async fn get_run_detail_reports_server_disconnected_while_keeping_last_read_at() {
  let harness = seeded_harness(mcp_state());
  running_run_files(&harness);
  let failed_at = time("2026-11-13T12:02:00Z");
  for _ in 0..3 {
    harness.store.record_failure("takt-dash", failed_at);
  }

  let detail = request_detail(&harness, "run-1").await.expect("応答がない間も run 詳細を組み立てられます");

  assert!(!detail.server_connected, "応答がなければ server_connected は偽です");
  assert_eq!(detail.server_last_read_at, Some(timestamp("2026-11-13T12:00:00Z")), "最後に読めた時刻を返します");
}

#[tokio::test]
async fn get_run_detail_returns_not_found_for_an_unknown_run_slug() {
  let harness = seeded_harness(mcp_state());
  running_run_files(&harness);

  let status = request_detail(&harness, "does-not-exist").await.expect_err("状態ストアにない runSlug はエラーです");

  assert_eq!(status.code(), tonic::Code::NotFound, "見つからない runSlug は Not Found です: {status:?}");
}

#[tokio::test]
async fn get_run_detail_returns_not_found_for_a_run_missing_from_the_task_table() {
  // 状態ストアにはあるが、tasks.yaml の対応表に載っていない run は Not Found です。
  let harness = seeded_harness(mcp_state());
  let fixture = RunFixture::new(harness.project_dir());
  fixture.write_tasks(&[RunTaskLine { run_slug: Some("other-run"), source_run_slug: None }]);

  let status = request_detail(&harness, "run-1").await.expect_err("対応表に無い run は詳細を返しません");

  assert_eq!(status.code(), tonic::Code::NotFound, "対応表に無い run は Not Found です: {status:?}");
}

#[tokio::test]
async fn get_run_detail_rejects_an_empty_run_slug_as_invalid_argument() {
  let harness = seeded_harness(mcp_state());

  let status = request_detail(&harness, "").await.expect_err("空の runSlug は拒否します");

  assert_eq!(status.code(), tonic::Code::InvalidArgument, "空の runSlug は Invalid Argument です: {status:?}");
}

#[tokio::test]
async fn get_run_detail_rejects_a_run_slug_that_is_not_a_single_directory_name() {
  let harness = seeded_harness(mcp_state());
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  for run_slug in ["runs/1", "runs\\1", "..", "."] {
    let status = client
      .get_run_detail(GetRunDetailRequest { run_slug: run_slug.to_owned() })
      .await
      .expect_err("run ディレクトリ名にならない runSlug は拒否します");

    assert_eq!(
      status.code(),
      tonic::Code::InvalidArgument,
      "runSlug={run_slug:?} は入力の誤りとして拒否します: {status:?}"
    );
  }

  server.stop().await;
}

#[tokio::test]
async fn get_run_detail_rejects_a_run_meta_without_a_status() {
  let harness = seeded_harness(mcp_state());
  let fixture = run_files(&harness);
  let mut meta = running_meta("implement", 3, WORKFLOW, "2026-11-13T09:00:00Z");
  meta.as_object_mut().expect("meta はオブジェクトです").remove("status");
  fixture.write_meta("run-1", &meta);
  fixture.append_records("run-1", &[workflow_start_record(WORKFLOW)]);

  let status = request_detail(&harness, "run-1").await.expect_err("run の状態が欠落した meta.json は失敗します");

  assert_eq!(
    status.code(),
    tonic::Code::Internal,
    "欠落した run の状態は Running に丸めず、内部エラーにします: {status:?}"
  );
}

#[tokio::test]
async fn get_run_detail_rejects_a_malformed_start_time() {
  // 欠損の許容は、不正な値の許容に読み替えない。RFC3339 でない startTime は内部エラーです。
  let harness = seeded_harness(mcp_state());
  let fixture = run_files(&harness);
  fixture.write_meta("run-1", &running_meta("implement", 3, WORKFLOW, "not-a-time"));
  fixture.append_records("run-1", &[workflow_start_record(WORKFLOW)]);

  let status = request_detail(&harness, "run-1").await.expect_err("不正な開始時刻は失敗します");

  assert_eq!(status.code(), tonic::Code::Internal, "不正な開始時刻は内部エラーです: {status:?}");
}

#[tokio::test]
async fn a_run_without_any_start_time_is_listed_and_reaches_run_detail() {
  // 開始時刻（meta の startTime とタスクの startedAt）も本体ログも無い実行中の run でも、一覧の
  // 読み取りが失敗せず run が状態ストアに入り、詳細取得に到達する。最後のログが分からない run を
  // 無音と判定しない。
  let mut state = mcp_state();
  state["tasks"][0].as_object_mut().expect("task はオブジェクトです").remove("startedAt");
  let harness =
    GrpcHarness::builder().single_project(project("takt-dash", project_dir()), server_meta(), state).build();
  let fixture = run_files(&harness);
  let mut meta = running_meta("implement", 1, WORKFLOW, "2026-11-13T09:00:00Z");
  meta.as_object_mut().expect("meta はオブジェクトです").remove("startTime");
  fixture.write_meta("run-1", &meta);
  let mut poller = Poller::new(
    harness.config_path().to_path_buf(),
    harness.activity.clone(),
    harness.store.clone(),
    harness.changes.clone(),
    harness.registry.clone(),
  );
  poller.read_cycle_once().await.expect("1サイクルを実行できます");
  poller.shutdown().await;
  let listed = harness
    .store
    .slot("takt-dash")
    .and_then(|slot| slot.snapshot)
    .and_then(|snapshot| snapshot.runs.into_iter().find(|run| run.run_slug == "run-1"))
    .expect("開始時刻が無くても run は一覧に含まれます");
  assert_eq!(listed.started_at, None, "開始時刻は未設定として保持します");

  let detail = request_detail(&harness, "run-1").await.expect("一覧に載った run は詳細取得に到達できます");

  assert_eq!(detail.started_at, None, "詳細の開始時刻も未設定で返ります");
  assert_eq!(detail.last_log_at, None, "最後のログの時刻も未設定です");
  assert!(!detail.log_silent, "最後のログの時刻が分からない run を無音と判定しません");
  assert_eq!(detail.task_name, "implement", "開始時刻が無くても他の項目は返ります");
}

#[tokio::test]
async fn get_run_detail_marks_log_silent_false_when_the_last_log_is_within_the_threshold() {
  // 最後のログが 1 分前（既定 10 分の閾値の内側）なら log_silent
  // は偽。下の外側ケースと対で観測する。
  let last_log_at = Utc::now() - Duration::minutes(1);
  let harness = seeded_harness(mcp_state());
  let fixture = run_files(&harness);
  fixture.write_meta("run-1", &running_meta("implement", 1, WORKFLOW, "2026-11-13T09:00:00Z"));
  fixture.append_records("run-1", &[
    workflow_start_record(WORKFLOW),
    step_complete_record("plan", "architect", 1, &last_log_at.to_rfc3339()),
  ]);
  fixture.set_body_log_modified("run-1", last_log_at);

  let detail = request_detail(&harness, "run-1").await.expect("run 詳細を取得できます");

  assert!(!detail.log_silent, "閾値内側の最後のログは無音ではありません");
}

#[tokio::test]
async fn get_run_detail_marks_log_silent_true_when_the_last_log_exceeds_the_threshold() {
  // 最後のログ（step と本体ログの更新時刻）が 30 分前なら log_silent は真。
  let last_log_at = Utc::now() - Duration::minutes(30);
  let harness = seeded_harness(mcp_state());
  let fixture = run_files(&harness);
  fixture.write_meta("run-1", &running_meta("implement", 1, WORKFLOW, "2026-11-13T09:00:00Z"));
  fixture.append_records("run-1", &[
    workflow_start_record(WORKFLOW),
    step_complete_record("plan", "architect", 1, &last_log_at.to_rfc3339()),
  ]);
  fixture.set_body_log_modified("run-1", last_log_at);

  let detail = request_detail(&harness, "run-1").await.expect("run 詳細を取得できます");

  assert!(detail.log_silent, "閾値外側の最後のログは無音です");
}

#[tokio::test]
async fn get_run_detail_last_log_at_follows_a_log_file_update_newer_than_any_step() {
  // 本体ログの更新時刻が step_complete と現在ステップの開始より新しければ、その時刻を採る。
  let harness = seeded_harness(mcp_state());
  let fixture = running_run_files(&harness);
  fixture.set_body_log_modified("run-1", time("2026-11-13T09:30:00Z"));

  let detail = request_detail(&harness, "run-1").await.expect("run 詳細を取得できます");

  assert_eq!(detail.last_log_at, Some(timestamp("2026-11-13T09:30:00Z")));
}

#[tokio::test]
async fn get_run_detail_last_log_at_ignores_a_step_complete_without_timestamp() {
  // timestamp を持たない step_complete
  // は最新時刻の候補にしない。欠落時刻（UNIX_EPOCH）を採る実装や、
  // 末尾の step をそのまま最新とみなす実装は落ちる。
  let harness = seeded_harness(mcp_state());
  let fixture = run_files(&harness);
  fixture.write_meta("run-1", &running_meta("implement", 1, WORKFLOW, "2026-11-13T09:00:00Z"));
  let mut without_timestamp = step_complete_record("implement", "coder", 1, "2026-11-13T09:00:00Z");
  without_timestamp.as_object_mut().expect("record はオブジェクトです").remove("timestamp");
  fixture.append_records("run-1", &[
    workflow_start_record(WORKFLOW),
    step_complete_record("plan", "architect", 1, "2026-11-13T09:40:00Z"),
    without_timestamp,
  ]);
  fixture.set_body_log_modified("run-1", time("2026-11-13T09:10:00Z"));

  let detail = request_detail(&harness, "run-1").await.expect("run 詳細を取得できます");

  assert_eq!(
    detail.last_log_at,
    Some(timestamp("2026-11-13T09:40:00Z")),
    "timestamp の無い step_complete は最新時刻の候補にしません"
  );
  assert_eq!(step_log_names(&detail), vec!["plan".to_owned(), "implement".to_owned()]);
}

#[tokio::test]
async fn run_list_and_run_detail_resolve_the_same_last_log_at() {
  // 一覧（poller）と詳細（GetRunDetail）が同じ run の最終ログ時刻を同じ規則で決め、無音判定が
  // 食い違わない。
  let harness =
    GrpcHarness::builder().single_project(project("takt-dash", project_dir()), server_meta(), mcp_state()).build();
  let fixture = running_run_files(&harness);
  fixture.set_body_log_modified("run-1", time("2026-11-13T09:03:00Z"));
  let mut poller = Poller::new(
    harness.config_path().to_path_buf(),
    harness.activity.clone(),
    harness.store.clone(),
    harness.changes.clone(),
    harness.registry.clone(),
  );
  poller.read_cycle_once().await.expect("1サイクルを実行できます");
  poller.shutdown().await;
  let listed = harness
    .store
    .slot("takt-dash")
    .and_then(|slot| slot.snapshot)
    .and_then(|snapshot| snapshot.runs.into_iter().find(|run| run.run_slug == "run-1"))
    .expect("running の run が一覧にあります");

  let detail = request_detail(&harness, "run-1").await.expect("run 詳細を取得できます");

  assert_eq!(listed.last_log_at, Some(time("2026-11-13T09:05:00Z")), "一覧は現在ステップの開始を採ります");
  assert_eq!(detail.last_log_at, listed.last_log_at.map(proto_timestamp), "詳細も一覧と同じ時刻です");
}

#[tokio::test]
async fn 完了した実行の詳細は実行中一覧になくても取得できる() {
  // 状態ストアの runs が空でも、完了タスクの runSlug からプロジェクトを解決して詳細を返します。
  // takt_list_tasks にもタスクが無いため、状態ストアの完了タスクからゴールとタスク名を補います。
  let harness = GrpcHarness::builder()
    .single_project(
      project("takt-dash", project_dir()),
      server_meta(),
      json!({
        "runs": { "run-1": { "runSlug": "run-1", "sessionId": "sess-1", "summary": "完了した run", "status": "completed", "stepLogs": [] } }
      }),
    )
    .build();
  let fixture = run_files(&harness);
  let mut meta = running_meta("review", 2, WORKFLOW, "2026-11-13T09:00:00Z");
  meta["status"] = json!("completed");
  fixture.write_meta("run-1", &meta);
  fixture.append_records("run-1", &[workflow_start_record(WORKFLOW)]);
  let snapshot = ProjectSnapshot {
    goals:         vec![],
    tasks:         vec![TaskEntry {
      name:            TaskName::try_from("implement").expect("テストのタスク名は有効です"),
      goal_id:         Some(GoalId::try_from("g1").expect("テストのゴール ID は有効です")),
      status:          TaskStatus::Completed,
      started_at:      Some(time("2026-11-13T09:00:00Z")),
      run_slug:        Some(RunSlug::try_from("run-1").expect("テストの run slug は有効です")),
      source_run_slug: None,
      current_step:    Some("review".to_owned()),
      merge_task:      false,
    }],
    inquiries:     vec![],
    runs:          vec![],
    notifications: vec![],
    proposals:     vec![],
  };
  harness.store.record_success("takt-dash", snapshot, time("2026-11-13T12:00:00Z"));

  let detail = request_detail(&harness, "run-1").await.expect("完了後も同じ runSlug で詳細を取得できます");

  assert_eq!(detail.run_slug, "run-1");
  assert_eq!(detail.project, "takt-dash", "完了タスクの runSlug からプロジェクトを解決します");
  assert_eq!(detail.goal_id, "g1", "状態ストアの完了タスクからゴールを補います");
  assert_eq!(detail.task_name, "implement", "状態ストアの完了タスクからタスク名を補います");
  assert_eq!(detail.status, RunStatus::Completed as i32, "status は meta.json の値です");
}

#[tokio::test]
async fn get_run_detail_resolves_a_completed_task_that_only_has_a_source_run_slug() {
  // 完了タスクが runSlug を持たず sourceRunSlug だけを持つ場合でも、状態ストアの投影から元 run の
  // プロジェクトを解決し、対応表の source_run_slug から run の保存場所を解決できる。別の slug では
  // NotFound になることを対で観測する。
  let harness = GrpcHarness::builder()
    .single_project(
      project("takt-dash", project_dir()),
      server_meta(),
      json!({
        "goals": [],
        "tasks": [
          { "name": "implement", "goalId": "g1", "status": "completed", "startedAt": "2026-11-13T09:00:00Z", "sourceRunSlug": "run-source" }
        ],
        "runs": {
          "run-source": { "runSlug": "run-source", "sessionId": "sess-source", "summary": "元の run", "status": "completed", "stepLogs": [] }
        }
      }),
    )
    .build();
  let fixture = RunFixture::new(harness.project_dir());
  fixture.write_tasks(&[RunTaskLine { run_slug: None, source_run_slug: Some("run-source") }]);
  let mut meta = running_meta("review", 1, WORKFLOW, "2026-11-13T09:00:00Z");
  meta["status"] = json!("completed");
  fixture.write_meta("run-source", &meta);
  fixture.append_records("run-source", &[workflow_start_record(WORKFLOW)]);
  let mut poller = Poller::new(
    harness.config_path().to_path_buf(),
    harness.activity.clone(),
    harness.store.clone(),
    harness.changes.clone(),
    harness.registry.clone(),
  );
  poller.read_cycle_once().await.expect("1サイクルを実行できます");
  poller.shutdown().await;

  let detail = request_detail(&harness, "run-source")
    .await
    .expect("sourceRunSlug だけを持つ完了タスクからも run 詳細を取得できます");
  assert_eq!(detail.run_slug, "run-source");
  assert_eq!(detail.project, "takt-dash", "sourceRunSlug からプロジェクトを解決します");

  let status =
    request_detail(&harness, "does-not-exist").await.expect_err("sourceRunSlug と異なる slug は解決しません");
  assert_eq!(status.code(), tonic::Code::NotFound, "未知の runSlug は Not Found です: {status:?}");
}

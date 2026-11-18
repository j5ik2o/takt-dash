//! gRPC の GetRunContent が、指定したレポートまたは指示書の本文だけを返し、run のディレクトリ
//! （レポートは `reports/` の下、指示書はプロジェクト側の
//! `interventions.jsonl`）の外を読まないことを 検証する。本文の取得でも takt-mcp
//! を起動も呼び出しもしない。

mod common;
mod grpc_harness;

use std::path::Path;

use common::*;
use grpc_harness::{client, start_server, GrpcHarness};
use serde_json::json;
use takt_dash_backend::{
  model::{ExecutionStatus, GoalStatus, ProjectSnapshot},
  takt_dash::v1::{get_run_content_request::Target, GetRunContentRequest, GetRunContentResponse},
};

const WORKFLOW: &str = "development-core";
/// 計画が決めた本文の上限（1 MiB）。
const CONTENT_LIMIT_BYTES: usize = 1024 * 1024;
const SECRET: &str = "外の秘密の本文";

fn project_dir() -> &'static Path {
  Path::new("/abs/takt-dash")
}

/// 状態ストアに run-1 を持ち、プロジェクト側に run-1 の保存場所を持つ土台を作る。
fn seeded_harness() -> (GrpcHarness, RunFixture) {
  let harness =
    GrpcHarness::builder().single_project(project("takt-dash", project_dir()), server_meta(), json!({})).build();
  let snapshot = ProjectSnapshot {
    goals: vec![goal("g1", GoalStatus::Created, ExecutionStatus::Active)],
    tasks: vec![],
    inquiries: vec![],
    runs: vec![run("run-1", "implement", time("2026-11-13T09:06:00Z"))],
    notifications: vec![],
    proposals: vec![],
    ..ProjectSnapshot::empty()
  };
  harness.store.record_success("takt-dash", snapshot, time("2026-11-13T12:00:00Z"));
  let fixture = RunFixture::new(harness.project_dir());
  fixture.write_tasks(&[RunTaskLine { run_slug: Some("run-1"), source_run_slug: None }]);
  fixture.write_meta("run-1", &running_meta("implement", 1, WORKFLOW, "2026-11-13T09:00:00Z"));
  fixture.append_records("run-1", &[workflow_start_record(WORKFLOW)]);
  (harness, fixture)
}

fn report_request(run_slug: &str, filename: &str) -> GetRunContentRequest {
  GetRunContentRequest { run_slug: run_slug.to_owned(), target: Some(Target::ReportFilename(filename.to_owned())) }
}

fn instruction_request(run_slug: &str, instruction_id: i32) -> GetRunContentRequest {
  GetRunContentRequest { run_slug: run_slug.to_owned(), target: Some(Target::InstructionId(instruction_id)) }
}

async fn request_content(
  harness: &GrpcHarness,
  request: GetRunContentRequest,
) -> Result<GetRunContentResponse, tonic::Status> {
  let server = start_server(harness).await;
  let mut client = client(server.address).await;
  let result = client.get_run_content(request).await;
  server.stop().await;
  result.map(tonic::Response::into_inner)
}

#[tokio::test]
async fn get_run_content_returns_only_the_requested_nested_report() {
  let (harness, fixture) = seeded_harness();
  fixture.write_report("run-1", "plan.md", "計画の本文");
  fixture.write_report("run-1", "sub/review.md", "レビュー本文");

  let content =
    request_content(&harness, report_request("run-1", "sub/review.md")).await.expect("入れ子のレポートを取得できます");

  assert_eq!(content.run_slug, "run-1");
  assert_eq!(content.content, "レビュー本文", "指定したレポートの本文だけを返します");
  assert_eq!(content.size_bytes, "レビュー本文".len() as i64);
  assert!(!content.truncated);
}

#[tokio::test]
async fn get_run_content_does_not_read_outside_the_reports_directory() {
  // run の外の secret.md、run の meta.json、reports/ の外を指すシンボリックリンク、.md 以外の名前、
  // 絶対パスのどれを指定しても、外の内容を返さず INVALID_ARGUMENT か NOT_FOUND を返す。
  let (harness, fixture) = seeded_harness();
  let run_dir = fixture.run_dir("run-1");
  let mut meta = running_meta("implement", 1, WORKFLOW, "2026-11-13T09:00:00Z");
  meta["task"] = json!(SECRET);
  fixture.write_meta("run-1", &meta);
  let outside = harness.project_dir().join("secret.md");
  std::fs::write(&outside, SECRET).expect("run の外にファイルを書けます");
  fixture.write_report("run-1", "plan.md", "計画の本文");
  fixture.write_report("run-1", "plan.txt", SECRET);
  std::os::unix::fs::symlink(&outside, run_dir.join("reports").join("link.md"))
    .expect("外を指すシンボリックリンクを作れます");
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  for filename in [
    "link.md",
    "../meta.json",
    "../../../../secret.md",
    "/etc/hosts",
    outside.to_str().expect("一時ディレクトリのパスは UTF-8 です"),
    "plan.txt",
    "",
  ] {
    let result = client.get_run_content(report_request("run-1", filename)).await;

    match result {
      | Ok(response) => {
        let response = response.into_inner();
        panic!("report_filename={filename:?} は読みません。返った本文: {:?}", response.content);
      },
      | Err(status) => assert!(
        matches!(status.code(), tonic::Code::InvalidArgument | tonic::Code::NotFound),
        "report_filename={filename:?} は INVALID_ARGUMENT か NOT_FOUND です: {status:?}"
      ),
    }
  }
  server.stop().await;
}

#[tokio::test]
async fn get_run_content_returns_not_found_for_a_missing_report() {
  let (harness, fixture) = seeded_harness();
  fixture.write_report("run-1", "plan.md", "計画の本文");

  let status =
    request_content(&harness, report_request("run-1", "review.md")).await.expect_err("無いレポートは本文を返しません");

  assert_eq!(status.code(), tonic::Code::NotFound, "無いレポートは Not Found です: {status:?}");
}

#[tokio::test]
async fn get_run_content_truncates_a_report_larger_than_the_limit() {
  // 本文の上限を超えるレポートは上限までに切り詰め、truncated を立てて返す。
  let (harness, fixture) = seeded_harness();
  let large = "a".repeat(CONTENT_LIMIT_BYTES + 10);
  fixture.write_report("run-1", "large.md", &large);

  let content = request_content(&harness, report_request("run-1", "large.md")).await.expect("大きなレポートも返します");

  assert!(content.truncated, "上限を超えた本文は truncated です");
  assert!(content.content.len() <= CONTENT_LIMIT_BYTES, "本文は上限以内です: {}", content.content.len());
  assert!(large.starts_with(&content.content), "切り詰めた本文はレポートの先頭部分です");
}

#[tokio::test]
async fn get_run_content_returns_only_the_requested_instruction() {
  let (harness, fixture) = seeded_harness();
  fixture.write_interventions("run-1", &[
    issued_event(1, "2026-11-13T08:50:00Z", "一つ目"),
    issued_event(2, "2026-11-13T08:55:00Z", "二つ目"),
  ]);

  let content = request_content(&harness, instruction_request("run-1", 2)).await.expect("指示書を取得できます");

  assert_eq!(content.run_slug, "run-1");
  assert_eq!(content.content, "二つ目", "指定した ID の指示書の本文だけを返します");
  assert!(!content.content.contains("一つ目"), "ほかの指示書の本文を含めません");
  assert!(!content.truncated);
}

#[tokio::test]
async fn get_run_content_rejects_unknown_and_non_positive_instruction_ids() {
  let (harness, fixture) = seeded_harness();
  fixture.write_interventions("run-1", &[issued_event(1, "2026-11-13T08:50:00Z", "一つ目")]);
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let missing = client.get_run_content(instruction_request("run-1", 3)).await.expect_err("無い ID は返しません");
  let zero = client.get_run_content(instruction_request("run-1", 0)).await.expect_err("0 の ID は拒否します");
  let negative = client.get_run_content(instruction_request("run-1", -1)).await.expect_err("負の ID は拒否します");
  server.stop().await;

  assert_eq!(missing.code(), tonic::Code::NotFound, "無い ID は Not Found です: {missing:?}");
  assert_eq!(zero.code(), tonic::Code::InvalidArgument, "0 は Invalid Argument です: {zero:?}");
  assert_eq!(negative.code(), tonic::Code::InvalidArgument, "負の ID は Invalid Argument です: {negative:?}");
}

#[tokio::test]
async fn get_run_content_returns_not_found_for_an_unknown_run_slug() {
  let (harness, fixture) = seeded_harness();
  fixture.write_report("run-1", "plan.md", "計画の本文");

  let status = request_content(&harness, report_request("does-not-exist", "plan.md"))
    .await
    .expect_err("状態ストアにない run は本文を返しません");

  assert_eq!(status.code(), tonic::Code::NotFound, "状態ストアにない run は Not Found です: {status:?}");
}

#[tokio::test]
async fn get_run_content_returns_not_found_for_a_report_of_a_cleaned_worktree() {
  // 掃除済みの worktree の run のレポートは、本文が残っていないため Not Found です。
  let (harness, fixture) = seeded_harness();
  std::fs::remove_dir_all(fixture.run_dir("run-1")).expect("プロジェクト側の run を消せます");
  let worktree =
    harness.project_dir().parent().expect("プロジェクトの cwd には親があります").join("takt-worktrees").join("wt-gone");
  fixture.write_worktree_tasks(&[WorktreeTaskLine {
    run_slug:        Some("run-1"),
    source_run_slug: None,
    worktree_path:   &worktree,
  }]);

  let status = request_content(&harness, report_request("run-1", "plan.md"))
    .await
    .expect_err("掃除済みの worktree のレポートは返しません");

  assert_eq!(status.code(), tonic::Code::NotFound, "掃除済みの run のレポートは Not Found です: {status:?}");
}

#[tokio::test]
async fn get_run_content_neither_starts_nor_calls_takt_mcp() {
  let (harness, fixture) = seeded_harness();
  fixture.write_report("run-1", "plan.md", "計画の本文");
  fixture.write_interventions("run-1", &[issued_event(1, "2026-11-13T08:50:00Z", "一つ目")]);
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  client.get_run_content(report_request("run-1", "plan.md")).await.expect("レポートを取得できます");
  client.get_run_content(instruction_request("run-1", 1)).await.expect("指示書を取得できます");
  server.stop().await;

  let processes = harness.recorded_processes();
  let calls = harness.recorded_calls();
  assert!(processes.is_empty(), "GetRunContent は takt-mcp を起動しません: {processes:?}");
  assert!(calls.is_empty(), "GetRunContent は takt-mcp の道具を呼びません: {calls:?}");
}

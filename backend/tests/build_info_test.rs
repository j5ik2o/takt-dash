//! backend のビルド元情報（短縮 SHA とブランチ名）の契約（issue #73 / AC2）。
//!
//! ビルド時に git から取得した値がバイナリへ埋め込まれ、空・`HEAD`・不正な値は
//! `unknown` へ正規化されること、GetOverview と WatchState が同じ build_info を
//! 返すことを観測する。

mod common;
mod grpc_harness;

use std::path::Path;

use common::*;
use grpc_harness::{client, start_server, GrpcHarness};
use serde_json::json;
use takt_dash_backend::{
  build_info,
  takt_dash::v1::{GetOverviewRequest, WatchStateRequest},
};
use tokio::time::{timeout, Duration};

const UNKNOWN: &str = "unknown";

fn project_dir() -> &'static Path {
  Path::new("/abs/takt-dash")
}

fn harness() -> GrpcHarness {
  GrpcHarness::builder().single_project(project("takt-dash", project_dir()), server_meta(), json!({})).build()
}

fn is_short_commit(value: &str) -> bool {
  (7..=40).contains(&value.len()) && value.chars().all(|character| character.is_ascii_hexdigit())
}

/// ビルド時と同じ checkout をテスト実行時点でも参照するための git 出力。取得できなければ `None`。
fn git_output(args: &[&str]) -> Option<String> {
  let output = std::process::Command::new("git").args(args).current_dir(env!("CARGO_MANIFEST_DIR")).output().ok()?;
  if !output.status.success() {
    return None;
  }
  String::from_utf8(output.stdout).ok().map(|value| value.trim().to_owned())
}

#[test]
fn embedded_commit_and_branch_match_the_checkout_used_for_the_build() {
  let info = build_info::current();
  let Some(head) = git_output(&["rev-parse", "HEAD"]) else {
    assert_eq!(info.commit, UNKNOWN, "git から取得できない場合はコミットを unknown にする: {}", info.commit);
    assert_eq!(info.branch, UNKNOWN, "git から取得できない場合はブランチを unknown にする: {}", info.branch);
    return;
  };

  assert!(
    info.commit == UNKNOWN || head.starts_with(&info.commit),
    "埋め込むコミットはビルド元の HEAD {head} の短縮 SHA である必要があります: {}",
    info.commit
  );

  let expected_branch = git_output(&["rev-parse", "--abbrev-ref", "HEAD"])
    .map_or_else(|| UNKNOWN.to_owned(), |value| build_info::normalize_branch(&value).to_owned());
  assert_eq!(
    info.branch, expected_branch,
    "埋め込むブランチはビルド元の checkout のブランチ名と一致する必要があります"
  );
}

#[test]
fn current_build_info_is_embedded_with_a_usable_commit_and_branch() {
  let info = build_info::current();

  assert!(
    info.commit == UNKNOWN || is_short_commit(&info.commit),
    "埋め込むコミットは unknown か 7〜40 桁の16進の短縮 SHA である必要があります: {}",
    info.commit
  );
  assert!(
    !info.branch.trim().is_empty(),
    "埋め込むブランチ名は空であってはいけません（取得できない場合は unknown）: {:?}",
    info.branch
  );
}

#[test]
fn commit_normalization_rejects_empty_head_and_malformed_values() {
  assert_eq!(build_info::normalize_commit(""), UNKNOWN, "空文字は unknown にする");
  assert_eq!(build_info::normalize_commit("   "), UNKNOWN, "空白だけは unknown にする");
  assert_eq!(build_info::normalize_commit("HEAD"), UNKNOWN, "HEAD は unknown にする");
  assert_eq!(build_info::normalize_commit("not-a-sha"), UNKNOWN, "16進でない値は unknown にする");
  assert_eq!(build_info::normalize_commit("abc"), UNKNOWN, "短すぎる値は unknown にする");
  assert_eq!(build_info::normalize_commit(" a1b2c3d\n"), "a1b2c3d", "前後の空白を除いた短縮 SHA は残す");
}

#[test]
fn branch_normalization_rejects_empty_and_head() {
  assert_eq!(build_info::normalize_branch(""), UNKNOWN, "空文字は unknown にする");
  assert_eq!(build_info::normalize_branch("   "), UNKNOWN, "空白だけは unknown にする");
  assert_eq!(build_info::normalize_branch("HEAD"), UNKNOWN, "HEAD は unknown にする");
  assert_eq!(
    build_info::normalize_branch(" takt/20261116-build-info\n"),
    "takt/20261116-build-info",
    "前後の空白を除いたブランチ名は残す"
  );
}

#[tokio::test]
async fn overview_and_watch_state_expose_the_same_embedded_build_info() {
  let harness = harness();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let overview = client.get_overview(GetOverviewRequest {}).await.expect("GetOverview を呼べます").into_inner();
  let overview_info = overview.build_info.expect("GetOverview は build_info を返す必要があります");
  assert_eq!(
    overview_info,
    build_info::current(),
    "GetOverview の build_info はバイナリへ埋め込んだ値と一致する必要があります"
  );

  let mut stream = client.watch_state(WatchStateRequest {}).await.expect("WatchState へ接続できます").into_inner();
  let first = timeout(Duration::from_secs(5), stream.message())
    .await
    .expect("WatchState の初回状態は即時に届きます")
    .expect("ストリームから読めます")
    .expect("初回の状態があります");
  assert_eq!(
    first.build_info,
    Some(overview_info),
    "WatchState の初回状態も GetOverview と同じ build_info を返す必要があります"
  );

  server.stop().await;
}

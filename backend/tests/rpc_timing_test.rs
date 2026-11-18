//! unary RPC ごとの処理時間と SQLite の接続ロック待ちが、backend のログに分かれて
//! 出ることを、実ソケットのテストサーバと生成クライアントの経路で検証する（AC72A-1）。
//!
//! ログは tracing の購読者をメモリ上の writer へ差し替えて観測する。テストバイナリ内で
//! 購読者は1つだけ初期化し、テストも1件に絞って書き込み先の取り合いを避ける。

mod common;
mod grpc_harness;

use std::{
  io::Write,
  path::Path,
  sync::{Arc, Mutex, Once, OnceLock},
};

use common::*;
use grpc_harness::{client, start_server, GrpcHarness};
use serde_json::json;
use takt_dash_backend::takt_dash::v1::{
  GetActivityRequest, GetOverviewRequest, GetPendingDecisionsRequest, GetProjectsRequest, GetRunDetailRequest,
};
use tracing_subscriber::fmt::MakeWriter;

#[derive(Clone, Default)]
struct SharedWriter(Arc<Mutex<Vec<u8>>>);

impl SharedWriter {
  fn contents(&self) -> String {
    let bytes = self.0.lock().expect("ログのバッファのロックが壊れています").clone();
    String::from_utf8(bytes).expect("ログは UTF-8 です")
  }
}

impl Write for SharedWriter {
  fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
    self.0.lock().expect("ログのバッファのロックが壊れています").extend_from_slice(buffer);
    Ok(buffer.len())
  }

  fn flush(&mut self) -> std::io::Result<()> {
    Ok(())
  }
}

impl<'a> MakeWriter<'a> for SharedWriter {
  type Writer = SharedWriter;

  fn make_writer(&'a self) -> Self::Writer {
    self.clone()
  }
}

/// ログの購読者を1回だけ初期化し、書き込み先を返す。
fn captured_logs() -> SharedWriter {
  static WRITER: OnceLock<SharedWriter> = OnceLock::new();
  static INITIALIZE: Once = Once::new();
  let writer = WRITER.get_or_init(SharedWriter::default).clone();
  INITIALIZE.call_once(|| {
    tracing_subscriber::fmt()
      .with_writer(WRITER.get().expect("書き込み先は初期化済みです").clone())
      .with_max_level(tracing::Level::INFO)
      .with_ansi(false)
      .with_target(false)
      .init();
  });
  writer
}

/// 1行から `field=数値` の数値を取り出す。フィールドが無い、または数値でなければ None。
fn field_value(line: &str, field: &str) -> Option<u64> {
  let needle = format!("{field}=");
  let start = line.find(&needle)? + needle.len();
  let digits: String = line[start..].chars().take_while(|character| character.is_ascii_digit()).collect();
  if digits.is_empty() {
    None
  } else {
    digits.parse().ok()
  }
}

#[tokio::test]
async fn unary_rpc_logs_processing_time_and_sqlite_lock_wait() {
  let logs = captured_logs();

  let harness = GrpcHarness::builder()
    .single_project(project("takt-dash", Path::new("/abs/takt-dash")), server_meta(), json!({}))
    .build();
  harness.store.record_success(
    "takt-dash",
    snapshot(vec![], vec![], vec![], vec![], vec![]),
    time("2026-11-13T12:00:00Z"),
  );
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  client.get_overview(GetOverviewRequest {}).await.expect("概要を取得できます");
  client.get_projects(GetProjectsRequest {}).await.expect("プロジェクトを取得できます");
  client.get_pending_decisions(GetPendingDecisionsRequest {}).await.expect("判断待ちを取得できます");
  client.get_activity(GetActivityRequest {}).await.expect("アクティビティを取得できます");
  // 空の runSlug は状態ストアを読む前の失敗経路であり、全終端で記録されることを確かめる。
  let error = client
    .get_run_detail(GetRunDetailRequest { run_slug: String::new() })
    .await
    .expect_err("空の runSlug は失敗します");
  assert_eq!(error.code(), tonic::Code::InvalidArgument);

  server.stop().await;

  let logs = logs.contents();
  for rpc in ["GetOverview", "GetProjects", "GetPendingDecisions", "GetActivity", "GetRunDetail"] {
    let line = logs
      .lines()
      .find(|line| line.contains("rpc=") && line.contains(rpc) && line.contains("elapsed_us="))
      .unwrap_or_else(|| panic!("{rpc} の処理時間のログがありません:\n{logs}"));
    assert!(field_value(line, "elapsed_us").is_some(), "{rpc} の elapsed_us が数値ではありません: {line}");
    assert!(
      field_value(line, "lock_wait_us").is_some(),
      "{rpc} の lock_wait_us が処理時間と分かれて出ていません: {line}"
    );
  }
}

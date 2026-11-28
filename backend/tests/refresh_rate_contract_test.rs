//! issue #40 の PR 1a：proto に足す更新間隔の型（`RefreshRate`・`WatchStateRequest` の要求の欄・
//! `DashboardState.applied_refresh_interval_ms`）の生成物と、版ずれでの互換を検証する。
//!
//! PR 1a の backend は新しい欄を読まない。どの接続も要求なしとして今と同じに動き、`applied` は
//! どこでも 0（設計 docs/design/issue-40.md の「PR 1a」と D6）。0 は「要求なし」の意味なので、
//! 古い画面・古い backend との組み合わせでも嘘にならない。

mod common;
mod grpc_harness;

use std::path::Path;

use common::*;
use grpc_harness::{client, start_server, GrpcHarness};
use prost::Message;
use serde_json::json;
use takt_dash_backend::takt_dash::v1::{self, GetOverviewRequest, RefreshRate, WatchStateRequest};
use tokio::time::{timeout, Duration};

/// この機能の無い版の `WatchStateRequest`（欄なし）。古い backend が要求を読む型の代役。
#[derive(Clone, PartialEq, prost::Message)]
struct WatchStateRequestWithoutDemand {}

/// この機能の無い版の `DashboardState` の一部（`read_at` と
/// `build_info`）。古い画面が状態を読む型の代役。
#[derive(Clone, PartialEq, prost::Message)]
struct DashboardStateWithoutApplied {
  #[prost(message, optional, tag = "1")]
  read_at:    Option<prost_types::Timestamp>,
  #[prost(message, optional, tag = "8")]
  build_info: Option<v1::BuildInfo>,
}

fn read_at() -> prost_types::Timestamp {
  prost_types::Timestamp { seconds: 1_731_600_000, nanos: 0 }
}

fn build_info() -> v1::BuildInfo {
  v1::BuildInfo { commit: "a1b2c3d".to_owned(), branch: "main".to_owned() }
}

#[test]
fn refresh_rate_maps_each_choice_to_its_milliseconds_and_proto_name() {
  let choices = [
    (RefreshRate::Unspecified, 0, "REFRESH_RATE_UNSPECIFIED"),
    (RefreshRate::OneSecond, 1000, "REFRESH_RATE_ONE_SECOND"),
    (RefreshRate::TwoSeconds, 2000, "REFRESH_RATE_TWO_SECONDS"),
    (RefreshRate::FiveSeconds, 5000, "REFRESH_RATE_FIVE_SECONDS"),
    (RefreshRate::TenSeconds, 10000, "REFRESH_RATE_TEN_SECONDS"),
  ];
  for (rate, millis, name) in choices {
    assert_eq!(rate as i32, millis, "{name} の値はミリ秒の {millis} です");
    assert_eq!(rate.as_str_name(), name);
    assert_eq!(RefreshRate::from_str_name(name), Some(rate));
    assert_eq!(RefreshRate::try_from(millis), Ok(rate), "{millis} は {name} として読めます");
  }
}

#[test]
fn refresh_rate_accepts_only_the_declared_values() {
  // 受けるのは 0 と 4 つの選択肢だけ（設計 D5）。それ以外は生成 enum に無い値として区別できる。
  for millis in [-1, 1, 999, 1001, 1500, 3000, 9999, 10001, 15000] {
    assert!(RefreshRate::try_from(millis).is_err(), "{millis} は RefreshRate の値ではありません");
  }
}

#[test]
fn default_watch_state_request_carries_no_demand() {
  let request = WatchStateRequest::default();

  assert_eq!(request.refresh_rate, 0, "既定のレートは 0（要求なし）です");
  assert_eq!(request.refresh_rate(), RefreshRate::Unspecified);
  assert_eq!(request.screen, "", "既定の画面の識別子は空です");
  assert_eq!(request.attempt, 0_u64, "既定の試行番号は 0 です");
}

#[test]
fn default_dashboard_state_reports_no_applied_interval() {
  assert_eq!(v1::DashboardState::default().applied_refresh_interval_ms, 0_u32);
}

#[test]
fn old_web_request_without_demand_decodes_as_no_demand() {
  // 古い画面（と古い BFF）は欄の無い WatchStateRequest を送る。wire では空のバイト列になる。
  let old = WatchStateRequestWithoutDemand {}.encode_to_vec();
  assert!(old.is_empty());

  let decoded = WatchStateRequest::decode(old.as_slice()).expect("新しい backend は古い要求を読めます");
  assert_eq!(decoded, WatchStateRequest::default(), "古い要求は要求なしとして読みます");
}

#[test]
fn new_web_request_without_demand_is_the_same_bytes_as_the_old_request() {
  // 新しい画面が要求を付けないときは、古い backend へ送るバイト列も今と同じ（空）になる。
  assert!(WatchStateRequest::default().encode_to_vec().is_empty());
}

#[test]
fn new_web_demand_uses_fields_one_to_three_and_old_backend_ignores_them() {
  let request =
    WatchStateRequest { refresh_rate: RefreshRate::OneSecond as i32, screen: "s".to_owned(), attempt: 3 };
  let bytes = request.encode_to_vec();

  // 1 番（varint 1000）・2 番（長さ 1 の "s"）・3 番（varint 3）。番号を変えると古い版と食い違う。
  assert_eq!(bytes, vec![0x08, 0xE8, 0x07, 0x12, 0x01, 0x73, 0x18, 0x03]);
  assert_eq!(
    WatchStateRequestWithoutDemand::decode(bytes.as_slice()),
    Ok(WatchStateRequestWithoutDemand {}),
    "古い backend は知らない欄を読み飛ばし、要求なしとして扱えます"
  );
  assert_eq!(WatchStateRequest::decode(bytes.as_slice()), Ok(request));
}

#[test]
fn applied_interval_uses_field_nine_and_old_web_ignores_it() {
  let state = v1::DashboardState {
    read_at: Some(read_at()),
    build_info: Some(build_info()),
    applied_refresh_interval_ms: 1000,
    ..Default::default()
  };
  let bytes = state.encode_to_vec();

  assert_eq!(
    v1::DashboardState { applied_refresh_interval_ms: 1000, ..Default::default() }.encode_to_vec(),
    vec![0x48, 0xE8, 0x07],
    "applied_refresh_interval_ms は 9 番の varint です"
  );
  let old = DashboardStateWithoutApplied::decode(bytes.as_slice()).expect("古い画面は新しい状態を読めます");
  assert_eq!(old.read_at, Some(read_at()), "知らない 9 番を読み飛ばしても既存の欄は残ります");
  assert_eq!(old.build_info, Some(build_info()));
}

#[test]
fn state_from_an_old_backend_decodes_with_applied_zero() {
  // 古い backend は 9 番を送らない。新しい画面はそれを要求なし（0）として読む。
  let old = DashboardStateWithoutApplied { read_at: Some(read_at()), build_info: Some(build_info()) };

  let decoded = v1::DashboardState::decode(old.encode_to_vec().as_slice()).expect("新しい型で古い状態を読めます");
  assert_eq!(decoded.applied_refresh_interval_ms, 0);
  assert_eq!(decoded.read_at, Some(read_at()));
  assert_eq!(decoded.build_info, Some(build_info()));
}

fn harness() -> GrpcHarness {
  GrpcHarness::builder()
    .single_project(project("takt-dash", Path::new("/abs/takt-dash")), server_meta(), json!({}))
    .build()
}

#[tokio::test]
async fn overview_and_watch_state_without_demand_report_applied_zero() {
  let harness = harness();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let overview = client.get_overview(GetOverviewRequest {}).await.expect("GetOverview を呼べます").into_inner();
  assert_eq!(overview.applied_refresh_interval_ms, 0, "unary の応答は要求なし（0）です");

  let mut stream =
    client.watch_state(WatchStateRequest::default()).await.expect("WatchState へ接続できます").into_inner();
  let first = timeout(Duration::from_secs(5), stream.message())
    .await
    .expect("初回の状態は即時に届きます")
    .expect("ストリームから読めます")
    .expect("初回の状態があります");
  assert_eq!(first.applied_refresh_interval_ms, 0, "要求なしの接続の状態は 0 です");

  server.stop().await;
}

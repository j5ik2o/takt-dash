//! taktDash のバックエンド。
//!
//! このクレートは、proto/ の .proto から生成した gRPC の型とクライアントを公開し、
//! takt-mcp から集約した状態を中核の状態ストアとして公開します。
//! proto/taktdash.proto の TaktDashService（読み取り系 API と状態ストリーミング）は
//! [`grpc`] モジュールが実装します。

pub mod activity;
pub mod build_info;
pub mod config;
pub mod conversation;
pub mod domain;
pub mod events;
pub mod grpc;
pub mod mcp;
pub mod model;
pub mod poller;
pub mod run_log;
pub mod store;

pub mod takt_dash {
  pub mod v1 {
    tonic::include_proto!("takt_dash.v1");
  }
}

#[cfg(test)]
mod tests {
  #[test]
  fn the_test_harness_runs() {
    assert_eq!(1 + 1, 2);
  }
}

//! taktDash のバックエンド。
//!
//! このクレートは、proto/ の .proto から生成した gRPC の型とクライアントを公開します。
//! takt-mcp からの状態の集約と gRPC サーバーの実装は、後続のタスクでこのクレートへ足します。

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

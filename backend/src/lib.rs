//! taktDash のバックエンド。
//!
//! このクレートは、土台の段階では最小のライブラリだけを持ちます。
//! gRPC の読み取り系 API と takt-mcp からの状態の集約は、後続のタスクでこのクレートへ足します。

#[cfg(test)]
mod tests {
  #[test]
  fn the_test_harness_runs() {
    assert_eq!(1 + 1, 2);
  }
}

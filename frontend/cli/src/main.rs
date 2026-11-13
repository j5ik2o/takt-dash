//! taktDash のコマンドラインの入口。
//!
//! このクレートは、土台の段階では最小のバイナリだけを持ちます。
//! バックエンドの gRPC へ接続する閲覧コマンドは、後続のタスクでこのクレートへ足します。

mod proto;

/// コマンドラインの起動引数を受け取り、実行結果を終了コードとして返します。
///
/// 土台の段階では、打ち込まれた引数をそのまま標準出力へ書き出します。
#[must_use]
pub fn run(args: &[String]) -> i32 {
  println!("takt-dash-cli: {}", args.join(" "));
  0
}

fn main() {
  let args: Vec<String> = std::env::args().skip(1).collect();
  std::process::exit(run(&args));
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn run_succeeds_for_any_arguments() {
    assert_eq!(run(&[]), 0);
    assert_eq!(run(&["overview".to_owned()]), 0);
  }
}

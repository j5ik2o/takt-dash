use std::{
  path::{Path, PathBuf},
  process::Command,
};

/// git を実行して標準出力を前後の空白を除いて返す。失敗したら空文字を返す。
fn git_output(args: &[&str]) -> String {
  Command::new("git")
    .args(args)
    .current_dir(env!("CARGO_MANIFEST_DIR"))
    .output()
    .ok()
    .filter(|output| output.status.success())
    .and_then(|output| String::from_utf8(output.stdout).ok())
    .map(|value| value.trim().to_owned())
    .unwrap_or_default()
}

/// 監視する git のパスを `--git-path` で解決する。worktree で `.git` がファイルになる場合や
/// `GIT_DIR` がリポジトリ外を指す場合も git 自身に解決させ、実在するパスだけを返す。
fn git_watch_path(spec: &str) -> Option<PathBuf> {
  let value = git_output(&["rev-parse", "--git-path", spec]);
  if value.is_empty() {
    return None;
  }
  let path = PathBuf::from(value);
  let resolved = if path.is_absolute() { path } else { Path::new(env!("CARGO_MANIFEST_DIR")).join(path) };
  resolved.exists().then_some(resolved)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
  tonic_prost_build::configure().compile_protos(&["../proto/taktdash.proto"], &["../proto"])?;
  println!("cargo:rerun-if-changed=../proto/taktdash.proto");

  // 埋め込むコミットとブランチは git の HEAD とブランチ tip に依存する。proto だけを監視すると、
  // proto を変更しないコミットやブランチ切り替えの後に古い値が再利用されるため、
  // 両方の変化で再実行する。
  for spec in ["HEAD", "refs/heads"] {
    if let Some(path) = git_watch_path(spec) {
      println!("cargo:rerun-if-changed={}", path.display());
    }
  }

  // バイナリ自身のビルド元を表す。取得できない環境では空文字を埋め込み、
  // 実行時に `unknown` へ正規化する。
  println!("cargo:rustc-env=TAKTDASH_BUILD_COMMIT={}", git_output(&["rev-parse", "--short", "HEAD"]));
  println!("cargo:rustc-env=TAKTDASH_BUILD_BRANCH={}", git_output(&["rev-parse", "--abbrev-ref", "HEAD"]));
  Ok(())
}

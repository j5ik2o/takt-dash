//! ビルド元の情報（短縮コミット SHA とブランチ名）。
//!
//! `build.rs` が git から取得して `cargo:rustc-env` で埋め込んだ値を読み、空・`HEAD`・
//! 不正な値を `unknown` へ正規化して返します。取得できない環境でもビルドを壊しません。

use crate::takt_dash::v1;

/// 取得できない場合に表示する値。
const UNKNOWN: &str = "unknown";

/// `build.rs` が埋め込むビルド元の短縮コミット SHA。
const EMBEDDED_COMMIT: &str = env!("TAKTDASH_BUILD_COMMIT");
/// `build.rs` が埋め込むビルド元のブランチ名。
const EMBEDDED_BRANCH: &str = env!("TAKTDASH_BUILD_BRANCH");

/// バイナリへ埋め込んだビルド元の情報を返す。
pub fn current() -> v1::BuildInfo {
  v1::BuildInfo {
    commit: normalize_commit(EMBEDDED_COMMIT).to_owned(),
    branch: normalize_branch(EMBEDDED_BRANCH).to_owned(),
  }
}

/// コミットの値を正規化する。空・空白・`HEAD`・7〜40 桁の16進でない値は `unknown` にする。
pub fn normalize_commit(value: &str) -> &str {
  let trimmed = value.trim();
  let usable = (7..=40).contains(&trimmed.len()) && trimmed.chars().all(|c| c.is_ascii_hexdigit());
  if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("HEAD") || !usable {
    UNKNOWN
  } else {
    trimmed
  }
}

/// ブランチ名を正規化する。空・空白・`HEAD` は `unknown` にする。
pub fn normalize_branch(value: &str) -> &str {
  let trimmed = value.trim();
  if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("HEAD") {
    UNKNOWN
  } else {
    trimmed
  }
}

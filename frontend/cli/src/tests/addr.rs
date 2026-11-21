//! 接続先の解決、設定ファイル、引数から表示までの到達経路。

use std::net::SocketAddr;

use super::{
  fixtures::sample_overview,
  support::{reserve_closed_addr, start_dummy_server, write_config},
};
use crate::{
  read_config_listen, resolve_backend_addr, resolve_overview_addr, run_with, AddrError, AddrSources, OverviewOptions,
  BACKEND_ADDR_ENV, DEFAULT_BACKEND_ADDR, EXIT_FAILURE, EXIT_OK, EXIT_USAGE,
};

// ----------------------------------------------------------------------------
// CLI-ADDR-RESOLVE: 接続先の解決（要件 18, 19）
// ----------------------------------------------------------------------------

#[test]
fn explicit_addr_wins_over_env_and_config() {
  let sources = AddrSources {
    cli_addr:      Some("127.0.0.1:50101"),
    env_addr:      Ok("127.0.0.1:50202".to_owned()),
    config_listen: Some("127.0.0.1:50303"),
  };

  assert_eq!(resolve_backend_addr(&sources), Ok("127.0.0.1:50101".parse().unwrap()));
}

#[test]
fn env_wins_over_config_listen() {
  let sources = AddrSources {
    cli_addr:      None,
    env_addr:      Ok("127.0.0.1:50202".to_owned()),
    config_listen: Some("127.0.0.1:50303"),
  };

  assert_eq!(resolve_backend_addr(&sources), Ok("127.0.0.1:50202".parse().unwrap()));
}

#[test]
fn config_listen_used_when_addr_and_env_absent() {
  let sources = AddrSources {
    cli_addr:      None,
    env_addr:      Err(std::env::VarError::NotPresent),
    config_listen: Some("127.0.0.1:50303"),
  };

  assert_eq!(resolve_backend_addr(&sources), Ok("127.0.0.1:50303".parse().unwrap()));
}

#[test]
fn default_addr_used_when_nothing_configured() {
  let sources =
    AddrSources { cli_addr: None, env_addr: Err(std::env::VarError::NotPresent), config_listen: None };

  assert_eq!(resolve_backend_addr(&sources), Ok("127.0.0.1:50051".parse().unwrap()));
  assert_eq!(DEFAULT_BACKEND_ADDR, "127.0.0.1:50051");
}

#[test]
fn empty_values_fall_through_to_next_candidate() {
  let sources =
    AddrSources { cli_addr: Some(""), env_addr: Ok(String::new()), config_listen: Some("127.0.0.1:50303") };

  assert_eq!(resolve_backend_addr(&sources), Ok("127.0.0.1:50303".parse().unwrap()));
}

#[test]
fn invalid_addr_is_not_replaced_with_default() {
  let sources = AddrSources {
    cli_addr:      Some("not-an-address"),
    env_addr:      Err(std::env::VarError::NotPresent),
    config_listen: None,
  };

  assert_eq!(
    resolve_backend_addr(&sources),
    Err(AddrError::Invalid { source: "--addr", value: "not-an-address".to_owned() })
  );
}

#[test]
fn invalid_addr_exits_with_usage_error() {
  let mut stdout = Vec::new();
  let mut stderr = Vec::new();

  let code = run_with(
    &["overview".to_owned(), "--addr".to_owned(), "not-an-address".to_owned()],
    Err(std::env::VarError::NotPresent),
    &mut stdout,
    &mut stderr,
  );

  let stderr = String::from_utf8(stderr).expect("標準エラーは UTF-8 である必要があります");
  assert_eq!(code, EXIT_USAGE, "--addr の不正値は既定値へ置き換えず異常終了する必要があります");
  assert!(stderr.contains("not-an-address"), "不正な値を示す必要があります: {stderr}");
}

#[test]
fn config_listen_overrides_default_for_connection() {
  let dir = tempfile::tempdir().expect("一時ディレクトリを作れませんでした");
  let config = write_config(&dir, r#"{"listen":"127.0.0.1:59999"}"#);

  let mut stdout = Vec::new();
  let mut stderr = Vec::new();
  let code = run_with(
    &["--config".to_owned(), config.display().to_string()],
    Err(std::env::VarError::NotPresent),
    &mut stdout,
    &mut stderr,
  );

  let stderr = String::from_utf8(stderr).expect("標準エラーは UTF-8 である必要があります");
  // 設定の listen が使われ（既定値ではない）、待ち受けが無いため接続不能で終わる。
  assert_eq!(code, EXIT_FAILURE, "設定の listen に待ち受けが無いため失敗する必要があります");
  assert!(stderr.contains("127.0.0.1:59999"), "設定の listen を接続先に使う必要があります: {stderr}");
  assert!(!stderr.contains(DEFAULT_BACKEND_ADDR), "既定値へ戻ってはいけません: {stderr}");
}

#[test]
fn missing_config_file_falls_through_to_default() {
  let dir = tempfile::tempdir().expect("一時ディレクトリを作れませんでした");
  let missing = dir.path().join("does-not-exist.json");

  // 設定ファイルが無いときは listen を未設定として扱い、解決は既定値へ進む。
  let listen = read_config_listen(&missing).expect("存在しない設定ファイルは未設定として扱う必要があります");
  assert_eq!(listen, None, "存在しない設定ファイルは listen を返しません");
  let sources = AddrSources {
    cli_addr:      None,
    env_addr:      Err(std::env::VarError::NotPresent),
    config_listen: listen.as_deref(),
  };
  assert_eq!(
    resolve_backend_addr(&sources),
    Ok(DEFAULT_BACKEND_ADDR.parse().unwrap()),
    "設定ファイルが無いときは既定値へ進む必要があります"
  );

  // 接続結果は環境に依存するため、待ち受けの無い未使用ポートを明示して決定的に観測する。
  let closed = reserve_closed_addr();
  let mut stdout = Vec::new();
  let mut stderr = Vec::new();
  let code = run_with(
    &["--config".to_owned(), missing.display().to_string(), "--addr".to_owned(), closed.to_string()],
    Err(std::env::VarError::NotPresent),
    &mut stdout,
    &mut stderr,
  );

  let stderr = String::from_utf8(stderr).expect("標準エラーは UTF-8 である必要があります");
  assert_eq!(code, EXIT_FAILURE, "待ち受けの無い接続先では失敗する必要があります: {stderr}");
  assert!(stderr.contains(&closed.to_string()), "明示した接続先を使う必要があります: {stderr}");
  assert!(!stderr.contains(DEFAULT_BACKEND_ADDR), "明示した接続先があるときは既定値へ戻ってはいけません: {stderr}");
}

#[test]
fn explicit_addr_skips_config_read_even_if_config_is_invalid() {
  // 上位の接続先指定（--addr）がある場合、設定ファイルは読まない。壊れた設定を置いても
  // 解析エラーにならず、--addr のダミーサーバーから概要を取得できることを観測する。
  let runtime =
    tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("テスト用の実行環境を作れませんでした");
  let server = runtime.block_on(start_dummy_server(sample_overview()));
  let dir = tempfile::tempdir().expect("一時ディレクトリを作れませんでした");
  let config = write_config(&dir, "{ これは JSON ではありません }");
  let mut stdout = Vec::new();
  let mut stderr = Vec::new();
  let args = [
    "overview".to_owned(),
    "--addr".to_owned(),
    format!("{}", server.addr),
    "--config".to_owned(),
    config.display().to_string(),
  ];

  let code = run_with(&args, Err(std::env::VarError::NotPresent), &mut stdout, &mut stderr);
  let seen_calls = server.seen_calls();
  runtime.block_on(server.shutdown());

  let stdout = String::from_utf8(stdout).expect("標準出力は UTF-8 である必要があります");
  let stderr = String::from_utf8(stderr).expect("標準エラーは UTF-8 である必要があります");
  assert_eq!(code, EXIT_OK, "--addr があるときは設定ファイルの解析で止まってはいけません。stderr: {stderr}");
  assert!(stderr.is_empty(), "成功時に設定ファイルのエラーを出してはいけません: {stderr}");
  assert_eq!(seen_calls, vec!["GetOverview".to_owned()], "--addr の接続先から概要を取得する必要があります");
  assert!(stdout.contains("概要"), "--addr の接続先から取得した概要を表示する必要があります: {stdout}");
}

#[test]
fn env_addr_skips_config_read_even_if_config_is_invalid() {
  // 上位の接続先指定（環境変数）がある場合も設定ファイルは読まない。
  let runtime =
    tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("テスト用の実行環境を作れませんでした");
  let server = runtime.block_on(start_dummy_server(sample_overview()));
  let dir = tempfile::tempdir().expect("一時ディレクトリを作れませんでした");
  let config = write_config(&dir, "{ これは JSON ではありません }");
  let mut stdout = Vec::new();
  let mut stderr = Vec::new();
  let args = ["overview".to_owned(), "--config".to_owned(), config.display().to_string()];

  let code = run_with(&args, Ok(format!("{}", server.addr)), &mut stdout, &mut stderr);
  let seen_calls = server.seen_calls();
  runtime.block_on(server.shutdown());

  let stdout = String::from_utf8(stdout).expect("標準出力は UTF-8 である必要があります");
  let stderr = String::from_utf8(stderr).expect("標準エラーは UTF-8 である必要があります");
  assert_eq!(code, EXIT_OK, "環境変数があるときは設定ファイルの解析で止まってはいけません。stderr: {stderr}");
  assert!(stderr.is_empty(), "成功時に設定ファイルのエラーを出してはいけません: {stderr}");
  assert_eq!(seen_calls, vec!["GetOverview".to_owned()], "環境変数の接続先から概要を取得する必要があります");
  assert!(stdout.contains("概要"), "環境変数の接続先から取得した概要を表示する必要があります: {stdout}");
}

#[test]
fn invalid_config_fails_when_no_higher_addr_is_specified() {
  // 上位の指定が無い場合だけ設定ファイルを読み、解析できなければ接続前に失敗する。
  let dir = tempfile::tempdir().expect("一時ディレクトリを作れませんでした");
  let config = write_config(&dir, "{ これは JSON ではありません }");
  let mut stdout = Vec::new();
  let mut stderr = Vec::new();

  let code = run_with(
    &["overview".to_owned(), "--config".to_owned(), config.display().to_string()],
    Err(std::env::VarError::NotPresent),
    &mut stdout,
    &mut stderr,
  );

  let stdout = String::from_utf8(stdout).expect("標準出力は UTF-8 である必要があります");
  let stderr = String::from_utf8(stderr).expect("標準エラーは UTF-8 である必要があります");
  assert_eq!(code, EXIT_FAILURE, "上位の指定が無いときの設定エラーは失敗にする必要があります");
  assert!(stderr.contains("JSON を解析できませんでした"), "設定の解析失敗を示す必要があります: {stderr}");
  assert!(stdout.is_empty(), "設定エラーで概要を標準出力へ出してはいけません: {stdout}");
}

#[test]
fn empty_env_addr_reads_config_as_unset() {
  // 空の環境変数は「上位の指定なし」として扱い、設定ファイルを読む。
  let dir = tempfile::tempdir().expect("一時ディレクトリを作れませんでした");
  let config = write_config(&dir, "{ これは JSON ではありません }");
  let mut stdout = Vec::new();
  let mut stderr = Vec::new();

  let code = run_with(
    &["overview".to_owned(), "--config".to_owned(), config.display().to_string()],
    Ok(String::new()),
    &mut stdout,
    &mut stderr,
  );

  let stderr = String::from_utf8(stderr).expect("標準エラーは UTF-8 である必要があります");
  assert_eq!(code, EXIT_FAILURE, "空の環境変数は未設定として設定ファイルを読む必要があります");
  assert!(stderr.contains("JSON を解析できませんでした"), "設定の解析失敗を示す必要があります: {stderr}");
}

#[test]
fn invalid_env_addr_does_not_fall_through_to_config() {
  // 非空だが不正な環境変数を「未設定」と扱い、設定の listen へ落ちてはいけない。
  let dir = tempfile::tempdir().expect("一時ディレクトリを作れませんでした");
  let listen = reserve_closed_addr();
  let config = write_config(&dir, &format!(r#"{{"listen":"{listen}"}}"#));
  let mut stdout = Vec::new();
  let mut stderr = Vec::new();

  let code = run_with(
    &["overview".to_owned(), "--config".to_owned(), config.display().to_string()],
    Ok("not-an-address".to_owned()),
    &mut stdout,
    &mut stderr,
  );

  let stderr = String::from_utf8(stderr).expect("標準エラーは UTF-8 である必要があります");
  assert_eq!(code, EXIT_USAGE, "不正な環境変数は既定値や設定の listen で補ってはいけません");
  assert!(stderr.contains("not-an-address"), "不正な値を示す必要があります: {stderr}");
  assert!(stderr.contains(BACKEND_ADDR_ENV), "どの指定が不正かを示す必要があります: {stderr}");
}

#[test]
fn resolve_overview_addr_falls_back_to_default_when_config_file_is_missing() {
  // 所有者関数 resolve_overview_addr を通して、設定ファイルの欠落が「未設定」として
  // 既定値へ解決されることを、ソケットを開かずに観測する。
  let dir = tempfile::tempdir().expect("一時ディレクトリを作れませんでした");
  let missing = dir.path().join("does-not-exist.json");
  let options = OverviewOptions { addr: None, config: Some(missing) };

  assert_eq!(
    resolve_overview_addr(&options, Err(std::env::VarError::NotPresent)),
    Ok(DEFAULT_BACKEND_ADDR.parse::<SocketAddr>().unwrap()),
    "設定ファイルが無いときは所有者関数も既定値へ解決する必要があります"
  );
}

// ----------------------------------------------------------------------------
// 到達経路: 引数解析・接続先解決・表示が一つの入口からつながる
// ----------------------------------------------------------------------------

#[test]
fn cli_end_to_end_from_arguments_to_stdout() {
  let runtime =
    tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("テスト用の実行環境を作れませんでした");
  let server = runtime.block_on(start_dummy_server(sample_overview()));

  // 環境変数ではなく --addr で接続先を渡す。表示は標準出力へ出る。
  let (code, stdout, stderr) = {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let args = ["overview".to_owned(), "--addr".to_owned(), format!("{}", server.addr)];
    let code = run_with(&args, Ok("127.0.0.1:1".to_owned()), &mut stdout, &mut stderr);
    (code, String::from_utf8(stdout).unwrap(), String::from_utf8(stderr).unwrap())
  };
  let seen_calls = server.seen_calls();
  runtime.block_on(server.shutdown());

  assert_eq!(code, EXIT_OK, "stderr: {stderr}");
  assert_eq!(seen_calls, vec!["GetOverview".to_owned()], "取得は GetOverview だけを呼ぶ必要があります");
  assert!(stdout.contains("概要"), "--addr の接続先から取得した概要を表示する必要があります: {stdout}");
  assert!(stdout.contains("j5ik2o-mac-mini（Mac mini、接続中）"), "{stdout}");
}

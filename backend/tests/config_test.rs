//! 設定ファイルの読み込み・検証・既定値・相対パス解決を検証する。

mod common;

use std::path::Path;

use takt_dash_backend::config::{
  load_config, parse_config, ConfigError, DEFAULT_INTERVAL_SECONDS, DEFAULT_PENDING_DECISION_STALE_MINUTES,
  DEFAULT_RUN_LOG_SILENT_MINUTES,
};

fn repo_config_path() -> &'static Path {
  Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../taktdash.config.json"))
}

#[test]
fn committed_config_parses_with_four_projects_and_mac_mini_server() {
  let config = load_config(repo_config_path()).expect("コミット済みの設定ファイルを読み込めます");

  let names: Vec<&str> = config.projects.iter().map(|project| project.name.as_str()).collect();
  assert_eq!(names, vec!["corporate-site", "event-store-adapter", "takt", "takt-dash"]);
  let mini = config.servers.get("mac-mini").expect("mac-mini の起動方法があります");
  assert_eq!(mini.command, "node");
  assert!(mini.args.iter().any(|arg| arg.ends_with("dist/app/mcp/index.js")));
  for project in &config.projects {
    assert_eq!(project.server, "mac-mini");
    assert!(project.cwd.is_absolute(), "{} の cwd は絶対パスです", project.name);
  }
}

#[test]
fn committed_config_carries_default_interval_and_thresholds() {
  let config = load_config(repo_config_path()).expect("設定ファイルを読み込めます");
  assert_eq!(config.interval_seconds, 30);
  assert_eq!(config.thresholds.pending_decision_stale_minutes, 240);
  assert_eq!(config.thresholds.run_log_silent_minutes, 10);
}

#[test]
fn committed_config_places_sqlite_under_gitignored_file() {
  let config = load_config(repo_config_path()).expect("設定ファイルを読み込めます");
  let file_name = config.sqlite_path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
  assert!(file_name.ends_with(".sqlite"), "SQLite の既定は .sqlite を含みます: {}", config.sqlite_path.display());
}

#[test]
fn omitted_interval_and_thresholds_fall_back_to_defaults() {
  let text = r#"{
    "mcpServers": { "mini": { "command": "node", "args": [] } },
    "projects": [ { "name": "p", "server": "mini", "cwd": "/abs/p" } ]
  }"#;
  let config = parse_config(text, Path::new("/base")).expect("既定値が適用されます");

  assert_eq!(config.interval_seconds, DEFAULT_INTERVAL_SECONDS);
  assert_eq!(config.thresholds.pending_decision_stale_minutes, DEFAULT_PENDING_DECISION_STALE_MINUTES);
  assert_eq!(config.thresholds.run_log_silent_minutes, DEFAULT_RUN_LOG_SILENT_MINUTES);
  assert_eq!(config.sqlite_path, Path::new("/base/data/taktdash.sqlite"));
}

#[test]
fn explicit_thresholds_override_defaults() {
  let text = r#"{
    "mcpServers": { "mini": { "command": "node", "args": [] } },
    "projects": [],
    "intervalSeconds": 5,
    "thresholds": { "pendingDecisionStaleMinutes": 60, "runLogSilentMinutes": 3 }
  }"#;
  let config = parse_config(text, Path::new("/base")).expect("設定ファイルを解析できます");

  assert_eq!(config.interval_seconds, 5);
  assert_eq!(config.thresholds.pending_decision_stale_minutes, 60);
  assert_eq!(config.thresholds.run_log_silent_minutes, 3);
}

#[test]
fn relative_sqlite_path_resolves_against_config_directory() {
  let text = r#"{
    "mcpServers": { "mini": { "command": "node", "args": [] } },
    "projects": [],
    "sqlite": { "path": "var/data.db" }
  }"#;
  let config = parse_config(text, Path::new("/etc/taktdash")).expect("設定ファイルを解析できます");
  assert_eq!(config.sqlite_path, Path::new("/etc/taktdash/var/data.db"));
}

#[test]
fn absolute_sqlite_path_is_used_as_is() {
  let text = r#"{
    "mcpServers": { "mini": { "command": "node", "args": [] } },
    "projects": [],
    "sqlite": { "path": "/var/taktdash.sqlite" }
  }"#;
  let config = parse_config(text, Path::new("/etc/taktdash")).expect("設定ファイルを解析できます");
  assert_eq!(config.sqlite_path, Path::new("/var/taktdash.sqlite"));
}

#[test]
fn project_referencing_unknown_server_is_rejected() {
  let text = r#"{
    "mcpServers": { "mini": { "command": "node", "args": [] } },
    "projects": [ { "name": "p", "server": "ghost", "cwd": "/abs/p" } ]
  }"#;
  let error = parse_config(text, Path::new("/base")).expect_err("未知のサーバー参照は拒否します");
  assert!(
    matches!(&error, ConfigError::Invalid(message) if message.contains("ghost")),
    "未知のサーバー名を含む検証エラーになります: {error}"
  );
}

#[test]
fn project_with_relative_cwd_is_rejected() {
  let text = r#"{
    "mcpServers": { "mini": { "command": "node", "args": [] } },
    "projects": [ { "name": "p", "server": "mini", "cwd": "relative/path" } ]
  }"#;
  let error = parse_config(text, Path::new("/base")).expect_err("相対パスの cwd は拒否します");
  assert!(
    matches!(&error, ConfigError::Invalid(message) if message.contains("絶対パス")),
    "絶対パスを求める検証エラーになります: {error}"
  );
}

#[test]
fn empty_mcp_servers_is_rejected() {
  let text = r#"{ "mcpServers": {}, "projects": [] }"#;
  let error = parse_config(text, Path::new("/base")).expect_err("mcpServers が空なら拒否します");
  assert!(matches!(error, ConfigError::Invalid(_)), "検証エラーになります: {error}");
}

#[test]
fn zero_interval_is_rejected() {
  let text = r#"{
    "mcpServers": { "mini": { "command": "node", "args": [] } },
    "projects": [],
    "intervalSeconds": 0
  }"#;
  let error = parse_config(text, Path::new("/base")).expect_err("0 秒の間隔は拒否します");
  assert!(matches!(error, ConfigError::Invalid(_)), "検証エラーになります: {error}");
}

#[test]
fn malformed_json_is_reported_as_parse_error() {
  let error = parse_config("{ not json", Path::new("/base")).expect_err("壊れた JSON は拒否します");
  assert!(matches!(error, ConfigError::Parse { .. }), "解析エラーになります: {error}");
}

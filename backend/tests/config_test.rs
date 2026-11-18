//! 設定ファイルの読み込み・検証・既定値・相対パス解決を検証する。

mod common;

use std::path::Path;

use takt_dash_backend::config::{
  load_config, parse_config, ConfigError, DEFAULT_CONNECTION_DELAY_SECONDS, DEFAULT_CONNECTION_FAILURE_COUNT,
  DEFAULT_INTERVAL_SECONDS, DEFAULT_LISTEN, DEFAULT_PENDING_DECISION_STALE_MINUTES, DEFAULT_RUN_LOG_SILENT_MINUTES,
  DEFAULT_SCREEN_READ_LANES, MAX_SCREEN_READ_LANES,
};

fn repo_config_path() -> &'static Path {
  Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../taktdash.config.example.json"))
}

#[test]
fn example_config_parses_with_four_projects_and_mac_mini_server() {
  let config = load_config(repo_config_path()).expect("見本の設定ファイルを読み込めます");

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
fn example_config_carries_default_interval_and_thresholds() {
  let config = load_config(repo_config_path()).expect("設定ファイルを読み込めます");
  assert_eq!(config.interval_seconds, 30);
  assert_eq!(config.thresholds.pending_decision_stale_minutes.get(), 240);
  assert_eq!(config.thresholds.run_log_silent_minutes.get(), 10);
  assert_eq!(config.thresholds.connection_failure_count, 3);
  assert_eq!(config.thresholds.connection_delay_seconds, 120);
}

#[test]
fn example_config_places_sqlite_under_gitignored_file() {
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
  assert_eq!(
    config.thresholds.pending_decision_stale_minutes.get(),
    i32::try_from(DEFAULT_PENDING_DECISION_STALE_MINUTES).expect("既定値は i32 の範囲です")
  );
  assert_eq!(
    config.thresholds.run_log_silent_minutes.get(),
    i32::try_from(DEFAULT_RUN_LOG_SILENT_MINUTES).expect("既定値は i32 の範囲です")
  );
  assert_eq!(config.thresholds.connection_failure_count, DEFAULT_CONNECTION_FAILURE_COUNT);
  assert_eq!(config.thresholds.connection_delay_seconds, DEFAULT_CONNECTION_DELAY_SECONDS);
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
  assert_eq!(config.thresholds.pending_decision_stale_minutes.get(), 60);
  assert_eq!(config.thresholds.run_log_silent_minutes.get(), 3);
}

#[test]
fn explicit_connection_thresholds_override_defaults() {
  let text = r#"{
    "mcpServers": { "mini": { "command": "node", "args": [] } },
    "projects": [],
    "thresholds": { "connectionFailureCount": 5, "connectionDelaySeconds": 30 }
  }"#;
  let config = parse_config(text, Path::new("/base")).expect("設定ファイルを解析できます");

  assert_eq!(config.thresholds.connection_failure_count, 5);
  assert_eq!(config.thresholds.connection_delay_seconds, 30);
  assert_eq!(
    config.thresholds.pending_decision_stale_minutes.get(),
    i32::try_from(DEFAULT_PENDING_DECISION_STALE_MINUTES).expect("既定値は i32 の範囲です"),
    "接続の閾値だけを変えても既存の閾値は既定のままです"
  );
}

#[test]
fn zero_connection_failure_count_is_rejected() {
  let text = r#"{
    "mcpServers": { "mini": { "command": "node", "args": [] } },
    "projects": [],
    "thresholds": { "connectionFailureCount": 0 }
  }"#;
  let error = parse_config(text, Path::new("/base")).expect_err("0回の連続失敗は拒否します");
  assert!(matches!(error, ConfigError::Invalid(_)), "検証エラーになります: {error}");
}

#[test]
fn zero_connection_delay_seconds_is_rejected() {
  let text = r#"{
    "mcpServers": { "mini": { "command": "node", "args": [] } },
    "projects": [],
    "thresholds": { "connectionDelaySeconds": 0 }
  }"#;
  let error = parse_config(text, Path::new("/base")).expect_err("0秒の遅延は拒否します");
  assert!(matches!(error, ConfigError::Invalid(_)), "検証エラーになります: {error}");
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
fn duplicate_project_name_is_rejected_at_the_config_boundary() {
  let text = r#"{
    "mcpServers": { "mini": { "command": "node", "args": [] } },
    "projects": [
      { "name": "takt-dash", "server": "mini", "cwd": "/abs/takt-dash" },
      { "name": "takt-dash", "server": "mini", "cwd": "/abs/takt-dash-2" }
    ]
  }"#;
  let error = parse_config(text, Path::new("/base")).expect_err("同名プロジェクトは拒否します");
  assert!(
    matches!(&error, ConfigError::Invalid(message) if message.contains("takt-dash")),
    "重複したプロジェクト名を含む検証エラーになります: {error}"
  );
}

#[test]
fn distinct_project_names_are_accepted_even_when_only_case_differs() {
  let text = r#"{
    "mcpServers": { "mini": { "command": "node", "args": [] } },
    "projects": [
      { "name": "ESA", "server": "mini", "cwd": "/abs/esa-upper" },
      { "name": "esa", "server": "mini", "cwd": "/abs/esa-lower" }
    ]
  }"#;
  let config =
    parse_config(text, Path::new("/base")).expect("大小文字だけが異なる名前は別プロジェクトとして読み込めます");
  let names: Vec<&str> = config.projects.iter().map(|project| project.name.as_str()).collect();
  assert_eq!(names, vec!["ESA", "esa"], "大小文字を正規化せず2件として読み込みます");
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

#[test]
fn example_config_listens_on_the_default_address() {
  let config = load_config(repo_config_path()).expect("設定ファイルを読み込めます");
  assert_eq!(config.listen.to_string(), DEFAULT_LISTEN, "listen を書かない設定は既定の listen を使います");
}

#[test]
fn omitted_listen_falls_back_to_the_default_localhost_address() {
  let text = r#"{
    "mcpServers": { "mini": { "command": "node", "args": [] } },
    "projects": [ { "name": "p", "server": "mini", "cwd": "/abs/p" } ]
  }"#;
  let config = parse_config(text, Path::new("/base")).expect("既定値が適用されます");

  assert_eq!(config.listen.to_string(), DEFAULT_LISTEN);
  assert_eq!(config.listen.ip().to_string(), "127.0.0.1", "既定の待ち受けは 127.0.0.1 です");
  assert_eq!(config.listen.port(), 50051, "既定の待ち受けポートは 50051 です");
}

#[test]
fn explicit_listen_overrides_the_default() {
  let text = r#"{
    "mcpServers": { "mini": { "command": "node", "args": [] } },
    "projects": [],
    "listen": "0.0.0.0:9000"
  }"#;
  let config = parse_config(text, Path::new("/base")).expect("設定ファイルを解析できます");

  assert_eq!(config.listen.to_string(), "0.0.0.0:9000");
}

#[test]
fn malformed_listen_is_rejected_at_the_config_boundary() {
  let text = r#"{
    "mcpServers": { "mini": { "command": "node", "args": [] } },
    "projects": [],
    "listen": "not-an-address"
  }"#;
  let error = parse_config(text, Path::new("/base")).expect_err("不正な listen は拒否します");
  assert!(
    matches!(&error, ConfigError::Invalid(message) if message.contains("listen")),
    "listen を含む検証エラーになります: {error}"
  );
}

#[test]
fn mcp_server_ids_that_are_blank_or_contain_control_characters_are_rejected() {
  for id in ["   ", "mini\n", "mini\u{0}", "mini\u{7f}"] {
    let key = serde_json::to_string(id).expect("JSON の文字列にできます");
    let text = format!(r#"{{ "mcpServers": {{ {key}: {{ "command": "node", "args": [] }} }}, "projects": [] }}"#);
    let error =
      parse_config(&text, Path::new("/base")).expect_err("空白だけ・制御文字を含む mcpServers のキーは拒否します");
    assert!(matches!(error, ConfigError::Invalid(_)), "mcpServers のキー {id:?} は検証エラーになります: {error}");
  }
}

#[test]
fn server_metadata_ids_that_are_blank_or_contain_control_characters_are_rejected() {
  for id in ["   ", "mini\n", "mini\u{0}", "mini\u{7f}"] {
    let key = serde_json::to_string(id).expect("JSON の文字列にできます");
    let text = format!(
      r#"{{ "mcpServers": {{ "mini": {{ "command": "node", "args": [] }} }}, "servers": {{ {key}: {{ "name": "mini", "kind": "takt", "concurrency": 1 }} }}, "projects": [] }}"#
    );
    let error =
      parse_config(&text, Path::new("/base")).expect_err("空白だけ・制御文字を含む servers のキーは拒否します");
    assert!(matches!(error, ConfigError::Invalid(_)), "servers のキー {id:?} は検証エラーになります: {error}");
  }
}

#[test]
fn project_names_that_are_blank_or_contain_control_characters_are_rejected() {
  for name in ["   ", "takt\ndash", "takt\u{0}dash", "takt\u{7f}dash"] {
    let value = serde_json::to_string(name).expect("JSON の文字列にできます");
    let text = format!(
      r#"{{ "mcpServers": {{ "mini": {{ "command": "node", "args": [] }} }}, "projects": [ {{ "name": {value}, "server": "mini", "cwd": "/abs/p" }} ] }}"#
    );
    let error =
      parse_config(&text, Path::new("/base")).expect_err("空白だけ・制御文字を含むプロジェクト名は拒否します");
    assert!(matches!(error, ConfigError::Invalid(_)), "プロジェクト名 {name:?} は検証エラーになります: {error}");
  }
}

#[test]
fn thresholds_beyond_the_proto_int32_range_are_rejected_but_the_maximum_is_accepted() {
  let over = r#"{
    "mcpServers": { "mini": { "command": "node", "args": [] } },
    "projects": [],
    "thresholds": { "pendingDecisionStaleMinutes": 2147483648 }
  }"#;
  let error = parse_config(over, Path::new("/base")).expect_err("i32 の上限を超える滞留の閾値は拒否します");
  assert!(matches!(error, ConfigError::Invalid(_)), "滞留の閾値の範囲外は検証エラーになります: {error}");

  let over_silent = r#"{
    "mcpServers": { "mini": { "command": "node", "args": [] } },
    "projects": [],
    "thresholds": { "runLogSilentMinutes": 2147483648 }
  }"#;
  let error = parse_config(over_silent, Path::new("/base")).expect_err("i32 の上限を超える無音の閾値は拒否します");
  assert!(matches!(error, ConfigError::Invalid(_)), "無音の閾値の範囲外は検証エラーになります: {error}");

  let maximum = r#"{
    "mcpServers": { "mini": { "command": "node", "args": [] } },
    "projects": [],
    "thresholds": { "pendingDecisionStaleMinutes": 2147483647, "runLogSilentMinutes": 2147483647 }
  }"#;
  parse_config(maximum, Path::new("/base")).expect("i32 の上限ちょうどの閾値は受け入れます");
}

/// プロジェクトの cwd に .takt/config.yaml を置いた状態で load_config が解決する
/// プロジェクトの同時実行上限を返す。takt_config が None のときは .takt 自体を作らない。
fn concurrency_resolved_from_takt_config(takt_config: Option<&str>) -> i32 {
  let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
  let project_dir = directory.path().join("proj");
  std::fs::create_dir_all(&project_dir).expect("プロジェクトのディレクトリを作れます");
  if let Some(text) = takt_config {
    std::fs::create_dir_all(project_dir.join(".takt")).expect(".takt を作れます");
    std::fs::write(project_dir.join(".takt/config.yaml"), text).expect("takt の設定を書けます");
  }
  let config_path = directory.path().join("taktdash.config.json");
  let config_text = format!(
    r#"{{ "mcpServers": {{ "mini": {{ "command": "node", "args": [] }} }},
         "servers": {{ "mini": {{ "name": "Mac mini", "kind": "Mac mini" }} }},
         "projects": [ {{ "name": "proj", "server": "mini", "cwd": "{}" }} ] }}"#,
    project_dir.display()
  );
  std::fs::write(&config_path, config_text).expect("設定を書けます");
  let config = load_config(&config_path).expect("設定を読み込めます");
  config.projects[0].concurrency
}

#[test]
fn project_concurrency_comes_from_the_project_takt_config() {
  assert_eq!(
    concurrency_resolved_from_takt_config(Some("concurrency: 3\n")),
    3,
    ".takt/config.yaml の concurrency をプロジェクトの上限にする"
  );
  assert_eq!(
    concurrency_resolved_from_takt_config(Some("concurrency: 2 # 同時実行の上限\n")),
    2,
    "値の後ろのコメントは無視する"
  );
}

#[test]
fn project_concurrency_defaults_to_one_without_a_takt_value() {
  assert_eq!(concurrency_resolved_from_takt_config(None), 1, ".takt/config.yaml が無ければ takt の既定値1にする");
  assert_eq!(
    concurrency_resolved_from_takt_config(Some("other: true\n")),
    1,
    "concurrency キーが無ければ既定値1にする"
  );
  assert_eq!(
    concurrency_resolved_from_takt_config(Some("nested:\n  concurrency: 5\n")),
    1,
    "ネストした同名キーは対象にしない"
  );
  assert_eq!(concurrency_resolved_from_takt_config(Some("concurrency: 0\n")), 1, "0以下は既定値1にする");
  assert_eq!(
    concurrency_resolved_from_takt_config(Some("concurrency: many\n")),
    1,
    "数値として読めなければ既定値1にする"
  );
}

#[test]
fn project_concurrency_accepts_valid_yaml_notations() {
  assert_eq!(
    concurrency_resolved_from_takt_config(Some("\"concurrency\": 3\n")),
    3,
    "キーを二重引用した表記でも上限を読む"
  );
  assert_eq!(
    concurrency_resolved_from_takt_config(Some("'concurrency': 4\n")),
    4,
    "キーを単一引用した表記でも上限を読む"
  );
  assert_eq!(
    concurrency_resolved_from_takt_config(Some("  other: true\n  concurrency: 5\n")),
    5,
    "ルート全体をインデントした表記でも上限を読む"
  );
  assert_eq!(
    concurrency_resolved_from_takt_config(Some("concurrency:\n  6\n")),
    6,
    "数値を次行に置く表記でも上限を読む"
  );
  assert_eq!(concurrency_resolved_from_takt_config(Some("{concurrency: 7}\n")), 7, "フローマッピングでも上限を読む");
  assert_eq!(
    concurrency_resolved_from_takt_config(Some("{ other: true, concurrency: 8 }\n")),
    8,
    "フローマッピングに他のキーがあっても上限を読む"
  );
}

#[test]
fn project_concurrency_handles_quotes_inside_plain_scalars_in_flow_mappings() {
  assert_eq!(
    concurrency_resolved_from_takt_config(Some("{base_branch: feature/o'brien, concurrency: 8}\n")),
    8,
    "フローマッピングで通常文字列の中のアポストロフィを引用の開始と誤認しない"
  );
  assert_eq!(
    concurrency_resolved_from_takt_config(Some("{base_branch: \"say \\\"hi, there\\\"\", concurrency: 9}\n")),
    9,
    "フローマッピングで二重引用の中のエスケープとカンマを項目の区切りと誤認しない"
  );
}

#[test]
fn project_concurrency_rejects_multi_line_plain_scalars() {
  assert_eq!(
    concurrency_resolved_from_takt_config(Some("concurrency:\n  3\n  invalid\n")),
    1,
    "次行の値に継続行が続く場合は複数行の通常スカラーであり、値全体が数値でないため既定値1にする"
  );
  assert_eq!(
    concurrency_resolved_from_takt_config(Some("concurrency: 3\n  invalid\n")),
    1,
    "同行の値に継続行が続く場合も複数行の通常スカラーとして既定値1にする"
  );
}

#[test]
fn omitted_screen_read_lanes_falls_back_to_default() {
  let text = r#"{
    "mcpServers": { "mini": { "command": "node", "args": [] } },
    "projects": [ { "name": "p", "server": "mini", "cwd": "/abs/p" } ]
  }"#;
  let config = parse_config(text, Path::new("/base")).expect("既定値が適用されます");
  assert_eq!(config.screen_read_lanes.get(), DEFAULT_SCREEN_READ_LANES, "画面読み取りレーンの既定は1本です");
}

#[test]
fn screen_read_lanes_is_clamped_into_range() {
  let base = |lanes: u32| {
    format!(
      r#"{{
        "mcpServers": {{ "mini": {{ "command": "node", "args": [] }} }},
        "projects": [ {{ "name": "p", "server": "mini", "cwd": "/abs/p" }} ],
        "screenReadLanes": {lanes}
      }}"#
    )
  };
  let config = parse_config(&base(2), Path::new("/base")).expect("範囲内の値はそのまま使います");
  assert_eq!(config.screen_read_lanes.get(), 2, "範囲内の値はそのまま使います");
  let config = parse_config(&base(0), Path::new("/base")).expect("0は下限へ丸めます");
  assert_eq!(config.screen_read_lanes.get(), 1, "0は下限の1へ丸めます");
  let config = parse_config(&base(9), Path::new("/base")).expect("上限超は上限へ丸めます");
  assert_eq!(config.screen_read_lanes.get(), MAX_SCREEN_READ_LANES, "上限を超える値は上限へ丸めます");
}

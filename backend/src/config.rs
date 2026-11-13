//! 接続設定の読み込み・検証・既定値の適用。
//!
//! 設定は .mcp.json と同じ mcpServers 形式を基にした1ファイルで、takt-mcp の起動方法・
//! サーバー・プロジェクト・読み取り間隔・異常判定の閾値を同じファイルに持ちます。
//! 値の解決はこの境界で一度だけ行い、以降の内部処理は解決済みの値だけを使います。

use std::{
  collections::BTreeMap,
  path::{Path, PathBuf},
};

use serde::Deserialize;

/// 読み取り間隔の既定値（秒）。
pub const DEFAULT_INTERVAL_SECONDS: u64 = 30;
/// 判断待ちの滞留の既定値（分）。
pub const DEFAULT_PENDING_DECISION_STALE_MINUTES: u32 = 240;
/// ログの無音の既定値（分）。
pub const DEFAULT_RUN_LOG_SILENT_MINUTES: u32 = 10;
/// SQLite の置き場所の既定値（設定ファイルのディレクトリ基準の相対パス）。
pub const DEFAULT_SQLITE_PATH: &str = "data/taktdash.sqlite";

/// 設定ファイルの読み込みと検証で起きる失敗。
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
  #[error("設定ファイル {path} を読み込めませんでした: {source}")]
  Read { path: String, source: std::io::Error },
  #[error("設定ファイル {path} の JSON を解析できませんでした: {source}")]
  Parse { path: String, source: serde_json::Error },
  #[error("設定の内容が不正です: {0}")]
  Invalid(String),
}

/// 検証済みの接続設定。相対パスは読み込み時に絶対パスへ解決済みです。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
  pub servers:          BTreeMap<String, McpServer>,
  pub server_meta:      BTreeMap<String, ServerMeta>,
  pub projects:         Vec<Project>,
  pub interval_seconds: u64,
  pub thresholds:       Thresholds,
  pub sqlite_path:      PathBuf,
}

/// takt-mcp の起動方法。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServer {
  pub command: String,
  pub args:    Vec<String>,
  pub env:     BTreeMap<String, String>,
}

/// サーバーの表示情報。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerMeta {
  pub name:        String,
  pub kind:        String,
  pub concurrency: i32,
}

/// 集約対象のプロジェクト。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
  pub name:   String,
  pub server: String,
  pub about:  String,
  pub cwd:    PathBuf,
}

/// 異常判定の閾値。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Thresholds {
  pub pending_decision_stale_minutes: u32,
  pub run_log_silent_minutes:         u32,
}

#[derive(Debug, Deserialize)]
struct RawConfig {
  #[serde(rename = "mcpServers", default)]
  mcp_servers:      BTreeMap<String, RawMcpServer>,
  #[serde(default)]
  servers:          BTreeMap<String, RawServerMeta>,
  #[serde(default)]
  projects:         Vec<RawProject>,
  #[serde(rename = "intervalSeconds")]
  interval_seconds: Option<u64>,
  #[serde(default)]
  thresholds:       RawThresholds,
  #[serde(default)]
  sqlite:           RawSqlite,
}

#[derive(Debug, Deserialize)]
struct RawMcpServer {
  command: String,
  #[serde(default)]
  args:    Vec<String>,
  #[serde(default)]
  env:     BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct RawServerMeta {
  name:        Option<String>,
  kind:        Option<String>,
  concurrency: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct RawProject {
  name:   String,
  server: String,
  about:  Option<String>,
  cwd:    String,
}

#[derive(Debug, Default, Deserialize)]
struct RawThresholds {
  #[serde(rename = "pendingDecisionStaleMinutes")]
  pending_decision_stale_minutes: Option<u32>,
  #[serde(rename = "runLogSilentMinutes")]
  run_log_silent_minutes:         Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
struct RawSqlite {
  path: Option<String>,
}

/// 設定ファイルを読み込み、検証と既定値の適用を済ませた [`Config`] を返す。
pub fn load_config(path: &Path) -> Result<Config, ConfigError> {
  let text =
    std::fs::read_to_string(path).map_err(|source| ConfigError::Read { path: path.display().to_string(), source })?;
  let base_dir = path.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
  parse_config(&text, &base_dir)
}

/// 設定の本文と、相対パスの解決に使う基準ディレクトリから [`Config`] を組み立てる。
pub fn parse_config(text: &str, base_dir: &Path) -> Result<Config, ConfigError> {
  let raw: RawConfig =
    serde_json::from_str(text).map_err(|source| ConfigError::Parse { path: base_dir.display().to_string(), source })?;
  let mut servers = BTreeMap::new();
  for (id, server) in raw.mcp_servers {
    servers.insert(id, McpServer { command: server.command, args: server.args, env: server.env });
  }
  if servers.is_empty() {
    return Err(ConfigError::Invalid("mcpServers が空です。takt-mcp の起動方法を1つ以上指定してください。".to_owned()));
  }
  let mut server_meta = BTreeMap::new();
  for (id, meta) in raw.servers {
    let name = meta.name.ok_or_else(|| ConfigError::Invalid(format!("サーバー {id} の name がありません。")))?;
    let kind = meta.kind.ok_or_else(|| ConfigError::Invalid(format!("サーバー {id} の kind がありません。")))?;
    let concurrency =
      meta.concurrency.ok_or_else(|| ConfigError::Invalid(format!("サーバー {id} の concurrency がありません。")))?;
    server_meta.insert(id, ServerMeta { name, kind, concurrency });
  }
  let mut projects = Vec::new();
  for project in raw.projects {
    if !servers.contains_key(&project.server) {
      return Err(ConfigError::Invalid(format!(
        "プロジェクト {} が参照するサーバー {} は mcpServers にありません。",
        project.name, project.server
      )));
    }
    let cwd = PathBuf::from(&project.cwd);
    if !cwd.is_absolute() {
      return Err(ConfigError::Invalid(format!(
        "プロジェクト {} の cwd {} は絶対パスである必要があります。",
        project.name, project.cwd
      )));
    }
    projects.push(Project {
      name: project.name,
      server: project.server,
      about: project.about.unwrap_or_default(),
      cwd,
    });
  }
  let sqlite_raw = raw.sqlite.path.unwrap_or_else(|| DEFAULT_SQLITE_PATH.to_owned());
  let sqlite_path =
    if Path::new(&sqlite_raw).is_absolute() { PathBuf::from(sqlite_raw) } else { base_dir.join(sqlite_raw) };
  let interval_seconds = raw.interval_seconds.unwrap_or(DEFAULT_INTERVAL_SECONDS);
  if interval_seconds == 0 {
    return Err(ConfigError::Invalid("intervalSeconds は1秒以上である必要があります。".to_owned()));
  }
  let thresholds = Thresholds {
    pending_decision_stale_minutes: raw
      .thresholds
      .pending_decision_stale_minutes
      .unwrap_or(DEFAULT_PENDING_DECISION_STALE_MINUTES),
    run_log_silent_minutes:         raw.thresholds.run_log_silent_minutes.unwrap_or(DEFAULT_RUN_LOG_SILENT_MINUTES),
  };
  Ok(Config { servers, server_meta, projects, interval_seconds, thresholds, sqlite_path })
}

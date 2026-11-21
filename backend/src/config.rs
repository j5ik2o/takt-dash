//! 接続設定の読み込み・検証・既定値の適用。
//!
//! 設定は .mcp.json と同じ mcpServers 形式を基にした1ファイルで、takt-mcp の起動方法・
//! サーバー・プロジェクト・読み取り間隔・異常判定の閾値を同じファイルに持ちます。
//! 値の解決はこの境界で一度だけ行い、以降の内部処理は解決済みの値だけを使います。
//!
//! mcpServers の各エントリは、子プロセスを起動する stdio 接続と、常駐する takt-mcp の
//! HTTP サーバーへつなぐ http 接続のどちらかです。読み取りと書き込みは、それぞれの接続先
//! （`server` と `writeServer`）が指すエントリの接続方法を使います。同じ mcpServers に
//! 対象サーバーごとのエントリを登録します。

use std::{
  collections::BTreeMap,
  net::{IpAddr, SocketAddr},
  path::{Path, PathBuf},
};

use serde::Deserialize;

use crate::domain::{ProjectName, ScreenReadLanes, ServerId, SilentMinutes, StaleMinutes};

/// 読み取り間隔の既定値（秒）。
pub const DEFAULT_INTERVAL_SECONDS: u64 = 30;
/// 判断待ちの滞留の既定値（分）。
pub const DEFAULT_PENDING_DECISION_STALE_MINUTES: u32 = 240;
/// ログの無音の既定値（分）。
pub const DEFAULT_RUN_LOG_SILENT_MINUTES: u32 = 10;
/// 接続の応答なしと判定する連続失敗回数の既定値。
pub const DEFAULT_CONNECTION_FAILURE_COUNT: u32 = 3;
/// 接続の遅延中を応答なしへ移す経過時間の既定値（秒）。
pub const DEFAULT_CONNECTION_DELAY_SECONDS: u32 = 120;
/// SQLite の置き場所の既定値（設定ファイルのディレクトリ基準の相対パス）。
pub const DEFAULT_SQLITE_PATH: &str = "data/taktdash.sqlite";
/// gRPC サーバーの待ち受けアドレスの既定値。
pub const DEFAULT_LISTEN: &str = "127.0.0.1:50051";
/// takt-mcp の HTTP サーバーが待ち受けるホスト。実機の transport は 127.0.0.1 固定です。
pub const HTTP_LOOPBACK_HOST: &str = "127.0.0.1";
/// プロジェクトの同時実行上限の既定値。プロジェクトの .takt/config.yaml に
/// concurrency キーが無い場合の takt 自身の既定値に合わせる。
pub const DEFAULT_PROJECT_CONCURRENCY: i32 = 1;
/// 画面読み取りレーンの本数の既定値（プロジェクトごと）。
pub const DEFAULT_SCREEN_READ_LANES: u32 = 1;
/// 画面読み取りレーンの本数の上限（プロジェクトごと）。子プロセスの増加を抑えるため、
/// 設定値がこれを超える場合は上限へ丸めます。
pub const MAX_SCREEN_READ_LANES: u32 = ScreenReadLanes::MAX;

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
  pub servers:           BTreeMap<String, McpServer>,
  pub server_meta:       BTreeMap<String, ServerMeta>,
  pub projects:          Vec<Project>,
  pub interval_seconds:  u64,
  /// 画面からの読み取りに使う takt-mcp 接続レーンの本数（プロジェクトごと、1〜3）。
  pub screen_read_lanes: ScreenReadLanes,
  pub thresholds:        Thresholds,
  pub sqlite_path:       PathBuf,
  /// gRPC サーバーの待ち受けアドレス。設定の境界で SocketAddr へ解決済みです。
  pub listen:            SocketAddr,
}

/// takt-mcp への接続方法。stdio は子プロセスを起動する起動方法、http は常駐サーバーの接続先です。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpServer {
  Stdio(StdioMcpServer),
  Http(HttpMcpServer),
}

impl McpServer {
  /// この接続方法を、接続先の種類ごとの [`McpConnection`] へ写す。
  pub fn connection(&self) -> McpConnection {
    match self {
      | McpServer::Stdio(server) => McpConnection::Stdio(server.clone()),
      | McpServer::Http(server) => McpConnection::Http(server.clone()),
    }
  }
}

/// 接続方法を、その種類だけを持つ接続に解決したもの。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpConnection {
  Stdio(StdioMcpServer),
  Http(HttpMcpServer),
}

impl McpConnection {
  /// SSH 越しに子プロセスを起動する接続か。コマンドの実行ファイル名が `ssh` のとき SSH
  /// とみなします。
  pub fn is_ssh(&self) -> bool {
    match self {
      | McpConnection::Stdio(server) => {
        std::path::Path::new(&server.command).file_name().and_then(|name| name.to_str()) == Some("ssh")
      },
      | McpConnection::Http(_) => false,
    }
  }
}

/// 子プロセスとして起動する takt-mcp の起動方法。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StdioMcpServer {
  pub command: String,
  pub args:    Vec<String>,
  pub env:     BTreeMap<String, String>,
}

/// 常駐する takt-mcp の HTTP サーバーへの接続先。読み取り専用の読み取り経路がこれを使います。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpMcpServer {
  pub endpoint:      HttpServerEndpoint,
  /// `Authorization` ヘッダにそのまま設定する値（`Bearer <token>`）。
  pub authorization: String,
}

/// takt-mcp の HTTP サーバーの接続先。ループバック `127.0.0.1` の `http://` だけを表せます。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpServerEndpoint {
  port: u16,
}

impl HttpServerEndpoint {
  pub fn port(&self) -> u16 {
    self.port
  }

  /// `http://127.0.0.1:<port>/` の URL を組み立てる。
  pub fn url(&self) -> String {
    format!("http://{HTTP_LOOPBACK_HOST}:{}/", self.port)
  }
}

impl TryFrom<&str> for HttpServerEndpoint {
  type Error = ConfigError;

  fn try_from(url: &str) -> Result<Self, Self::Error> {
    let rest = url
      .strip_prefix("http://")
      .ok_or_else(|| ConfigError::Invalid(format!("HTTP 接続先 {url} は http:// で始まる必要があります。")))?;
    let authority = rest.split('/').next().unwrap_or_default();
    if authority.is_empty() {
      return Err(ConfigError::Invalid(format!("HTTP 接続先 {url} にホストとポートがありません。")));
    }
    let (host, port_text) = authority
      .rsplit_once(':')
      .ok_or_else(|| ConfigError::Invalid(format!("HTTP 接続先 {url} にポート番号がありません。")))?;
    let host: IpAddr = host.parse().map_err(|_| {
      ConfigError::Invalid(format!(
        "HTTP 接続先 {url} のホストを解釈できませんでした。ホストは {HTTP_LOOPBACK_HOST} である必要があります。"
      ))
    })?;
    if !host.is_loopback() || host.to_string() != HTTP_LOOPBACK_HOST {
      return Err(ConfigError::Invalid(format!(
        "HTTP 接続先 {url} のホストは {HTTP_LOOPBACK_HOST} である必要があります。実機の takt-mcp は 127.0.0.1 のみで待ち受けます。"
      )));
    }
    let port: u16 = port_text.parse().map_err(|_| {
      ConfigError::Invalid(format!("HTTP 接続先 {url} のポート番号 {port_text} を解釈できませんでした。"))
    })?;
    if port == 0 {
      return Err(ConfigError::Invalid(format!("HTTP 接続先 {url} のポート番号は 1 以上である必要があります。")));
    }
    let path = &rest[authority.len()..];
    if !path.is_empty() && path != "/" {
      return Err(ConfigError::Invalid(format!(
        "HTTP 接続先 {url} のパス {path} はサポートしません。実機の takt-mcp は / だけを公開します。"
      )));
    }
    Ok(HttpServerEndpoint { port })
  }
}

/// サーバーの表示情報。同時実行の上限はサーバー単位には存在しないため持たない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerMeta {
  pub name: String,
  pub kind: String,
}

/// 集約対象のプロジェクト。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
  pub name:         String,
  /// 読み取り接続先（`mcpServers` の id）。画面と定期読み取りが使います。
  pub server:       String,
  /// 書き込み接続先（`mcpServers` の id）。未設定なら `server` と同じ。
  pub write_server: String,
  pub about:        String,
  pub cwd:          PathBuf,
  /// 同時実行の上限。プロジェクトの .takt/config.yaml の concurrency から解決し、
  /// キーが無い場合は takt の既定値にする。parse_config では既定値を入れ、
  /// load_config がファイルから解決した値で置き換える。
  pub concurrency:  i32,
}

/// 異常判定の閾値。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Thresholds {
  pub pending_decision_stale_minutes: StaleMinutes,
  pub run_log_silent_minutes:         SilentMinutes,
  pub connection_failure_count:       u32,
  pub connection_delay_seconds:       u32,
}

#[derive(Debug, Deserialize)]
struct RawConfig {
  #[serde(rename = "mcpServers", default)]
  mcp_servers:       BTreeMap<String, RawMcpServer>,
  #[serde(default)]
  servers:           BTreeMap<String, RawServerMeta>,
  #[serde(default)]
  projects:          Vec<RawProject>,
  #[serde(rename = "intervalSeconds")]
  interval_seconds:  Option<u64>,
  #[serde(rename = "screenReadLanes")]
  screen_read_lanes: Option<u32>,
  #[serde(default)]
  thresholds:        RawThresholds,
  #[serde(default)]
  sqlite:            RawSqlite,
  listen:            Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawMcpServer {
  #[serde(rename = "type")]
  kind:    Option<String>,
  command: Option<String>,
  #[serde(default)]
  args:    Vec<String>,
  #[serde(default)]
  env:     BTreeMap<String, String>,
  url:     Option<String>,
  #[serde(default)]
  headers: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct RawServerMeta {
  name: Option<String>,
  kind: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawProject {
  name:         String,
  server:       String,
  #[serde(rename = "writeServer")]
  write_server: Option<String>,
  about:        Option<String>,
  cwd:          String,
}

#[derive(Debug, Default, Deserialize)]
struct RawThresholds {
  #[serde(rename = "pendingDecisionStaleMinutes")]
  pending_decision_stale_minutes: Option<u32>,
  #[serde(rename = "runLogSilentMinutes")]
  run_log_silent_minutes:         Option<u32>,
  #[serde(rename = "connectionFailureCount")]
  connection_failure_count:       Option<u32>,
  #[serde(rename = "connectionDelaySeconds")]
  connection_delay_seconds:       Option<u32>,
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
  let mut config = parse_config(&text, &base_dir)?;
  for project in &mut config.projects {
    project.concurrency = read_project_concurrency(&project.cwd);
  }
  Ok(config)
}

/// プロジェクトの .takt/config.yaml から同時実行の上限を読む。ファイルが無い・
/// キーが無い・値が不正な場合は takt の既定値を使う。表示に使う値であり、
/// 読めないことを設定エラーにはしない。
fn read_project_concurrency(cwd: &Path) -> i32 {
  let path = cwd.join(".takt").join("config.yaml");
  let Ok(text) = std::fs::read_to_string(&path) else {
    return DEFAULT_PROJECT_CONCURRENCY;
  };
  parse_takt_concurrency(&text).unwrap_or(DEFAULT_PROJECT_CONCURRENCY)
}

/// takt の設定（YAML）のトップレベルの concurrency を取り出す。ネストした
/// 同名のキーは対象にしない。値が正の整数でない場合は None を返す。
///
/// 対応する表記は、平坦なブロックマッピングに加えて、キーの引用（単一・
/// 二重）、ルート全体のインデント、数値を次の行へ置く書き方、1行のフロー
/// マッピングである。引用はスカラーの開始位置にある場合だけ引用として
/// 扱い、通常スカラーの途中に現れる引用符は文字として扱う。複数行に
/// またがる通常スカラーは、値全体を1行で表せないため対象にしない。
/// アンカーや複数行にまたがるフローマッピングなどの完全な YAML 構文も
/// 対象にしない。
fn parse_takt_concurrency(text: &str) -> Option<i32> {
  let lines = yaml_content_lines(text);
  let first = lines.first().map(|(_, content)| *content)?;
  parse_block_concurrency(&lines).or_else(|| parse_flow_concurrency(first))
}

/// ブロックマッピングからトップレベルの concurrency を取り出す。ネストした
/// 同名のキーは対象にしない。値が正の整数でない場合は None を返す。
fn parse_block_concurrency(lines: &[(usize, &str)]) -> Option<i32> {
  let root_indent = lines.iter().map(|(indent, _)| *indent).min()?;
  for (index, (indent, content)) in lines.iter().enumerate() {
    if *indent != root_indent {
      continue;
    }
    let Some((key, value)) = split_yaml_entry(content) else {
      continue;
    };
    if unquote_yaml_scalar(key) != "concurrency" {
      continue;
    }
    if !value.trim().is_empty() {
      // より深くインデントされた内容行が続けば、値は複数行の通常スカラー
      // であり、この行の文字列だけでは値全体を表さないため受理しない。
      if continues_on_following_line(lines, index + 1, root_indent + 1) {
        return None;
      }
      return parse_concurrency_value(value);
    }
    // 値が同じ行に無い場合は、より深くインデントされた次の行が単独の
    // スカラーであるときだけ、その行を値として扱う。
    let (next_indent, next) = lines.get(index + 1)?;
    if *next_indent > root_indent && is_plain_scalar_line(next) {
      // 値の行と同じかより深いインデントの内容行が続けば、値は複数行の
      // 通常スカラーであり、最初の行だけでは値全体を表さないため受理しない。
      if continues_on_following_line(lines, index + 2, *next_indent) {
        return None;
      }
      return parse_concurrency_value(next);
    }
    return None;
  }
  None
}

/// 1行のフローマッピング（`{ key: value, ... }`）からトップレベルの
/// concurrency を取り出す。値が正の整数でない場合は None を返す。
fn parse_flow_concurrency(line: &str) -> Option<i32> {
  let inner = line.trim().strip_prefix('{')?.strip_suffix('}')?;
  for entry in split_flow_entries(inner) {
    let Some((key, value)) = split_yaml_entry(entry) else {
      continue;
    };
    if unquote_yaml_scalar(key) == "concurrency" {
      return parse_concurrency_value(value);
    }
  }
  None
}

/// concurrency の値を正の整数として検証する。引用符で囲まれた数値も受け付ける。
fn parse_concurrency_value(value: &str) -> Option<i32> {
  let parsed: i32 = unquote_yaml_scalar(value).parse().ok()?;
  (parsed > 0).then_some(parsed)
}

/// YAML の内容行を（インデント幅, インデントを除いた内容）の列にする。
/// 空行とコメントだけの行は落とす。
fn yaml_content_lines(text: &str) -> Vec<(usize, &str)> {
  text
    .lines()
    .filter_map(|line| {
      let line = strip_yaml_comment(line).trim_end();
      let content = line.trim_start();
      if content.is_empty() {
        return None;
      }
      Some((line.len() - content.len(), content))
    })
    .collect()
}

/// 行からコメントを取り除く。引用符の中の # はコメントとみなさない。
/// YAML の規則どおり、# の直前が行頭か空白の場合だけコメントとして扱う。
fn strip_yaml_comment(line: &str) -> &str {
  let mut scan = YamlQuoteScan::new();
  let mut after_boundary = true;
  let mut chars = line.char_indices().peekable();
  while let Some((index, ch)) = chars.next() {
    let next = chars.peek().map(|(_, ch)| *ch);
    if !scan.advance(ch, next) && ch == '#' && after_boundary {
      return &line[..index];
    }
    after_boundary = ch == ' ' || ch == '\t';
  }
  line
}

/// YAML の1行を走査する際の引用状態。引用はスカラーの開始位置にある場合
/// だけ引用として扱い、通常スカラーの途中に現れる引用符（アポストロフィ
/// など）は文字として扱う。二重引用の中のエスケープと、単一引用の中で
/// 重ねた引用符は、引用を閉じるものとして扱わない。
struct YamlQuoteScan {
  quote:           Option<char>,
  escaped:         bool,
  at_scalar_start: bool,
}

impl YamlQuoteScan {
  fn new() -> Self {
    Self { quote: None, escaped: false, at_scalar_start: true }
  }

  /// 文字を1つ読み進める。その文字が引用の内側（引用符そのものと
  /// エスケープされた文字を含む）なら true を返す。呼び出し側は、true の
  /// 文字を区切りやコメントの開始として扱わない。
  fn advance(&mut self, ch: char, next: Option<char>) -> bool {
    if self.escaped {
      self.escaped = false;
      return true;
    }
    if let Some(open) = self.quote {
      if (open == '"' && ch == '\\') || (open == '\'' && ch == '\'' && next == Some('\'')) {
        self.escaped = true;
      } else if ch == open {
        self.quote = None;
        self.at_scalar_start = false;
      }
      return true;
    }
    match ch {
      | '"' | '\'' if self.at_scalar_start => {
        self.quote = Some(ch);
        true
      },
      | ':' | ',' | '{' | '[' => {
        self.at_scalar_start = true;
        false
      },
      | ' ' | '\t' => false,
      | _ => {
        self.at_scalar_start = false;
        false
      },
    }
  }
}

/// 指定した位置の内容行が、基準以上のインデントを持つかどうかを返す。
/// 通常スカラーに継続する内容行があるかの判定に使う。
fn continues_on_following_line(lines: &[(usize, &str)], index: usize, min_indent: usize) -> bool {
  lines.get(index).is_some_and(|(indent, _)| *indent >= min_indent)
}

/// `key: value` の形の行をキーと値へ分ける。キーが引用符で始まる場合は、
/// 閉じ引用符の直後のコロンを区切りとする。コロンが無ければ None を返す。
fn split_yaml_entry(content: &str) -> Option<(&str, &str)> {
  let content = content.trim();
  if let Some(quote) = content.chars().next().filter(|ch| *ch == '"' || *ch == '\'') {
    let rest = &content[quote.len_utf8()..];
    let close = rest.find(quote)?;
    let key = &content[..quote.len_utf8() + close + quote.len_utf8()];
    let value = rest[close + quote.len_utf8()..].trim_start().strip_prefix(':')?;
    return Some((key, value));
  }
  content.split_once(':')
}

/// フローマッピングの中身を、入れ子と引用符を考慮しながらカンマで分ける。
fn split_flow_entries(inner: &str) -> Vec<&str> {
  let mut entries = Vec::new();
  let mut depth = 0usize;
  let mut scan = YamlQuoteScan::new();
  let mut start = 0;
  let mut chars = inner.char_indices().peekable();
  while let Some((index, ch)) = chars.next() {
    let next = chars.peek().map(|(_, ch)| *ch);
    if scan.advance(ch, next) {
      continue;
    }
    match ch {
      | '{' | '[' => depth += 1,
      | '}' | ']' => depth = depth.saturating_sub(1),
      | ',' if depth == 0 => {
        entries.push(&inner[start..index]);
        start = index + ch.len_utf8();
      },
      | _ => {},
    }
  }
  entries.push(&inner[start..]);
  entries
}

/// スカラーの前後の引用符（単一・二重）を、対になっている場合だけ取り除く。
fn unquote_yaml_scalar(value: &str) -> &str {
  let value = value.trim();
  let bytes = value.as_bytes();
  if value.len() >= 2 {
    let first = bytes[0];
    if (first == b'"' || first == b'\'') && bytes[value.len() - 1] == first {
      return &value[1..value.len() - 1];
    }
  }
  value
}

/// ネストしたマッピングやシーケンスではなく、値そのものを表す行かどうか。
fn is_plain_scalar_line(content: &str) -> bool {
  !content.trim_start().starts_with("- ") && split_yaml_entry(content).is_none()
}

/// 設定の本文と、相対パスの解決に使う基準ディレクトリから [`Config`] を組み立てる。
pub fn parse_config(text: &str, base_dir: &Path) -> Result<Config, ConfigError> {
  let raw: RawConfig =
    serde_json::from_str(text).map_err(|source| ConfigError::Parse { path: base_dir.display().to_string(), source })?;
  let mut servers = BTreeMap::new();
  for (id, server) in raw.mcp_servers {
    validate_server_id(&id)?;
    servers.insert(id.clone(), resolve_mcp_server(&id, server)?);
  }
  if servers.is_empty() {
    return Err(ConfigError::Invalid("mcpServers が空です。takt-mcp の起動方法を1つ以上指定してください。".to_owned()));
  }
  let mut server_meta = BTreeMap::new();
  for (id, meta) in raw.servers {
    validate_server_id(&id)?;
    let name = meta.name.ok_or_else(|| ConfigError::Invalid(format!("サーバー {id} の name がありません。")))?;
    let kind = meta.kind.ok_or_else(|| ConfigError::Invalid(format!("サーバー {id} の kind がありません。")))?;
    server_meta.insert(id, ServerMeta { name, kind });
  }
  let mut projects = Vec::new();
  let mut seen_names = std::collections::BTreeSet::new();
  for project in raw.projects {
    validate_server_id(&project.server)?;
    ProjectName::try_from(project.name.as_str())
      .map_err(|error| ConfigError::Invalid(format!("プロジェクト名 {} が不正です: {error}", project.name)))?;
    if !servers.contains_key(&project.server) {
      return Err(ConfigError::Invalid(format!(
        "プロジェクト {} が参照するサーバー {} は mcpServers にありません。",
        project.name, project.server
      )));
    }
    if !seen_names.insert(project.name.clone()) {
      return Err(ConfigError::Invalid(format!(
        "プロジェクト名 {} が重複しています。プロジェクト名は一意である必要があります。",
        project.name
      )));
    }
    let write_server = project.write_server.unwrap_or_else(|| project.server.clone());
    validate_server_id(&write_server)?;
    servers.get(&write_server).ok_or_else(|| {
      ConfigError::Invalid(format!(
        "プロジェクト {} が参照する書き込み用サーバー {} は mcpServers にありません。",
        project.name, write_server
      ))
    })?;
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
      write_server,
      about: project.about.unwrap_or_default(),
      cwd,
      concurrency: DEFAULT_PROJECT_CONCURRENCY,
    });
  }
  let sqlite_raw = raw.sqlite.path.unwrap_or_else(|| DEFAULT_SQLITE_PATH.to_owned());
  let sqlite_path =
    if Path::new(&sqlite_raw).is_absolute() { PathBuf::from(sqlite_raw) } else { base_dir.join(sqlite_raw) };
  let interval_seconds = raw.interval_seconds.unwrap_or(DEFAULT_INTERVAL_SECONDS);
  if interval_seconds == 0 {
    return Err(ConfigError::Invalid("intervalSeconds は1秒以上である必要があります。".to_owned()));
  }
  // 画面読み取りレーンの本数は任意項目です。範囲外の値はエラーにせず、1〜上限へ丸めます。
  let screen_read_lanes = ScreenReadLanes::clamped(raw.screen_read_lanes.unwrap_or(DEFAULT_SCREEN_READ_LANES));
  let pending_decision_stale_minutes =
    raw.thresholds.pending_decision_stale_minutes.unwrap_or(DEFAULT_PENDING_DECISION_STALE_MINUTES);
  let run_log_silent_minutes = raw.thresholds.run_log_silent_minutes.unwrap_or(DEFAULT_RUN_LOG_SILENT_MINUTES);
  let thresholds = Thresholds {
    pending_decision_stale_minutes: StaleMinutes::try_from(pending_decision_stale_minutes)
      .map_err(|error| ConfigError::Invalid(format!("pendingDecisionStaleMinutes が不正です: {error}")))?,
    run_log_silent_minutes:         SilentMinutes::try_from(run_log_silent_minutes)
      .map_err(|error| ConfigError::Invalid(format!("runLogSilentMinutes が不正です: {error}")))?,
    connection_failure_count:       raw.thresholds.connection_failure_count.unwrap_or(DEFAULT_CONNECTION_FAILURE_COUNT),
    connection_delay_seconds:       raw.thresholds.connection_delay_seconds.unwrap_or(DEFAULT_CONNECTION_DELAY_SECONDS),
  };
  if thresholds.connection_failure_count == 0 {
    return Err(ConfigError::Invalid("connectionFailureCount は1回以上である必要があります。".to_owned()));
  }
  if thresholds.connection_delay_seconds == 0 {
    return Err(ConfigError::Invalid("connectionDelaySeconds は1秒以上である必要があります。".to_owned()));
  }
  let listen_raw = raw.listen.unwrap_or_else(|| DEFAULT_LISTEN.to_owned());
  let listen = listen_raw.parse::<SocketAddr>().map_err(|error| {
    ConfigError::Invalid(format!("listen {listen_raw} を待ち受けアドレスとして解釈できませんでした: {error}"))
  })?;
  Ok(Config { servers, server_meta, projects, interval_seconds, screen_read_lanes, thresholds, sqlite_path, listen })
}

/// 1 件の mcpServers のエントリを、stdio 起動方法か HTTP 接続先のどちらかへ解決する。
///
/// `type` が `http` のときは HTTP 接続先として `url` と `Authorization` ヘッダを必須にし、
/// `command` などの起動方法は使わない。`type` がない、または `stdio` のときは起動方法として
/// `command` を必須にします。
fn resolve_mcp_server(id: &str, server: RawMcpServer) -> Result<McpServer, ConfigError> {
  let is_http = matches!(server.kind.as_deref(), Some("http"));
  if is_http {
    let url =
      server.url.ok_or_else(|| ConfigError::Invalid(format!("サーバー {id} の type: http には url が必須です。")))?;
    let endpoint = HttpServerEndpoint::try_from(url.as_str())?;
    let authorization = server.headers.get("Authorization").cloned().ok_or_else(|| {
      ConfigError::Invalid(format!("サーバー {id} の HTTP 接続先には headers.Authorization が必須です。"))
    })?;
    if !authorization.starts_with("Bearer ") || authorization.len() <= "Bearer ".len() {
      return Err(ConfigError::Invalid(format!(
        "サーバー {id} の headers.Authorization は \"Bearer <token>\" である必要があります。"
      )));
    }
    return Ok(McpServer::Http(HttpMcpServer { endpoint, authorization }));
  }
  let command = server.command.ok_or_else(|| {
    ConfigError::Invalid(format!("サーバー {id} の command がありません。stdio の起動方法には command が必須です。"))
  })?;
  Ok(McpServer::Stdio(StdioMcpServer { command, args: server.args, env: server.env }))
}

fn validate_server_id(id: &str) -> Result<(), ConfigError> {
  ServerId::try_from(id)
    .map(|_| ())
    .map_err(|error| ConfigError::Invalid(format!("サーバー ID {id} が不正です: {error}")))
}

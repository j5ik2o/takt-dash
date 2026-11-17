//! takt-mcp 子プロセスの起動・所有・再利用・再起動と JSON-RPC の往復。

use std::{
  collections::BTreeMap,
  path::PathBuf,
  process::Stdio,
  sync::{Arc, OnceLock},
  time::Duration,
};

use serde_json::{json, Value};
use tokio::{
  io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter},
  process::{Child, ChildStdin, ChildStdout, Command},
  sync::Semaphore,
  time::{timeout, Instant},
};

use super::protocol::{Incoming, Notification, Outgoing, Request, Response, PROTOCOL_VERSION};

/// 輸送層の失敗。プロセスの起動失敗・タイムアウト・不正な応答・プロセス死亡を表します。
/// 道具が isError を返した場合は輸送エラーではなく道具エラーとして扱います。
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
  #[error("takt-mcp の子プロセスを起動できませんでした: {0}")]
  Spawn(String),
  #[error("takt-mcp との通信で入出力に失敗しました: {0}")]
  Io(String),
  #[error("takt-mcp から {0} 秒以内に応答がありませんでした")]
  Timeout(u64),
  #[error("takt-mcp の再起動を抑制しています。あと {0} 秒ほど待ってから再試行します")]
  Backoff(u64),
  #[error("takt-mcp の応答が JSON として不正です: {0}")]
  InvalidJson(String),
  #[error("takt-mcp が JSON-RPC のエラーを返しました（コード {code}）: {message}")]
  Rpc { code: i64, message: String },
  #[error("takt-mcp の子プロセスが終了しました")]
  ProcessExited,
}

/// tools/call の結果のうち、道具自体が isError を返した失敗。子プロセスは生存しているため
/// 輸送エラーとは区別します。
#[derive(Debug, thiserror::Error)]
#[error("takt-mcp の道具 {tool} がエラーを返しました: {message}")]
pub struct ToolCallError {
  pub tool:    String,
  pub message: String,
}

/// 道具呼び出しの失敗。輸送エラーと道具エラーを区別します。
#[derive(Debug, thiserror::Error)]
pub enum CallError {
  #[error(transparent)]
  Transport(#[from] TransportError),
  #[error(transparent)]
  Tool(#[from] ToolCallError),
}

/// 子プロセスをどのように起動するか。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpClientConfig {
  pub command:         String,
  pub args:            Vec<String>,
  pub env:             BTreeMap<String, String>,
  /// 子プロセスを起動する作業ディレクトリ。takt-mcp はこの cwd を許可済みの根として扱います。
  pub cwd:             PathBuf,
  /// 1呼び出しあたりのタイムアウト。
  pub request_timeout: Duration,
  /// 道具ごとのタイムアウト。ここに無い道具は `request_timeout` を使います。
  /// 応答が大きく高負荷で遅れやすい読み取り道具だけを個別に長くします。
  pub tool_timeouts:   BTreeMap<String, Duration>,
}

/// 起動と初期化だけに使う期限。道具の呼び出しの期限とは分けます。高負荷のときは
/// 子プロセスの起動と initialize の往復だけで呼び出しの期限（15秒）を超えるため、
/// 呼び出しと同じ期限で初期化を打ち切ると、起動が永久に終わらなくなります。
const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(60);

/// 同時に起動と初期化を行う子プロセスの上限。プロジェクト全件分を同時に起動すると、
/// 起動同士が資源を取り合って初期化が終わらなくなります。
const MAX_CONCURRENT_SPAWNS: usize = 3;

/// 起動に失敗した後の再起動を抑える間隔の起点と上限。連続失敗のたびに倍にし、
/// 上限で止めます。失敗のたびに待ち時間なしで再起動すると、起動と終了の繰り返しが
/// 負荷をさらに押し上げます。
const SPAWN_BACKOFF_BASE: Duration = Duration::from_secs(5);
const SPAWN_BACKOFF_MAX: Duration = Duration::from_secs(300);

/// プロセス全体で起動と初期化の同時実行を数えるセマフォ。クライアントが
/// プロジェクトごとに分かれていても、backend 全体で上限を効かせます。
fn spawn_permits() -> Arc<Semaphore> {
  static PERMITS: OnceLock<Arc<Semaphore>> = OnceLock::new();
  PERMITS.get_or_init(|| Arc::new(Semaphore::new(MAX_CONCURRENT_SPAWNS))).clone()
}

/// 起動の連続失敗の回数から、次の起動を抑える間隔を求めます。
fn spawn_backoff(consecutive_failures: u32) -> Duration {
  let mut delay = SPAWN_BACKOFF_BASE;
  for _ in 1..consecutive_failures.min(16) {
    delay = delay.saturating_mul(2).min(SPAWN_BACKOFF_MAX);
  }
  delay.min(SPAWN_BACKOFF_MAX)
}

/// 1プロジェクト分の takt-mcp 子プロセスを所有するクライアント。
///
/// 子プロセスの寿命はこのクライアントが所有します。輸送エラーが起きたら子プロセスを破棄し、
/// 次の呼び出しで新しい子プロセスを起動して再初期化します。起動と初期化に失敗した場合は、
/// 次の起動を間隔を空けて行います（[`spawn_backoff`]）。
pub struct McpClient {
  config:                     McpClientConfig,
  process:                    Option<McpProcess>,
  next_id:                    u64,
  /// 起動と初期化の連続失敗の回数。再起動を抑える間隔を決めます。
  consecutive_spawn_failures: u32,
  /// この時刻までは子プロセスを起動しません。起動に失敗した間だけ設定します。
  next_spawn_at:              Option<Instant>,
}

struct McpProcess {
  child:  Child,
  stdin:  BufWriter<ChildStdin>,
  stdout: BufReader<ChildStdout>,
}

impl McpClient {
  /// まだ子プロセスを起動しないクライアントを作る。最初の呼び出しで起動します。
  pub fn new(config: McpClientConfig) -> Self {
    McpClient { config, process: None, next_id: 1, consecutive_spawn_failures: 0, next_spawn_at: None }
  }

  /// 渡された順序で、指定した道具を1つずつ呼び出して応答の本文テキストを返す。
  ///
  /// 輸送エラーが起きたら子プロセスを破棄し、次の呼び出しで新しい子プロセスを起動します。
  /// 道具が isError を返した場合は輸送エラーではなく [`CallError::Tool`] として返し、
  /// 子プロセスは破棄しません。
  pub async fn call_tool(&mut self, name: &str, arguments: Value) -> Result<String, CallError> {
    self.ensure_started().await?;
    let id = self.take_id();
    let request = Request::new(id, "tools/call", Some(json!({ "name": name, "arguments": arguments })));
    let result = self.request(request, self.timeout_for(name)).await?;
    extract_tool_text(name, &result).map_err(CallError::from)
  }

  /// 道具の呼び出しに使う期限。道具ごとの設定があればそれを使い、無ければ全体の期限を使います。
  fn timeout_for(&self, tool: &str) -> Duration {
    self.config.tool_timeouts.get(tool).copied().unwrap_or(self.config.request_timeout)
  }

  /// 子プロセスを起動して終了させる。停止指令を受けたときに呼びます。
  pub async fn shutdown(&mut self) {
    if let Some(mut process) = self.process.take() {
      let _ = process.child.kill().await;
    }
  }

  fn take_id(&mut self) -> u64 {
    let id = self.next_id;
    self.next_id += 1;
    id
  }

  async fn ensure_started(&mut self) -> Result<(), TransportError> {
    if self.process.is_some() {
      return Ok(());
    }
    if let Some(next_spawn_at) = self.next_spawn_at {
      let remaining = next_spawn_at.saturating_duration_since(Instant::now());
      if !remaining.is_zero() {
        return Err(TransportError::Backoff(remaining.as_secs().max(1)));
      }
    }
    match self.spawn().await {
      | Ok(process) => {
        self.consecutive_spawn_failures = 0;
        self.next_spawn_at = None;
        self.process = Some(process);
        Ok(())
      },
      | Err(error) => {
        self.consecutive_spawn_failures = self.consecutive_spawn_failures.saturating_add(1);
        self.next_spawn_at = Some(Instant::now() + spawn_backoff(self.consecutive_spawn_failures));
        Err(error)
      },
    }
  }

  async fn spawn(&mut self) -> Result<McpProcess, TransportError> {
    // 起動と初期化の同時実行をプロセス全体で絞ります。許可を待つ時間は初期化の
    // 期限に含めません。許可は初期化が終わるまで保持します。
    let _permit = spawn_permits()
      .acquire_owned()
      .await
      .map_err(|_| TransportError::Spawn("起動の許可を待つ間にセマフォが閉じました".to_owned()))?;
    let mut command = Command::new(&self.config.command);
    command.args(&self.config.args);
    command.current_dir(&self.config.cwd);
    for (key, value) in &self.config.env {
      command.env(key, value);
    }
    command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    command.kill_on_drop(true);
    let mut child = command.spawn().map_err(|error| TransportError::Spawn(error.to_string()))?;
    let stdin =
      child.stdin.take().ok_or_else(|| TransportError::Spawn("子プロセスの標準入力がありません".to_owned()))?;
    let stdout =
      child.stdout.take().ok_or_else(|| TransportError::Spawn("子プロセスの標準出力がありません".to_owned()))?;
    let mut process = McpProcess { child, stdin: BufWriter::new(stdin), stdout: BufReader::new(stdout) };
    let initialize = Request::new(
      self.take_id(),
      "initialize",
      Some(json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": {},
        "clientInfo": { "name": "taktdash-backend", "version": env!("CARGO_PKG_VERSION") },
      })),
    );
    // 初期化の期限は呼び出しの期限と分けます。呼び出しの期限で初期化を打ち切ると、
    // 高負荷のときに起動が終わらないまま子プロセスを捨て続けることになります。
    process.exchange(initialize, INITIALIZE_TIMEOUT.as_secs()).await?;
    let initialized = Notification::new("notifications/initialized", None);
    process.send(&Outgoing::Notification(initialized)).await?;
    Ok(process)
  }

  async fn request(&mut self, request: Request, timeout: Duration) -> Result<Value, TransportError> {
    let timeout_secs = timeout.as_secs();
    let process = match self.process.as_mut() {
      | Some(process) => process,
      | None => return Err(TransportError::ProcessExited),
    };
    match process.exchange(request, timeout_secs).await {
      | Ok(result) => Ok(result),
      | Err(error) => {
        self.process = None;
        Err(error)
      },
    }
  }
}

impl McpProcess {
  async fn send<T: serde::Serialize>(&mut self, message: &T) -> Result<(), TransportError> {
    let mut serialized = serde_json::to_vec(message).map_err(|error| TransportError::Io(error.to_string()))?;
    serialized.push(b'\n');
    self.stdin.write_all(&serialized).await.map_err(|error| TransportError::Io(error.to_string()))?;
    self.stdin.flush().await.map_err(|error| TransportError::Io(error.to_string()))
  }

  async fn exchange(&mut self, request: Request, timeout_secs: u64) -> Result<Value, TransportError> {
    // 呼び出し全体（送信と、応答が揃うまでの読み取りの往復）に同じ期限を適用します。
    // 途中で通知やサーバー要求が来ても期限は延びません。
    let deadline = Duration::from_secs(timeout_secs.max(1));
    match timeout(deadline, self.exchange_until_reply(request)).await {
      | Ok(result) => result,
      | Err(_) => Err(TransportError::Timeout(timeout_secs)),
    }
  }

  async fn exchange_until_reply(&mut self, request: Request) -> Result<Value, TransportError> {
    let id = request.id;
    self.send(&Outgoing::Request(request)).await?;
    loop {
      let line = self.read_line().await?;
      let incoming: Incoming =
        serde_json::from_str(&line).map_err(|error| TransportError::InvalidJson(format!("{error}: {line}")))?;
      if incoming.is_server_request() {
        let response = Response::empty_result(incoming.id.clone().unwrap_or(Value::Null));
        self.send(&Outgoing::Response(response)).await?;
        continue;
      }
      if incoming.is_notification() {
        continue;
      }
      if let Some(error) = incoming.error {
        return Err(TransportError::Rpc { code: error.code, message: error.message });
      }
      if incoming.id.as_ref().and_then(Value::as_u64) == Some(id) {
        return Ok(incoming.result.unwrap_or(Value::Null));
      }
    }
  }

  async fn read_line(&mut self) -> Result<String, TransportError> {
    let mut line = String::new();
    let read = self.stdout.read_line(&mut line).await.map_err(|error| TransportError::Io(error.to_string()))?;
    if read == 0 {
      return Err(TransportError::ProcessExited);
    }
    while line.ends_with('\n') || line.ends_with('\r') {
      line.pop();
    }
    Ok(line)
  }
}

/// tools/call の result から本文テキストを取り出す。isError
/// は輸送エラーではなく道具エラーとして返す。
fn extract_tool_text(tool: &str, result: &Value) -> Result<String, ToolCallError> {
  if result.get("isError").and_then(Value::as_bool) == Some(true) {
    let text = result
      .get("content")
      .and_then(Value::as_array)
      .and_then(|items| items.first())
      .and_then(|item| item.get("text"))
      .and_then(Value::as_str)
      .unwrap_or("takt-mcp の道具がエラーを返しました");
    return Err(ToolCallError { tool: tool.to_owned(), message: text.to_owned() });
  }
  let text = result
    .get("content")
    .and_then(Value::as_array)
    .and_then(|items| items.first())
    .and_then(|item| item.get("text"))
    .and_then(Value::as_str)
    .ok_or_else(|| ToolCallError {
      tool:    tool.to_owned(),
      message: "tools/call の結果に content[0].text がありません".to_owned(),
    })?;
  Ok(text.to_owned())
}

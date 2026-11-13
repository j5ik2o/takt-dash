//! takt-mcp 子プロセスの起動・所有・再利用・再起動と JSON-RPC の往復。

use std::{collections::BTreeMap, path::PathBuf, process::Stdio, time::Duration};

use serde_json::{json, Value};
use tokio::{
  io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter},
  process::{Child, ChildStdin, ChildStdout, Command},
  time::timeout,
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
}

/// 1プロジェクト分の takt-mcp 子プロセスを所有するクライアント。
///
/// 子プロセスの寿命はこのクライアントが所有します。輸送エラーが起きたら子プロセスを破棄し、
/// 次の呼び出しで新しい子プロセスを起動して再初期化します。
pub struct McpClient {
  config:  McpClientConfig,
  process: Option<McpProcess>,
  next_id: u64,
}

struct McpProcess {
  child:  Child,
  stdin:  BufWriter<ChildStdin>,
  stdout: BufReader<ChildStdout>,
}

impl McpClient {
  /// まだ子プロセスを起動しないクライアントを作る。最初の呼び出しで起動します。
  pub fn new(config: McpClientConfig) -> Self {
    McpClient { config, process: None, next_id: 1 }
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
    let result = self.request(request).await?;
    extract_tool_text(name, &result).map_err(CallError::from)
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
    if self.process.is_none() {
      let process = self.spawn().await?;
      self.process = Some(process);
    }
    Ok(())
  }

  async fn spawn(&mut self) -> Result<McpProcess, TransportError> {
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
    let timeout_secs = self.config.request_timeout.as_secs();
    let initialize = Request::new(
      self.take_id(),
      "initialize",
      Some(json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": {},
        "clientInfo": { "name": "taktdash-backend", "version": env!("CARGO_PKG_VERSION") },
      })),
    );
    process.exchange(initialize, timeout_secs).await?;
    let initialized = Notification::new("notifications/initialized", None);
    process.send(&Outgoing::Notification(initialized)).await?;
    Ok(process)
  }

  async fn request(&mut self, request: Request) -> Result<Value, TransportError> {
    let timeout_secs = self.config.request_timeout.as_secs();
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
    let id = request.id;
    self.send(&Outgoing::Request(request)).await?;
    let deadline = Duration::from_secs(timeout_secs.max(1));
    loop {
      let line = match timeout(deadline, self.read_line()).await {
        | Ok(result) => result?,
        | Err(_) => return Err(TransportError::Timeout(timeout_secs)),
      };
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

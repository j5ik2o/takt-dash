//! 常駐する takt-mcp HTTP サーバーとの JSON-RPC（MCP）通信。
//!
//! 読み取り経路と書き込み経路が、home ごとに常駐する takt-mcp の HTTP サーバーへ
//! `tools/call` を送ります。子プロセスは起動せず、`initialize` で作った 1 つのセッションを
//! 使い回します。未知セッション（サーバー再起動などで消えた場合の 404）は、要求が処理されて
//! いないことが分かるため、セッションを作り直して 1 回だけ再試行します。読み取りでは、
//! 接続の失敗（I/O エラー）でも同じ再試行をします。書き込みでは、サーバーが書き込みを
//! 適用した後で接続が切れた場合と区別できないため、I/O エラーでは再試行せず、輸送エラー
//! として表面化させます。それ以外の失敗も輸送エラーとして表面化させ、空の結果へ
//! 縮退しません。

use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{
  body::Incoming,
  header::{HeaderName, HeaderValue},
  Request, StatusCode,
};
use hyper_util::{
  client::legacy::{connect::HttpConnector, Client},
  rt::TokioExecutor,
};
use serde_json::{json, Value};
use tokio::time::timeout;

use super::{
  client::{extract_tool_text, CallError, ToolCaller, TransportError},
  protocol::{Incoming as JsonRpcIncoming, PROTOCOL_VERSION},
};
use crate::config::HttpMcpServer;

/// 1 台の常駐 HTTP サーバーへ読み取りの `tools/call` を送るハンドル。
///
/// `initialize` で作ったセッション ID を保持し、以降の要求へ `Mcp-Session-Id` として付けます。
/// 未知セッション（404）は、セッションが消えたとみなして作り直し、同じ要求を 1 回だけ
/// 再試行します。ハンドルは自分が保持するセッションを高々 1 つに保ちます。
pub struct HttpReadHandle {
  server:                   HttpMcpServer,
  request_timeout:          Duration,
  session_id:               Option<String>,
  next_id:                  u64,
  client:                   Client<HttpConnector, Full<Bytes>>,
  /// 接続の失敗（I/O エラー）でセッションが消えた場合に再試行するか。読み取りは再試行し、
  /// 書き込みは再試行しません（適用済みの書き込みを重複送信しないため）。
  retry_on_io_session_loss: bool,
}

impl HttpReadHandle {
  /// 接続先と 1 呼び出しのタイムアウトからハンドルを作る。まだセッションは作りません。
  ///
  /// HTTP 接続先だけを受け付けます。stdio の起動方法を渡した場合は、常駐サーバーへ読み取りを
  /// 向ける前提が崩れているため、パニックではなく MCP の契約違反として扱えるようエラーにします。
  pub fn new(connection: crate::config::McpConnection, request_timeout: Duration) -> Self {
    Self::create(connection, request_timeout, true)
  }

  /// 書き込み用のハンドルを作る。接続の失敗（I/O エラー）では再試行しません。サーバーが
  /// 書き込みを適用した後で接続が切れた場合と区別できず、再試行すると書き込みを重複して
  /// 送信するためです。未知セッション（404）の再試行は、要求が処理されていないことが
  /// 分かるため、書き込みでも行います。
  pub fn new_for_write(connection: crate::config::McpConnection, request_timeout: Duration) -> Self {
    Self::create(connection, request_timeout, false)
  }

  fn create(
    connection: crate::config::McpConnection,
    request_timeout: Duration,
    retry_on_io_session_loss: bool,
  ) -> Self {
    let server = match connection {
      | crate::config::McpConnection::Http(server) => server,
      | crate::config::McpConnection::Stdio(_) => {
        panic!("HttpReadHandle は HTTP 接続先だけを受け付けます。stdio 接続先が渡されました。")
      },
    };
    HttpReadHandle {
      server,
      request_timeout,
      session_id: None,
      next_id: 1,
      client: Client::builder(TokioExecutor::new()).build(HttpConnector::new()),
      retry_on_io_session_loss,
    }
  }

  /// このハンドルが現在保持しているセッション数（0 または 1）。
  pub fn session_count(&self) -> usize {
    usize::from(self.session_id.is_some())
  }

  /// 接続先の URL。テストから接続状態の観測に使います。
  pub fn url(&self) -> String {
    self.server.endpoint.url()
  }

  fn take_id(&mut self) -> u64 {
    let id = self.next_id;
    self.next_id += 1;
    id
  }

  /// セッションを確立する。確立済みなら何もしません。
  ///
  /// 書き込みの前に呼ぶと、道具を送る前の失敗と、送ったあとの失敗を呼び出し側が区別できます。
  pub async fn connect(&mut self) -> Result<(), TransportError> {
    self.ensure_session().await.map(|_| ())
  }

  async fn ensure_session(&mut self) -> Result<String, TransportError> {
    if let Some(session) = &self.session_id {
      return Ok(session.clone());
    }
    let id = self.take_id();
    let initialize = json!({
      "jsonrpc": "2.0",
      "id": id,
      "method": "initialize",
      "params": {
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": {},
        "clientInfo": { "name": "taktdash-backend", "version": env!("CARGO_PKG_VERSION") },
      },
    });
    let (header, incoming) = self.exchange(initialize, None).await.map_err(ExchangeError::into_transport)?;
    if let Some(error) = incoming.error {
      return Err(TransportError::Rpc { code: error.code, message: error.message });
    }
    let session = header
      .ok_or_else(|| TransportError::Protocol("initialize の応答に Mcp-Session-Id ヘッダがありません".to_owned()))?;
    let initialized = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
    self.notify(initialized, &session).await?;
    self.session_id = Some(session.clone());
    Ok(session)
  }

  /// 1 回の tools/call を送る。未知セッション（404）は、要求が処理されていないことが
  /// 分かるため、セッションを作り直して 1 回再試行します。接続の失敗（I/O エラー）も
  /// セッションの消失として再試行しますが、書き込み用のハンドルでは再試行しません
  /// （`retry_on_io_session_loss`）。サーバー再起動では保持していた接続とセッションが
  /// 同時に失効するためです。それ以外の失敗は輸送エラーとして表面化させます。
  async fn tools_call(&mut self, name: &str, arguments: Value) -> Result<Value, TransportError> {
    let session = self.ensure_session().await?;
    let id = self.take_id();
    let request =
      json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": { "name": name, "arguments": arguments } });
    match self.exchange(request.clone(), Some(&session)).await {
      | Ok((_, incoming)) => self.unwrap_result(name, incoming),
      | Err(error) if error.session_lost() && (self.retry_on_io_session_loss || error.unknown_session()) => {
        // セッションが消えた（サーバー再起動など）。セッションを作り直して 1 回だけ再試行します。
        self.session_id = None;
        let session = self.ensure_session().await?;
        let id = self.take_id();
        let retry = json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": { "name": name, "arguments": arguments } });
        match self.exchange(retry, Some(&session)).await {
          | Ok((_, incoming)) => self.unwrap_result(name, incoming),
          | Err(error) => Err(error.into_transport()),
        }
      },
      | Err(error) => Err(error.into_transport()),
    }
  }

  fn unwrap_result(&self, _name: &str, incoming: JsonRpcIncoming) -> Result<Value, TransportError> {
    if let Some(error) = incoming.error {
      return Err(TransportError::Rpc { code: error.code, message: error.message });
    }
    Ok(incoming.result.unwrap_or(Value::Null))
  }

  /// 通知（応答本文を持たない）を送る。既知セッションの通知はサーバーが 202 を返します。
  async fn notify(&self, message: Value, session: &str) -> Result<(), TransportError> {
    match self.exchange(message, Some(session)).await {
      | Ok(_) => Ok(()),
      | Err(error) => Err(error.into_transport()),
    }
  }

  async fn exchange(
    &self,
    message: Value,
    session: Option<&str>,
  ) -> Result<(Option<String>, JsonRpcIncoming), ExchangeError> {
    let body = serde_json::to_vec(&message)
      .map_err(|error| ExchangeError::Transport(TransportError::InvalidJson(error.to_string())))?;
    let uri = hyper::Uri::try_from(self.server.endpoint.url()).map_err(|error| {
      ExchangeError::Transport(TransportError::Protocol(format!("HTTP 接続先の URL を解釈できませんでした: {error}")))
    })?;
    let mut builder = Request::builder().method("POST").uri(uri).header("content-type", "application/json");
    let authorization = HeaderValue::from_str(&self.server.authorization).map_err(|error| {
      ExchangeError::Transport(TransportError::Protocol(format!("Authorization ヘッダを作れませんでした: {error}")))
    })?;
    builder = builder.header("authorization", authorization);
    // MCP の Streamable HTTP では、応答が JSON と SSE のどちらでも返せることを
    // Accept で申告しなければならない。実 takt-mcp は申告が無い要求を 406 で拒否する。
    builder = builder.header("accept", "application/json, text/event-stream");
    if let Some(session) = session {
      let value = HeaderValue::from_str(session).map_err(|error| {
        ExchangeError::Transport(TransportError::Protocol(format!("Mcp-Session-Id を作れませんでした: {error}")))
      })?;
      builder = builder.header(HeaderName::from_static("mcp-session-id"), value);
      // 初期化で合意した版（2025-06-18）では、初期化後の要求に MCP-Protocol-Version が
      // 必須です。付けないと、サーバーは要求を旧版として解釈することがあります。
      builder =
        builder.header(HeaderName::from_static("mcp-protocol-version"), HeaderValue::from_static(PROTOCOL_VERSION));
    }
    let request = builder
      .body(Full::new(Bytes::from(body)))
      .map_err(|error| ExchangeError::Transport(TransportError::Protocol(error.to_string())))?;
    let response = timeout(self.request_timeout, self.client.request(request))
      .await
      .map_err(|_| ExchangeError::Transport(TransportError::Timeout(self.request_timeout.as_secs())))?
      .map_err(|error| ExchangeError::Transport(TransportError::Io(error.to_string())))?;
    let status = response.status();
    let content_type = response.headers().get("content-type").and_then(|value| value.to_str().ok()).map(str::to_owned);
    let session_id = response.headers().get("mcp-session-id").and_then(|value| value.to_str().ok()).map(str::to_owned);
    let collected = timeout(self.request_timeout, read_body(response.into_body()))
      .await
      .map_err(|_| ExchangeError::Transport(TransportError::Timeout(self.request_timeout.as_secs())))??;
    if !status.is_success() && status != StatusCode::ACCEPTED && status != StatusCode::NO_CONTENT {
      return Err(ExchangeError::Status(status));
    }
    if collected.is_empty() {
      // 通知の受け付けなど、応答本文を持たない応答。
      return Ok((session_id, JsonRpcIncoming { id: None, result: Some(Value::Null), error: None, method: None }));
    }
    let incoming = parse_incoming(&collected, content_type.as_deref())?;
    Ok((session_id, incoming))
  }
}

/// 応答本文を JSON-RPC メッセージとして読む。
///
/// 通常の応答は `application/json` の本文そのものである。サーバーが
/// `text/event-stream` で返す場合は、ストリーム内のイベントのうち、結果か
/// エラーを持つ最後のメッセージを応答として採用する。通知だけのストリームは
/// 応答ではないため、失敗として表面化させる。
fn parse_incoming(body: &[u8], content_type: Option<&str>) -> Result<JsonRpcIncoming, ExchangeError> {
  if content_type.is_some_and(|value| value.contains("text/event-stream")) {
    let text = String::from_utf8_lossy(body);
    // SSE では1つのイベントの JSON を複数の `data:` 行へ分けることがあるため、
    // 空行で区切られたイベントごとに `data:` 行を改行で連結してから解釈します。
    let mut events: Vec<String> = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    for line in text.lines() {
      if line.trim().is_empty() {
        if !current.is_empty() {
          events.push(current.join("\n"));
          current.clear();
        }
        continue;
      }
      if let Some(data) = line.trim_start().strip_prefix("data:") {
        current.push(data.strip_prefix(' ').unwrap_or(data));
      }
    }
    if !current.is_empty() {
      events.push(current.join("\n"));
    }
    let mut response: Option<JsonRpcIncoming> = None;
    for data in &events {
      let Ok(incoming) = serde_json::from_str::<JsonRpcIncoming>(data) else {
        continue;
      };
      if incoming.result.is_some() || incoming.error.is_some() {
        response = Some(incoming);
      }
    }
    return response.ok_or_else(|| {
      ExchangeError::Transport(TransportError::InvalidJson(format!(
        "text/event-stream の応答に結果を持つメッセージがありません: {}",
        text.chars().take(200).collect::<String>()
      )))
    });
  }
  serde_json::from_slice(body).map_err(|error| {
    ExchangeError::Transport(TransportError::InvalidJson(format!("{error}: {}", String::from_utf8_lossy(body))))
  })
}

impl HttpReadHandle {
  /// 対象プロジェクトの cwd から読み取りの snapshot を組み立てる。
  ///
  /// 道具のまとまりは呼び出し先を [`ToolCaller`] として抽象化しているため、ハンドル自身を
  /// 呼び出し先として読み取り経路を共有します。
  pub async fn read_snapshot(
    &mut self,
    cwd: &std::path::Path,
    prev: Option<&crate::model::ProjectSnapshot>,
    run_logs: &crate::run_progress::RunLogHandle,
  ) -> Result<crate::model::ProjectSnapshot, crate::mcp::ToolError> {
    crate::mcp::ReadToolSet::new(self, cwd.to_path_buf()).read_snapshot(prev, run_logs).await
  }

  /// 対象プロジェクトの cwd から run の状態を読む。
  pub async fn read_remote_run(
    &mut self,
    cwd: &std::path::Path,
    run_slug: &crate::domain::RunSlug,
  ) -> Result<crate::mcp::RemoteRun, crate::mcp::ToolError> {
    crate::mcp::ReadToolSet::new(self, cwd.to_path_buf()).read_remote_run(run_slug).await
  }

  /// 詳細用に、指示書の一覧とタスク本文まで読む。
  pub async fn read_remote_run_detail(
    &mut self,
    cwd: &std::path::Path,
    run_slug: &crate::domain::RunSlug,
  ) -> Result<crate::mcp::RemoteRun, crate::mcp::ToolError> {
    let mut toolset = crate::mcp::ReadToolSet::new(self, cwd.to_path_buf());
    let mut remote = toolset.read_remote_run(run_slug).await?;
    remote.instructions = toolset.read_remote_instructions(run_slug).await?;
    if remote.task_text.is_empty() {
      remote.task_text = toolset.read_remote_task_text(run_slug).await?;
    }
    Ok(remote)
  }

  /// takt_get_run の view instructions から指示書の一覧を読む。
  pub async fn read_remote_instructions(
    &mut self,
    cwd: &std::path::Path,
    run_slug: &crate::domain::RunSlug,
  ) -> Result<Vec<crate::run_log::Instruction>, crate::mcp::ToolError> {
    crate::mcp::ReadToolSet::new(self, cwd.to_path_buf()).read_remote_instructions(run_slug).await
  }

  /// takt_get_run の view task からタスク本文を読む。
  pub async fn read_remote_task_text(
    &mut self,
    cwd: &std::path::Path,
    run_slug: &crate::domain::RunSlug,
  ) -> Result<String, crate::mcp::ToolError> {
    crate::mcp::ReadToolSet::new(self, cwd.to_path_buf()).read_remote_task_text(run_slug).await
  }

  /// takt_get_run の view report からレポートの本文を読む。
  pub async fn read_remote_report(
    &mut self,
    cwd: &std::path::Path,
    run_slug: &crate::domain::RunSlug,
    filename: &crate::domain::ReportFilename,
    max_bytes: u64,
  ) -> Result<crate::run_log::RunContent, crate::mcp::ToolError> {
    crate::mcp::ReadToolSet::new(self, cwd.to_path_buf()).read_remote_report(run_slug, filename, max_bytes).await
  }

  /// takt_get_run の view instructions から指示書の本文を読む。
  pub async fn read_remote_instruction(
    &mut self,
    cwd: &std::path::Path,
    run_slug: &crate::domain::RunSlug,
    instruction_id: crate::domain::InstructionId,
    max_bytes: u64,
  ) -> Result<crate::run_log::RunContent, crate::mcp::ToolError> {
    crate::mcp::ReadToolSet::new(self, cwd.to_path_buf())
      .read_remote_instruction(run_slug, instruction_id, max_bytes)
      .await
  }

  /// 対象プロジェクトの cwd からゴールの詳細を読む。
  pub async fn read_goal_detail(
    &mut self,
    cwd: &std::path::Path,
    goal_id: &crate::domain::GoalId,
  ) -> Result<crate::model::GoalDetail, crate::mcp::ToolError> {
    crate::mcp::ReadToolSet::new(self, cwd.to_path_buf()).read_goal_detail(goal_id).await
  }

  /// 対象プロジェクトの cwd からタスクの詳細を読む。
  pub async fn read_task_detail(
    &mut self,
    cwd: &std::path::Path,
    project: &crate::domain::ProjectName,
    task_name: &crate::domain::TaskName,
    goal_id: Option<&crate::domain::GoalId>,
  ) -> Result<Option<crate::model::TaskDetail>, crate::mcp::ToolError> {
    crate::mcp::ReadToolSet::new(self, cwd.to_path_buf()).read_task_detail(project, task_name, goal_id).await
  }
}

async fn read_body(body: Incoming) -> Result<Vec<u8>, ExchangeError> {
  let collected =
    body.collect().await.map_err(|error| ExchangeError::Transport(TransportError::Io(error.to_string())))?;
  Ok(collected.to_bytes().to_vec())
}

impl ToolCaller for HttpReadHandle {
  fn connection_key(&self) -> String {
    format!("http:{:?}", self.server)
  }

  fn call_tool<'a>(
    &'a mut self,
    name: &'a str,
    arguments: Value,
  ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, CallError>> + Send + 'a>> {
    Box::pin(async move {
      let result = self.tools_call(name, arguments).await?;
      extract_tool_text(name, &result).map_err(CallError::from)
    })
  }
}

/// HTTP 往復の失敗。未知セッション（404）を再試行のために区別します。
enum ExchangeError {
  Status(StatusCode),
  Transport(TransportError),
}

impl ExchangeError {
  /// この失敗が、既存セッションの失効（サーバー再起動での接続断・未知セッション）を表すか。
  fn session_lost(&self) -> bool {
    match self {
      | ExchangeError::Status(status) => *status == StatusCode::NOT_FOUND,
      | ExchangeError::Transport(TransportError::Io(_)) => true,
      | ExchangeError::Transport(_) => false,
    }
  }

  /// 未知セッション（404）かどうか。この応答を受けた要求はサーバーに処理されていない
  /// ことが分かるため、再試行しても操作を重複して実行しません。
  fn unknown_session(&self) -> bool {
    matches!(self, ExchangeError::Status(status) if *status == StatusCode::NOT_FOUND)
  }

  fn into_transport(self) -> TransportError {
    match self {
      | ExchangeError::Status(status) => {
        TransportError::Http(format!("takt-mcp の HTTP サーバーがステータス {status} を返しました"))
      },
      | ExchangeError::Transport(error) => error,
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn parses_a_plain_json_response() {
    let incoming = match parse_incoming(br#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#, Some("application/json")) {
      | Ok(incoming) => incoming,
      | Err(_) => panic!("JSON 応答を読めます"),
    };
    assert_eq!(incoming.result, Some(json!({ "ok": true })));
  }

  #[test]
  fn parses_the_response_message_out_of_an_event_stream() {
    let body = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\",\"params\":{}}\n\nevent: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"ok\":true}}\n\n";
    let incoming = match parse_incoming(body.as_bytes(), Some("text/event-stream")) {
      | Ok(incoming) => incoming,
      | Err(_) => panic!("SSE 応答を読めます"),
    };
    assert_eq!(incoming.result, Some(json!({ "ok": true })));
  }

  #[test]
  fn parses_an_event_stream_message_split_across_data_lines() {
    let body = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":2,\ndata: \"result\":{\"ok\":true}}\n\n";
    let incoming = match parse_incoming(body.as_bytes(), Some("text/event-stream")) {
      | Ok(incoming) => incoming,
      | Err(_) => panic!("複数行に分かれた data を連結して SSE 応答を読めます"),
    };
    assert_eq!(incoming.result, Some(json!({ "ok": true })));
  }

  #[test]
  fn an_event_stream_without_a_response_message_is_an_error() {
    let body = "data: {\"jsonrpc\":\"2.0\",\"method\":\"ping\"}\n\n";
    assert!(parse_incoming(body.as_bytes(), Some("text/event-stream")).is_err(), "応答が無い SSE は失敗です");
  }
}

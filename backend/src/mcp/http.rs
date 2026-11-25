//! 常駐する takt-mcp HTTP サーバーとの JSON-RPC（MCP）通信。
//!
//! 読み取り経路と書き込み経路が、home ごとに常駐する takt-mcp の HTTP サーバーへ
//! `tools/call` を送ります。子プロセスは起動せず、`initialize` で作った 1 つのセッション
//! （[`HttpSession`]）を使い回し、使い終えたら DELETE で閉じます。takt-mcp はセッションに
//! 期限を持たないため、要求ごとに作って閉じないと、サーバー側にたまり続けます。
//! 未知セッション（サーバー再起動などで消えた場合の 404）は、要求が処理されて
//! いないことが分かるため、セッションを作り直して 1 回だけ再試行します。読み取りでは、
//! 接続の失敗（I/O エラー）のとき、同じセッションで 1 回送り直します。書き込みでは、
//! サーバーが書き込みを適用した後で接続が切れた場合と区別できないため、I/O エラーでは
//! 再試行せず、輸送エラーとして表面化させます。それ以外の失敗も輸送エラーとして
//! 表面化させ、空の結果へ縮退しません。

use std::{
  sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
  },
  time::Duration,
};

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
use tokio::{sync::Mutex as AsyncMutex, time::timeout};

use super::{
  client::{extract_tool_text, CallError, ToolCaller, TransportError},
  protocol::{Incoming as JsonRpcIncoming, PROTOCOL_VERSION},
};
use crate::config::HttpMcpServer;

/// セッションを閉じる DELETE を待つ上限。閉じる処理は後始末であり、応答しないサーバーに
/// 引きずられないよう、道具の呼び出しより短く打ち切ります。
const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);

type HttpClient = Client<HttpConnector, Full<Bytes>>;

/// 1 台の常駐 HTTP サーバーに作る 1 つのセッション。
///
/// takt-mcp の HTTP サーバーはセッションに期限を持たず、DELETE を受けるか再起動するまで
/// 持ち続けます。そのため、セッションは要求ごとに作らず、この値を複製して複数のハンドルで
/// 共有します。最後の持ち主が消えたとき、または [`HttpSession::close`] で、サーバーへ
/// DELETE を送ってセッションを閉じます。
#[derive(Clone)]
pub struct HttpSession {
  inner: Arc<SessionInner>,
}

struct SessionInner {
  server:       HttpMcpServer,
  client:       HttpClient,
  /// サーバーが採番したセッション ID。未確立なら `None`。ガードは `.await` を跨ぎません。
  session_id:   Mutex<Option<String>>,
  /// セッションの確立を直列化する鍵。同時に届いた要求がセッションを 1 つずつ作らないよう、
  /// initialize から ID の保存までの間だけ保持します。
  establishing: AsyncMutex<()>,
  next_id:      AtomicU64,
}

impl HttpSession {
  /// 接続先からセッションの置き場を作る。まだセッションは作りません。
  ///
  /// HTTP 接続先だけを受け付けます。stdio の起動方法を渡した場合は、常駐サーバーへ要求を
  /// 向ける前提が崩れているため、パニックします。
  pub fn new(connection: crate::config::McpConnection) -> Self {
    match connection {
      | crate::config::McpConnection::Http(server) => Self::for_server(server),
      | crate::config::McpConnection::Stdio(_) => {
        panic!("HttpSession は HTTP 接続先だけを受け付けます。stdio 接続先が渡されました。")
      },
    }
  }

  /// HTTP 接続先からセッションの置き場を作る。まだセッションは作りません。
  pub fn for_server(server: HttpMcpServer) -> Self {
    // 接続は要求ごとに張り、使い終えた接続を持ち越しません。サーバーが待機中の接続を
    // 閉じるのと同時に要求を送ると、要求が届いたか分からない接続の失敗になり、書き込みでは
    // 再試行できないためです。接続先はループバックで、張り直しの費用は小さく済みます。
    let client = Client::builder(TokioExecutor::new()).pool_max_idle_per_host(0).build(HttpConnector::new());
    HttpSession {
      inner: Arc::new(SessionInner {
        server,
        client,
        session_id: Mutex::new(None),
        establishing: AsyncMutex::new(()),
        next_id: AtomicU64::new(1),
      }),
    }
  }

  /// 保持しているセッションをサーバーへの DELETE で閉じ、応答（または打ち切り）まで待つ。
  ///
  /// backend の停止時に使います。閉じたあとに要求が届けば、セッションを作り直します。
  pub async fn close(&self) {
    if let Some(closing) = self.inner.take_and_close() {
      closing.await;
    }
  }
}

impl SessionInner {
  fn current(&self) -> Option<String> {
    self.session_id.lock().expect("HTTP セッションのロックが壊れています").clone()
  }

  /// 保持しているセッションが `lost` のままなら手放す。ほかの要求が先に作り直した
  /// セッションは捨てません。
  fn forget(&self, lost: &str) {
    let mut current = self.session_id.lock().expect("HTTP セッションのロックが壊れています");
    if current.as_deref() == Some(lost) {
      *current = None;
    }
  }

  fn take_id(&self) -> u64 {
    self.next_id.fetch_add(1, Ordering::Relaxed)
  }

  /// セッションを確立して保存する。確立済みなら、そのセッションを返します。
  async fn establish(&self, request_timeout: Duration) -> Result<String, TransportError> {
    let _establishing = timeout(request_timeout, self.establishing.lock())
      .await
      .map_err(|_| TransportError::Timeout(request_timeout.as_secs()))?;
    if let Some(session) = self.current() {
      return Ok(session);
    }
    let initialize = json!({
      "jsonrpc": "2.0",
      "id": self.take_id(),
      "method": "initialize",
      "params": {
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": {},
        "clientInfo": { "name": "taktdash-backend", "version": env!("CARGO_PKG_VERSION") },
      },
    });
    let (header, incoming) =
      self.exchange(initialize, None, request_timeout).await.map_err(ExchangeError::into_transport)?;
    let session = header;
    if let Some(error) = incoming.error {
      self.close_unsaved(session);
      return Err(TransportError::Rpc { code: error.code, message: error.message });
    }
    let session = session
      .ok_or_else(|| TransportError::Protocol("initialize の応答に Mcp-Session-Id ヘッダがありません".to_owned()))?;
    let initialized = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
    if let Err(error) = self.exchange(initialized, Some(&session), request_timeout).await {
      // サーバーはセッションを作り終えています。保存しないセッションを残さないよう閉じます。
      self.close_unsaved(Some(session));
      return Err(error.into_transport());
    }
    *self.session_id.lock().expect("HTTP セッションのロックが壊れています") = Some(session.clone());
    Ok(session)
  }

  /// 確立の途中で失敗し、保存しなかったセッションを背後で閉じる。
  fn close_unsaved(&self, session: Option<String>) {
    if let Some(closing) = session.and_then(|session| self.closing(&session)) {
      spawn_close(closing);
    }
  }

  /// 保持しているセッションを手放し、閉じる処理を返す。セッションが無ければ `None`。
  fn take_and_close(&self) -> Option<impl std::future::Future<Output = ()> + Send + 'static> {
    let session = self.session_id.lock().expect("HTTP セッションのロックが壊れています").take()?;
    self.closing(&session)
  }

  /// `session` へ DELETE を送る処理を作る。失敗は呼び出し側へ返しません。サーバーが
  /// 止まっていればセッションも消えており、閉じる必要が無いためです。
  fn closing(&self, session: &str) -> Option<impl std::future::Future<Output = ()> + Send + 'static> {
    let request = match self.request("DELETE", Some(session), None) {
      | Ok(request) => request,
      | Err(error) => {
        tracing::debug!(%error, "takt-mcp の HTTP セッションを閉じる要求を作れませんでした。");
        return None;
      },
    };
    let client = self.client.clone();
    Some(async move {
      let sent = timeout(CLOSE_TIMEOUT, async {
        let response = client.request(request).await.map_err(|error| error.to_string())?;
        let status = response.status();
        // 応答本文を読み切り、接続を最後まで使い終えます。
        let _ = response.into_body().collect().await;
        Ok::<StatusCode, String>(status)
      })
      .await;
      match sent {
        | Ok(Ok(status)) if status.is_success() || status == StatusCode::NOT_FOUND => {},
        | Ok(Ok(status)) => tracing::debug!(%status, "takt-mcp の HTTP セッションを閉じられませんでした。"),
        | Ok(Err(error)) => tracing::debug!(%error, "takt-mcp の HTTP セッションを閉じる要求が届きませんでした。"),
        | Err(_) => tracing::debug!("takt-mcp の HTTP セッションを閉じる要求が時間切れになりました。"),
      }
    })
  }

  /// 1 つの HTTP 要求を組み立てる。`body` が無い要求（DELETE）は本文を持ちません。
  fn request(
    &self,
    method: &str,
    session: Option<&str>,
    body: Option<Vec<u8>>,
  ) -> Result<Request<Full<Bytes>>, TransportError> {
    let uri = hyper::Uri::try_from(self.server.endpoint.url())
      .map_err(|error| TransportError::Protocol(format!("HTTP 接続先の URL を解釈できませんでした: {error}")))?;
    let mut builder = Request::builder().method(method).uri(uri);
    if body.is_some() {
      builder = builder.header("content-type", "application/json");
    }
    let authorization = HeaderValue::from_str(&self.server.authorization)
      .map_err(|error| TransportError::Protocol(format!("Authorization ヘッダを作れませんでした: {error}")))?;
    builder = builder.header("authorization", authorization);
    // MCP の Streamable HTTP では、応答が JSON と SSE のどちらでも返せることを
    // Accept で申告しなければならない。実 takt-mcp は申告が無い要求を 406 で拒否する。
    builder = builder.header("accept", "application/json, text/event-stream");
    if let Some(session) = session {
      let value = HeaderValue::from_str(session)
        .map_err(|error| TransportError::Protocol(format!("Mcp-Session-Id を作れませんでした: {error}")))?;
      builder = builder.header(HeaderName::from_static("mcp-session-id"), value);
      // 初期化で合意した版（2025-06-18）では、初期化後の要求に MCP-Protocol-Version が
      // 必須です。付けないと、サーバーは要求を旧版として解釈することがあります。
      builder =
        builder.header(HeaderName::from_static("mcp-protocol-version"), HeaderValue::from_static(PROTOCOL_VERSION));
    }
    builder
      .body(Full::new(Bytes::from(body.unwrap_or_default())))
      .map_err(|error| TransportError::Protocol(error.to_string()))
  }

  async fn exchange(
    &self,
    message: Value,
    session: Option<&str>,
    request_timeout: Duration,
  ) -> Result<(Option<String>, JsonRpcIncoming), ExchangeError> {
    let body = serde_json::to_vec(&message)
      .map_err(|error| ExchangeError::Transport(TransportError::InvalidJson(error.to_string())))?;
    let request = self.request("POST", session, Some(body)).map_err(ExchangeError::Transport)?;
    let response = timeout(request_timeout, self.client.request(request))
      .await
      .map_err(|_| ExchangeError::Transport(TransportError::Timeout(request_timeout.as_secs())))?
      .map_err(|error| ExchangeError::Transport(TransportError::Io(error.to_string())))?;
    let status = response.status();
    let content_type = response.headers().get("content-type").and_then(|value| value.to_str().ok()).map(str::to_owned);
    let session_id = response.headers().get("mcp-session-id").and_then(|value| value.to_str().ok()).map(str::to_owned);
    let collected = timeout(request_timeout, read_body(response.into_body()))
      .await
      .map_err(|_| ExchangeError::Transport(TransportError::Timeout(request_timeout.as_secs())))??;
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

impl Drop for SessionInner {
  /// 最後の持ち主が消えたら、保持しているセッションを背後で閉じる。接続先の設定が変わって
  /// 置き場を作り直したときに、古いセッションをサーバーへ残しません。
  fn drop(&mut self) {
    if let Some(closing) = self.take_and_close() {
      spawn_close(closing);
    }
  }
}

/// セッションを閉じる処理を背後で走らせる。ランタイムの外（停止の途中など）では送りません。
fn spawn_close(closing: impl std::future::Future<Output = ()> + Send + 'static) {
  if let Ok(runtime) = tokio::runtime::Handle::try_current() {
    runtime.spawn(closing);
  }
}

/// 1 台の常駐 HTTP サーバーへ `tools/call` を送るハンドル。
///
/// [`HttpSession`] のセッション ID を、要求へ `Mcp-Session-Id` として付けます。セッションは
/// 同じ接続先のハンドルどうしで共有でき、要求は互いを待たずに並行して送ります。
/// 未知セッション（404）は、セッションが消えたとみなして作り直し、同じ要求を 1 回だけ
/// 再試行します。
pub struct HttpReadHandle {
  session:           HttpSession,
  request_timeout:   Duration,
  /// 接続の失敗（I/O エラー）で同じ要求を送り直すか。読み取りは送り直し、書き込みは
  /// 送り直しません（適用済みの書き込みを重複送信しないため）。
  retry_on_io_error: bool,
}

impl HttpReadHandle {
  /// 接続先と 1 呼び出しのタイムアウトからハンドルを作る。まだセッションは作りません。
  /// このハンドルだけが使うセッションを持ち、ハンドルが消えるとセッションを閉じます。
  pub fn new(connection: crate::config::McpConnection, request_timeout: Duration) -> Self {
    Self::with_session(HttpSession::new(connection), request_timeout)
  }

  /// 書き込み用のハンドルを作る。接続の失敗（I/O エラー）では再試行しません。サーバーが
  /// 書き込みを適用した後で接続が切れた場合と区別できず、再試行すると書き込みを重複して
  /// 送信するためです。未知セッション（404）の再試行は、要求が処理されていないことが
  /// 分かるため、書き込みでも行います。
  pub fn new_for_write(connection: crate::config::McpConnection, request_timeout: Duration) -> Self {
    Self::with_session_for_write(HttpSession::new(connection), request_timeout)
  }

  /// 共有するセッションを使う読み取り用のハンドルを作る。
  pub fn with_session(session: HttpSession, request_timeout: Duration) -> Self {
    HttpReadHandle { session, request_timeout, retry_on_io_error: true }
  }

  /// 共有するセッションを使う書き込み用のハンドルを作る。再試行の扱いは
  /// [`Self::new_for_write`] と同じです。
  pub fn with_session_for_write(session: HttpSession, request_timeout: Duration) -> Self {
    HttpReadHandle { session, request_timeout, retry_on_io_error: false }
  }

  /// このハンドルのセッションが確立済みかどうかを数で返す（0 または 1）。
  pub fn session_count(&self) -> usize {
    usize::from(self.session.inner.current().is_some())
  }

  /// 接続先の URL。テストから接続状態の観測に使います。
  pub fn url(&self) -> String {
    self.session.inner.server.endpoint.url()
  }

  /// セッションをサーバーへの DELETE で閉じ、応答（または打ち切り）まで待つ。
  pub async fn close(&self) {
    self.session.close().await;
  }

  /// セッションを確立する。確立済みなら何もしません。
  ///
  /// 書き込みの前に呼ぶと、道具を送る前の失敗と、送ったあとの失敗を呼び出し側が区別できます。
  pub async fn connect(&mut self) -> Result<(), TransportError> {
    self.ensure_session().await.map(|_| ())
  }

  /// セッションを得る。確立は別のタスクで最後まで走らせます。呼び出しが途中で打ち切られても、
  /// サーバーが作り終えたセッションを保存し、次の要求が使えるようにするためです。
  async fn ensure_session(&self) -> Result<String, TransportError> {
    if let Some(session) = self.session.inner.current() {
      return Ok(session);
    }
    let inner = self.session.inner.clone();
    let request_timeout = self.request_timeout;
    tokio::spawn(async move { inner.establish(request_timeout).await })
      .await
      .map_err(|error| TransportError::Io(format!("HTTP セッションの確立が中断されました: {error}")))?
  }

  /// 1 回の tools/call を送る。
  ///
  /// 接続の失敗（I/O エラー）は、読み取りだけ同じセッションで 1 回送り直します。書き込み用の
  /// ハンドルでは送り直しません（`retry_on_io_error`）。未知セッション（404）は、要求が
  /// 処理されていないことが分かるため、セッションを作り直して 1 回だけ再試行します。
  /// サーバーの再起動は、送り直した要求への 404 として分かります。それ以外の失敗は
  /// 輸送エラーとして表面化させます。
  async fn tools_call(&mut self, name: &str, arguments: Value) -> Result<Value, TransportError> {
    let session = self.ensure_session().await?;
    let mut failure = match self.call_once(name, &arguments, &session).await {
      | Ok(incoming) => return unwrap_result(incoming),
      | Err(error) => error,
    };
    if failure.io_error() && self.retry_on_io_error {
      failure = match self.call_once(name, &arguments, &session).await {
        | Ok(incoming) => return unwrap_result(incoming),
        | Err(error) => error,
      };
    }
    if !failure.unknown_session() {
      return Err(failure.into_transport());
    }
    // セッションが消えた（サーバー再起動など）。セッションを作り直して 1 回だけ再試行します。
    self.session.inner.forget(&session);
    let session = self.ensure_session().await?;
    match self.call_once(name, &arguments, &session).await {
      | Ok(incoming) => unwrap_result(incoming),
      | Err(error) => Err(error.into_transport()),
    }
  }

  async fn call_once(&self, name: &str, arguments: &Value, session: &str) -> Result<JsonRpcIncoming, ExchangeError> {
    let request = json!({
      "jsonrpc": "2.0",
      "id": self.session.inner.take_id(),
      "method": "tools/call",
      "params": { "name": name, "arguments": arguments },
    });
    self.session.inner.exchange(request, Some(session), self.request_timeout).await.map(|(_, incoming)| incoming)
  }
}

fn unwrap_result(incoming: JsonRpcIncoming) -> Result<Value, TransportError> {
  if let Some(error) = incoming.error {
    return Err(TransportError::Rpc { code: error.code, message: error.message });
  }
  Ok(incoming.result.unwrap_or(Value::Null))
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
  /// 接続先は 127.0.0.1 の URL だけで決まるため、トークン（authorization）は鍵に入れません。
  fn connection_key(&self) -> String {
    format!("http:{}", self.url())
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
  /// 接続の失敗（I/O エラー）かどうか。要求がサーバーに届いたかは分かりません。
  fn io_error(&self) -> bool {
    matches!(self, ExchangeError::Transport(TransportError::Io(_)))
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

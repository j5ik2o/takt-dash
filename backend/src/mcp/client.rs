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
  /// 要求を送る前に失敗した。takt-mcp は要求を受け取っていません。接続を張れなかった場合と、
  /// 消えたセッションを作り直せなかった場合です。
  #[error("takt-mcp へ要求を送れませんでした: {0}")]
  NotSent(String),
  #[error("takt-mcp から {0} 秒以内に応答がありませんでした")]
  Timeout(u64),
  #[error("takt-mcp の再起動を抑制しています。あと {0} 秒ほど待ってから再試行します")]
  Backoff(u64),
  #[error("takt-mcp の応答が JSON として不正です: {0}")]
  InvalidJson(String),
  #[error("takt-mcp の HTTP サーバーとのやり取りに失敗しました: {0}")]
  Http(String),
  #[error("takt-mcp の HTTP 応答が MCP の契約に合いません: {0}")]
  Protocol(String),
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

/// 道具を呼び出せる接続の抽象。stdio の子プロセスとの往復（[`McpClient`]）と、常駐する HTTP
/// サーバーとの往復（[`crate::mcp::HttpReadHandle`]）の両方を [`crate::mcp::ToolSet`] が
/// 同じ形で扱えるようにします。`async fn` をトレイトオブジェクトにするため、返り値は
/// ボックス化した `Future` にします。
pub trait ToolCaller: Send {
  fn call_tool<'a>(
    &'a mut self,
    name: &'a str,
    arguments: Value,
  ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, CallError>> + Send + 'a>>;

  /// 接続先を区別する鍵。接続先が変わると値も変わり、
  /// 旧接続先から読んだ記憶を使わないために使います。
  fn connection_key(&self) -> String;

  /// 呼び出しの前に接続の開始を済ませる。stdio の子プロセスはここで起動と初期化を
  /// 済ませます。HTTP のハンドルはセッションを呼び出しの中で作るため、何もしません。
  fn ensure_started<'a>(
    &'a mut self,
  ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), TransportError>> + Send + 'a>> {
    Box::pin(async { Ok(()) })
  }
}

/// stdio 接続先を区別する鍵。環境変数はトークンを含みうるため、値を鍵に入れず、ハッシュだけを
/// 入れます。
pub(super) fn stdio_connection_key(command: &str, args: &[String], env: &BTreeMap<String, String>) -> String {
  use std::hash::{Hash, Hasher};
  let mut hasher = std::collections::hash_map::DefaultHasher::new();
  env.hash(&mut hasher);
  format!("stdio:{command:?}:{args:?}:env-{:016x}", hasher.finish())
}

impl ToolCaller for McpClient {
  fn connection_key(&self) -> String {
    stdio_connection_key(&self.config.command, &self.config.args, &self.config.env)
  }

  fn call_tool<'a>(
    &'a mut self,
    name: &'a str,
    arguments: Value,
  ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, CallError>> + Send + 'a>> {
    Box::pin(McpClient::call_tool(self, name, arguments))
  }

  fn ensure_started<'a>(
    &'a mut self,
  ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), TransportError>> + Send + 'a>> {
    Box::pin(McpClient::ensure_started(self))
  }
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
  config: McpClientConfig,
  process: Option<McpProcess>,
  next_id: u64,
  /// 起動と初期化の連続失敗の回数。再起動を抑える間隔を決めます。
  consecutive_spawn_failures: u32,
  /// この時刻までは子プロセスを起動しません。起動に失敗した間だけ設定します。
  next_spawn_at: Option<Instant>,
  /// このクライアントが担う接続レーンの名前（例: `poller`、`screen-0`、`write`）。
  /// 起動とエラーのログで、どのレーンの子プロセスかを識別するために使います。
  /// レジストリ経由で作るクライアントだけが持ち、直接作る場合は空です。
  lane: String,
}

/// 起動に失敗したときに、次の起動を抑えるか。
#[derive(Clone, Copy, PartialEq, Eq)]
enum OnSpawnFailure {
  /// 次の起動を間隔を空けて行う。呼び出しの経路の起動です。
  BackOff,
  /// 次の起動を抑えない。温めの起動です。
  Retry,
}

struct McpProcess {
  child:  Child,
  stdin:  BufWriter<ChildStdin>,
  stdout: BufReader<ChildStdout>,
}

impl McpClient {
  /// まだ子プロセスを起動しないクライアントを作る。最初の呼び出しで起動します。
  pub fn new(config: McpClientConfig) -> Self {
    Self::with_lane(config, "")
  }

  /// レーン名つきでクライアントを作る。レジストリがレーンごとに作るときに使います。
  pub(crate) fn with_lane(config: McpClientConfig, lane: &str) -> Self {
    McpClient {
      config,
      process: None,
      next_id: 1,
      consecutive_spawn_failures: 0,
      next_spawn_at: None,
      lane: lane.to_owned(),
    }
  }

  /// 子プロセスの起動と初期化だけを行い、道具は呼ばない。
  ///
  /// 書き込みレーンのように、最初の利用が利用者の操作で遅延がそのまま見えるレーンを
  /// 起動時に温めるために使います。既に起動済みなら何もしません。
  ///
  /// 温めの起動失敗は再起動の抑制を始めません。温めをしない場合と同じく、次の利用が
  /// 起動を試みられるようにするためです。
  pub async fn warm_up(&mut self) -> Result<(), TransportError> {
    self.start(OnSpawnFailure::Retry).await
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

  /// 子プロセスが未起動なら起動して initialize まで済ませる。
  ///
  /// 道具呼び出しの計時から起動・初期化を除くため、呼び出し側が計時前に準備だけを進められます。
  /// 起動に失敗したら、次の起動を間隔を空けて行います。
  pub(crate) async fn ensure_started(&mut self) -> Result<(), TransportError> {
    self.start(OnSpawnFailure::BackOff).await
  }

  async fn start(&mut self, on_failure: OnSpawnFailure) -> Result<(), TransportError> {
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
        tracing::info!(lane = %self.lane, "takt-mcp の子プロセスを起動しました。");
        Ok(())
      },
      | Err(error) => {
        if on_failure == OnSpawnFailure::BackOff {
          self.consecutive_spawn_failures = self.consecutive_spawn_failures.saturating_add(1);
          self.next_spawn_at = Some(Instant::now() + spawn_backoff(self.consecutive_spawn_failures));
        }
        tracing::warn!(lane = %self.lane, error = %error, "takt-mcp の子プロセスを起動できませんでした。");
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

  /// 往復の間は子プロセスをクライアントから外して持ち、応答を読み終えてから戻す。
  ///
  /// 呼び出しが途中で打ち切られると、外した子プロセスはここで破棄されます（起動時の
  /// `kill_on_drop` で終了します）。読みかけの応答を残した子プロセスを次の要求へ
  /// 使い回さないためです。
  async fn request(&mut self, request: Request, timeout: Duration) -> Result<Value, TransportError> {
    let timeout_secs = timeout.as_secs();
    let Some(mut process) = self.process.take() else {
      return Err(TransportError::ProcessExited);
    };
    match process.exchange(request, timeout_secs).await {
      | Ok(result) => {
        self.process = Some(process);
        Ok(result)
      },
      | Err(error) => {
        tracing::warn!(lane = %self.lane, error = %error, "takt-mcp との通信に失敗したため、子プロセスを破棄します。");
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
      let incoming: Incoming = serde_json::from_str(&line)
        .map_err(|error| TransportError::InvalidJson(format!("{error}: {}", super::response_excerpt(&line))))?;
      if incoming.is_server_request() {
        let response = Response::empty_result(incoming.id.clone().unwrap_or(Value::Null));
        self.send(&Outgoing::Response(response)).await?;
        continue;
      }
      if incoming.is_notification() {
        continue;
      }
      if let Some(error) = incoming.error {
        // 中断した呼び出しのエラー応答が残っていても、それはこの要求の失敗では
        // ありません。ID が一致するエラーと、ID を持たないプロトコルのエラーだけを返します。
        if incoming.id.is_none() || incoming.id.as_ref().and_then(Value::as_u64) == Some(id) {
          return Err(TransportError::Rpc { code: error.code, message: error.message });
        }
        continue;
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
pub(super) fn extract_tool_text(tool: &str, result: &Value) -> Result<String, ToolCallError> {
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

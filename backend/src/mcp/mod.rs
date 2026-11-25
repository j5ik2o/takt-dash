//! takt-mcp との JSON-RPC（MCP）通信。
//!
//! 書き込みは takt-mcp の子プロセスをプロジェクトの cwd で起動し、標準入出力で
//! initialize・notifications/initialized・tools/call を話します（stdio）。読み取りは home ごとに
//! 常駐する takt-mcp の HTTP サーバーへ `tools/call` を送ります（http）。子プロセスの起動失敗・
//! タイムアウト・不正な応答・プロセス死亡・HTTP の失敗は輸送エラーとして扱い、道具が isError を
//! 返した場合は道具エラーとして区別します。

mod client;
mod http;
mod metrics;
mod protocol;
mod registry;
pub mod tools;

/// 設定の接続先を区別する鍵。[`ToolCaller::connection_key`] と同じ値です。
///
/// 読み取り結果がどの接続先から読まれたかを、設定から解決した接続先と比べるために使います。
pub fn connection_key(connection: &crate::config::McpConnection) -> String {
  match connection {
    | crate::config::McpConnection::Stdio(server) => {
      client::stdio_connection_key(&server.command, &server.args, &server.env)
    },
    | crate::config::McpConnection::Http(server) => http::http_connection_key(server),
  }
}

pub use client::{CallError, McpClient, McpClientConfig, ToolCallError, ToolCaller, TransportError};
pub use http::HttpReadHandle;
pub use metrics::{ToolCallMetrics, ToolCallOutcome, ToolCallStat};
pub use registry::{ClientHandle, Lane, McpClientRegistry};
pub use tools::{
  client_config, ApproveGoalRegistrationResult, MergeGoalPrResult, ReadToolSet, RemoteRun, ToolError, WriteToolSet,
};

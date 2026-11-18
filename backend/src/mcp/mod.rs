//! takt-mcp の子プロセスとの JSON-RPC（MCP）通信。
//!
//! 子プロセスをプロジェクトの cwd で起動し、標準入出力で initialize・notifications/initialized・
//! tools/call を話します。起動失敗・タイムアウト・不正な応答・プロセス死亡は輸送エラーとして扱い、
//! 道具が isError を返した場合は道具エラーとして区別します。

mod client;
mod metrics;
mod protocol;
mod registry;
pub mod tools;

pub use client::{CallError, McpClient, McpClientConfig, ToolCallError, TransportError};
pub use metrics::{ToolCallMetrics, ToolCallOutcome, ToolCallStat};
pub use registry::{ClientHandle, Lane, McpClientRegistry};
pub use tools::{
  client_config, ApproveGoalRegistrationResult, MergeGoalPrResult, ReadToolSet, RemoteRun, ToolError, WriteToolSet,
};

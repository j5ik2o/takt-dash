//! JSON-RPC 2.0 の線形式。MCP のメッセージは改行区切りの JSON です。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 要求する MCP のプロトコル版。
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// クライアントから送る JSON-RPC メッセージ。
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum Outgoing {
  Request(Request),
  Notification(Notification),
  Response(Response),
}

#[derive(Debug, Serialize)]
pub struct Request {
  pub jsonrpc: &'static str,
  pub id:      u64,
  pub method:  String,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub params:  Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct Notification {
  pub jsonrpc: &'static str,
  pub method:  String,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub params:  Option<Value>,
}

/// サーバーからの要求に返す応答。
#[derive(Debug, Serialize)]
pub struct Response {
  pub jsonrpc: &'static str,
  pub id:      Value,
  pub result:  Value,
}

impl Request {
  pub fn new(id: u64, method: impl Into<String>, params: Option<Value>) -> Self {
    Request { jsonrpc: "2.0", id, method: method.into(), params }
  }
}

impl Notification {
  pub fn new(method: impl Into<String>, params: Option<Value>) -> Self {
    Notification { jsonrpc: "2.0", method: method.into(), params }
  }
}

impl Response {
  pub fn empty_result(id: Value) -> Self {
    Response { jsonrpc: "2.0", id, result: Value::Null }
  }
}

/// サーバーから届く JSON-RPC メッセージ。
#[derive(Debug, Deserialize)]
pub struct Incoming {
  #[serde(default)]
  pub id:     Option<Value>,
  #[serde(default)]
  pub result: Option<Value>,
  #[serde(default)]
  pub error:  Option<RpcError>,
  #[serde(default)]
  pub method: Option<String>,
}

impl Incoming {
  /// サーバーからの要求（id と method を持ち、result も error も持たない）。
  pub fn is_server_request(&self) -> bool {
    self.id.is_some() && self.method.is_some() && self.result.is_none() && self.error.is_none()
  }

  /// 通知（id を持たず method を持つ）。
  pub fn is_notification(&self) -> bool {
    self.id.is_none() && self.method.is_some()
  }
}

/// JSON-RPC のエラー。
#[derive(Debug, Deserialize)]
pub struct RpcError {
  pub code:    i64,
  pub message: String,
}

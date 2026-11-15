//! プロジェクト単位の takt-mcp 子プロセスの共有レジストリ。
//!
//! 定期読み取り（[`crate::poller::Poller`]）と gRPC の書き込み RPC が同じ
//! `(server_id, project)` のクライアントを共有し、読み取りと操作を同じ接続で行います。

use std::{
  collections::BTreeMap,
  sync::{Arc, Mutex},
};

use tokio::sync::Mutex as AsyncMutex;

use super::{McpClient, McpClientConfig};

/// レジストリのキー。設定上、プロジェクト名はサーバー内で一意です。
type ClientKey = (String, String);

type ClientHandle = Arc<AsyncMutex<McpClient>>;

/// プロジェクトごとの takt-mcp クライアントを所有するレジストリ。
///
/// map 自体の排他は [`std::sync::Mutex`] で行い、ガードは `entry`・`get`・`retain` の
/// 式の中だけに閉じて `.await` を跨ぎません。クライアントごとの排他は
/// [`tokio::sync::Mutex`] で行い、1 回の `ToolSet` 操作の間だけ保持します。ロックを
/// 入れ子にしないため、書き込みが読み取りを待たせてもデッドロックしません。
#[derive(Clone, Default)]
pub struct McpClientRegistry {
  clients: Arc<Mutex<BTreeMap<ClientKey, ClientHandle>>>,
}

impl McpClientRegistry {
  pub fn new() -> Self {
    Self::default()
  }

  /// 指定したプロジェクトのクライアントを返す。無ければ `make` で起動設定を作って登録します。
  ///
  /// 子プロセスの起動は最初の道具呼び出しまで遅延します。
  pub fn client(&self, server_id: &str, project: &str, make: impl FnOnce() -> McpClientConfig) -> ClientHandle {
    let key = (server_id.to_owned(), project.to_owned());
    let mut clients = self.clients.lock().expect("takt-mcp レジストリのロックが壊れています");
    clients.entry(key).or_insert_with(|| Arc::new(AsyncMutex::new(McpClient::new(make())))).clone()
  }

  /// 指定したプロジェクトのクライアントを返す。登録が無ければ `None` を返します。
  pub fn get(&self, server_id: &str, project: &str) -> Option<ClientHandle> {
    let key = (server_id.to_owned(), project.to_owned());
    let clients = self.clients.lock().expect("takt-mcp レジストリのロックが壊れています");
    clients.get(&key).cloned()
  }

  /// 指定したキー以外のクライアントをレジストリから外す。
  ///
  /// 誰も保持していないクライアントはここで破棄され、子プロセスは
  /// [`McpClient`] の `kill_on_drop` で停止します。書き込みがまだ同じハンドルを
  /// 保持している場合は、その操作が終わってハンドルが消えた時点で停止します。
  pub fn retain(&self, wanted: &[(String, String)]) {
    let mut clients = self.clients.lock().expect("takt-mcp レジストリのロックが壊れています");
    clients.retain(|(server_id, project), _| wanted.iter().any(|key| &key.0 == server_id && &key.1 == project));
  }

  /// すべてのクライアントを map から取り出し、1 件ずつ排他取得して停止する。
  ///
  /// map のロックは取り出しの後すぐに解放します。クライアントごとのロックだけを
  /// 取得するため、停止が別の操作を待つことはあってもデッドロックしません。
  pub async fn shutdown_all(&self) {
    let clients: Vec<ClientHandle> = {
      let mut clients = self.clients.lock().expect("takt-mcp レジストリのロックが壊れています");
      std::mem::take(&mut *clients).into_values().collect()
    };
    for client in clients {
      client.lock().await.shutdown().await;
    }
  }
}

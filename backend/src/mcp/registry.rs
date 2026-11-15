//! プロジェクト単位の takt-mcp 子プロセスの共有レジストリ。
//!
//! 定期読み取り（[`crate::poller::Poller`]）と gRPC の書き込み RPC が同じ
//! `(server_id, project)` のクライアントを共有し、読み取りと操作を同じ接続で行います。
//! 起動設定が変わったプロジェクトは旧ハンドルを外して作り直し、作り直したことを記録して
//! 定期読み取り側が差分基準をリセットできるようにします。

use std::{
  collections::{BTreeMap, BTreeSet},
  sync::{Arc, Mutex},
};

use tokio::sync::Mutex as AsyncMutex;

use super::{McpClient, McpClientConfig};

/// レジストリのキー。設定上、プロジェクト名はサーバー内で一意です。
type ClientKey = (String, String);

type ClientHandle = Arc<AsyncMutex<McpClient>>;

/// 1 プロジェクト分のクライアントと、その起動設定。
struct Entry {
  config: McpClientConfig,
  client: ClientHandle,
}

/// レジストリの内部状態。map と「作り直した」集合を同じロックで保護します。
#[derive(Default)]
struct RegistryInner {
  clients: BTreeMap<ClientKey, Entry>,
  rebuilt: BTreeSet<ClientKey>,
}

/// プロジェクトごとの takt-mcp クライアントを所有するレジストリ。
///
/// map 自体の排他は [`std::sync::Mutex`] で行い、ガードは `client`・`get`・`retain`・
/// `take_rebuilt_for` の式の中だけに閉じて `.await` を跨ぎません。クライアントごとの排他は
/// [`tokio::sync::Mutex`] で行い、1 回の `ToolSet` 操作の間だけ保持します。ロックを
/// 入れ子にしないため、書き込みが読み取りを待たせてもデッドロックしません。
#[derive(Clone, Default)]
pub struct McpClientRegistry {
  inner: Arc<Mutex<RegistryInner>>,
}

impl McpClientRegistry {
  pub fn new() -> Self {
    Self::default()
  }

  /// 指定したプロジェクトのクライアントを返す。
  ///
  /// 登録が無ければ `make` で起動設定を作って登録します。登録済みのクライアントの起動設定が
  /// `make` の返す設定と異なる場合は、旧ハンドルをレジストリから外して新しい設定で作り直し、
  /// 作り直したことを記録します。設定が同じなら子プロセスを保ったまま同じハンドルを返します。
  ///
  /// 子プロセスの起動は最初の道具呼び出しまで遅延します。
  pub fn client(&self, server_id: &str, project: &str, make: impl FnOnce() -> McpClientConfig) -> ClientHandle {
    let key = (server_id.to_owned(), project.to_owned());
    let wanted = make();
    let (handle, replaced) = {
      let mut inner = self.inner.lock().expect("takt-mcp レジストリのロックが壊れています");
      if let Some(entry) = inner.clients.get(&key) {
        if entry.config == wanted {
          return entry.client.clone();
        }
      }
      inner.rebuilt.insert(key.clone());
      let handle = Arc::new(AsyncMutex::new(McpClient::new(wanted.clone())));
      let replaced = inner.clients.insert(key, Entry { config: wanted, client: handle.clone() });
      (handle, replaced)
    };
    // 旧ハンドルは map のロックを解放してから破棄する。書き込みがまだ保持している場合は、
    // その操作が終わって最後のハンドルが消えた時点で子プロセスが停止します。
    drop(replaced);
    handle
  }

  /// 指定したプロジェクトのクライアントを返す。登録が無ければ `None` を返します。
  pub fn get(&self, server_id: &str, project: &str) -> Option<ClientHandle> {
    let key = (server_id.to_owned(), project.to_owned());
    let inner = self.inner.lock().expect("takt-mcp レジストリのロックが壊れています");
    inner.clients.get(&key).map(|entry| entry.client.clone())
  }

  /// 指定したキー以外のクライアントをレジストリから外す。
  ///
  /// 誰も保持していないクライアントはここで破棄され、子プロセスは
  /// [`McpClient`] の `kill_on_drop` で停止します。書き込みがまだ同じハンドルを
  /// 保持している場合は、その操作が終わってハンドルが消えた時点で停止します。
  pub fn retain(&self, wanted: &[(String, String)]) {
    let mut inner = self.inner.lock().expect("takt-mcp レジストリのロックが壊れています");
    inner.clients.retain(|(server_id, project), _| wanted.iter().any(|key| &key.0 == server_id && &key.1 == project));
    inner.rebuilt.retain(|(server_id, project)| wanted.iter().any(|key| &key.0 == server_id && &key.1 == project));
  }

  /// 指定したプロジェクトが作り直されたかを返し、記録を消す。
  ///
  /// 定期読み取りは読み取りの前にこれを呼び、作り直したプロジェクトの差分基準を
  /// リセットします。読み取り側の設定差分と、書き込み RPC 経由の作り直しの双方を拾えます。
  pub fn take_rebuilt_for(&self, server_id: &str, project: &str) -> bool {
    let key = (server_id.to_owned(), project.to_owned());
    let mut inner = self.inner.lock().expect("takt-mcp レジストリのロックが壊れています");
    inner.rebuilt.remove(&key)
  }

  /// すべてのクライアントを map から取り出し、1 件ずつ排他取得して停止する。
  ///
  /// map のロックは取り出しの後すぐに解放します。クライアントごとのロックだけを
  /// 取得するため、停止が別の操作を待つことはあってもデッドロックしません。
  pub async fn shutdown_all(&self) {
    let clients: Vec<ClientHandle> = {
      let mut inner = self.inner.lock().expect("takt-mcp レジストリのロックが壊れています");
      inner.rebuilt.clear();
      std::mem::take(&mut inner.clients).into_values().map(|entry| entry.client).collect()
    };
    for client in clients {
      client.lock().await.shutdown().await;
    }
  }
}

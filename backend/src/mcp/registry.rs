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

/// 定期読み取りへ渡す、同じ世代のクライアントとその読取先。
pub(crate) struct ClientRead {
  /// 読み取りに使うクライアント。取得した世代のハンドルを、周期の最後まで使います。
  pub(crate) client:  ClientHandle,
  /// そのクライアントを作るときに使った起動設定の cwd。道具呼び出しとローカル run の
  /// 集計の読取先に使います。
  pub(crate) cwd:     std::path::PathBuf,
  /// このクライアントが前回の読み取り以降に作り直されたか。初回作成・設定変更の双方で真に
  /// なり、`take_for_read` が消費すると次に作り直すまで偽に戻ります。
  pub(crate) rebuilt: bool,
}

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
/// map 自体の排他は [`std::sync::Mutex`] で行い、ガードは `client`・`take_for_read`・`retain`
/// の式の中だけに閉じて `.await` を跨ぎません。クライアントごとの排他は
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

  /// 定期読み取り用に、指定したプロジェクトのクライアントと、作り直したかの記録、
  /// およびそのクライアントの起動設定の cwd を一体で取り出す。
  ///
  /// ハンドルの複製・`rebuilt` の消費・`cwd` の複製を同じロックで行うため、取得する
  /// ハンドル・「作り直した」の記録・読取先の cwd は常に同じ世代に対応します。これがないと、
  /// 通知の消費とハンドルの取得の間に書き込み RPC が作り直した場合、旧世代のハンドルを
  /// 新世代の基準で読むか、新世代の通知を消費したまま旧世代の比較基準を使う取り違えが
  /// 起きます。さらに、取得したハンドルの起動設定の cwd と、周期冒頭に読み込んだ設定の
  /// cwd が食い違うと、道具呼び出しとローカル run の集計を旧接続先のディレクトリで
  /// 行ってしまいます。
  ///
  /// 戻り値は、登録が無ければ `None`、あれば同じエントリから作った [`ClientRead`] です。
  /// 作り直した記録は呼び出し時に消費され、次に作り直すまで再び真にはなりません。
  pub(crate) fn take_for_read(&self, server_id: &str, project: &str) -> Option<ClientRead> {
    let key = (server_id.to_owned(), project.to_owned());
    let mut inner = self.inner.lock().expect("takt-mcp レジストリのロックが壊れています");
    let entry = inner.clients.get(&key).map(|entry| (entry.client.clone(), entry.config.cwd.clone()));
    let rebuilt = inner.rebuilt.remove(&key);
    entry.map(|(client, cwd)| ClientRead { client, cwd, rebuilt })
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

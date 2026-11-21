//! プロジェクト単位の takt-mcp 子プロセスの共有レジストリ。
//!
//! 接続は用途別のレーンに分けます。定期読み取り（[`crate::poller::Poller`]）は
//! [`Lane::Poller`]、画面からの読み取りは [`Lane::Screen`]、書き込み RPC は
//! [`Lane::Write`] を使い、レーンごとに別の子プロセスを持ちます。重い読み取りが
//! 走っても、書き込みと画面の読み取りは別の子プロセスで進みます。
//! 起動設定が変わったプロジェクトは旧ハンドルを外して作り直し、作り直したことを記録して
//! 定期読み取り側が差分基準をリセットできるようにします。
//!
//! あわせて、プロジェクトごとの書き込みの通し番号を持ちます。書き込み RPC が成功する
//! たびに進め、定期読み取りは読み取り開始時点の番号と比べることで、読み取りの途中で
//! 書き込みが完了した場合に古い読み取り結果を捨てられます。
//!
//! 通し番号の判定と反映が書き込みと競合しないよう、プロジェクトごとの書き込みゲートも
//! 持ちます。書き込み側は書き込み RPC の発行前から通し番号の記録まで、定期読み取りは
//! 古さの確認から結果の反映の完了まで、同じゲートを保持して直列化します。

use std::{
  collections::{BTreeMap, BTreeSet},
  sync::{Arc, Mutex},
};

use tokio::sync::Mutex as AsyncMutex;

use super::{McpClient, McpClientConfig};
use crate::domain::ScreenReadLanes;

/// takt-mcp への接続の用途を表すレーン。
///
/// レーンごとに子プロセスが分かれるため、1つのレーンの重い呼び出しが他のレーンを
/// 待たせることはありません。画面読み取りレーンだけが複数本にでき、本数は設定で決まります。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lane {
  /// poller の定期読み取り。プロジェクトごとに1本で固定します。
  Poller,
  /// 画面からの読み取り。プロジェクトごとに設定値（1〜3本）だけ持ちます。
  Screen,
  /// 書き込み RPC。プロジェクトごとに1本で固定します。
  Write,
}

impl Lane {
  /// ログと識別に使うレーンの名前。
  pub fn name(self) -> &'static str {
    match self {
      | Lane::Poller => "poller",
      | Lane::Screen => "screen",
      | Lane::Write => "write",
    }
  }
}

/// レジストリの内部キー。画面読み取りレーンは本数分の番号で区別します。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum LaneKey {
  Poller,
  Screen(u32),
  Write,
}

impl LaneKey {
  /// 子プロセスのログへ出すレーン名。画面読み取りは番号つきで区別します。
  fn label(&self) -> String {
    match self {
      | LaneKey::Poller => "poller".to_owned(),
      | LaneKey::Screen(index) => format!("screen-{index}"),
      | LaneKey::Write => "write".to_owned(),
    }
  }
}

/// レジストリのキー。設定上、プロジェクト名はサーバー内で一意です。
type ClientKey = (String, String, LaneKey);

/// プロジェクトを指すキー。poller の再構築記録とレーンをまたぐ書き込みの記録に使います。
type ProjectKey = (String, String);

/// レジストリが貸し出すクライアントの取っ手。クライアントごとの排他はこの中の
/// [`tokio::sync::Mutex`] で行います。
pub type ClientHandle = Arc<AsyncMutex<McpClient>>;

/// 定期読み取りへ渡す、同じ取得時点の読取先と、書き込み先の通し番号。
pub(crate) struct ClientRead {
  pub(crate) client:         ClientHandle,
  pub(crate) cwd:            std::path::PathBuf,
  pub(crate) rebuilt:        bool,
  pub(crate) write_sequence: u64,
}

/// 1 レーン分のクライアントと、その起動設定。
struct Entry {
  config: McpClientConfig,
  client: ClientHandle,
}

/// レジストリの内部状態。map と「作り直した」集合と書き込みの通し番号を同じロックで保護します。
#[derive(Default)]
struct RegistryInner {
  clients:      BTreeMap<ClientKey, Entry>,
  rebuilt:      BTreeSet<ProjectKey>,
  /// 画面読み取りレーンの次回割り当て位置。プロジェクトごとにラウンドロビンで進めます。
  screen_next:  BTreeMap<ProjectKey, u32>,
  /// プロジェクトごとの書き込みの通し番号。書き込みが成功するたびに1つ進めます。
  write_counts: BTreeMap<ProjectKey, u64>,
  /// プロジェクトごとの書き込みゲート。書き込み経路と定期読み取りの反映の段が共有し、
  /// 通し番号の前進と読み取り結果の反映を直列化します。通し番号そのものとは別の構造に
  /// することで、ゲートを保持したまま通し番号を記録してもデッドロックしません。
  write_gates:  BTreeMap<ProjectKey, Arc<AsyncMutex<()>>>,
}

/// プロジェクトごとの takt-mcp クライアントをレーン別に所有するレジストリ。
///
/// map 自体の排他は [`std::sync::Mutex`] で行い、ガードは各メソッドの式の中だけに閉じて
/// `.await` を跨ぎません。クライアントごとの排他は [`tokio::sync::Mutex`] で行い、1 回の
/// 道具操作の間だけ保持します。
#[derive(Clone, Default)]
pub struct McpClientRegistry {
  inner: Arc<Mutex<RegistryInner>>,
}

impl McpClientRegistry {
  pub fn new() -> Self {
    Self::default()
  }

  /// 指定したプロジェクト・レーンのクライアントを返す。
  ///
  /// 登録が無ければ `make` で起動設定を作って登録します。登録済みのクライアントの起動設定が
  /// `make` の返す設定と異なる場合は、旧ハンドルをレジストリから外して新しい設定で作り直し、
  /// poller レーンの作り直しを記録します。戻り値の2つ目は、クライアントを新規に
  /// 作った（作り直しを含む）かどうかで、呼び出し側が起動時の温めを行う判断に使います。
  ///
  /// 子プロセスの起動は最初の道具呼び出しまで遅延します（温めは呼び出し側が行います）。
  pub fn client(
    &self,
    server_id: &str,
    project: &str,
    lane: Lane,
    make: impl FnOnce() -> McpClientConfig,
  ) -> (ClientHandle, bool) {
    let lane_key = match lane {
      | Lane::Poller => LaneKey::Poller,
      | Lane::Write => LaneKey::Write,
      // 画面読み取りを本数で選ぶ場合は screen_client を使います。ここでは1本目を返します。
      | Lane::Screen => LaneKey::Screen(0),
    };
    self.client_for_key(server_id, project, lane_key, make())
  }

  /// 画面読み取りレーンのクライアントを、要求ごとにラウンドロビンで1本選んで返す。
  ///
  /// `lanes` 本の中から前回の次の位置を選びます。全本が使用中でも、選んだ1本のロックが
  /// 空くのを待つ形で構いません（呼び出し側がロック取得で待ちます）。
  pub fn screen_client(
    &self,
    server_id: &str,
    project: &str,
    lanes: ScreenReadLanes,
    make: impl FnOnce() -> McpClientConfig,
  ) -> ClientHandle {
    let lanes = lanes.get();
    let index = {
      let mut inner = self.inner.lock().expect("takt-mcp レジストリのロックが壊れています");
      let project_key = (server_id.to_owned(), project.to_owned());
      let next = inner.screen_next.entry(project_key).or_insert(0);
      let index = *next % lanes;
      *next = next.wrapping_add(1);
      index
    };
    self.client_for_key(server_id, project, LaneKey::Screen(index), make()).0
  }

  /// プロジェクトの全レーンのクライアントをそろえる。poller のセットアップが使います。
  ///
  /// 定期読み取り・画面読み取り（`screen_lanes` 本）・書き込みの各レーンを作り、書き込み
  /// レーンのハンドルと、書き込みレーンを新規に作ったかどうかを返します。呼び出し側は、
  /// 新規に作った書き込みレーンだけを温めます。
  pub fn ensure_lanes(
    &self,
    server_id: &str,
    project: &str,
    screen_lanes: ScreenReadLanes,
    make: impl Fn() -> McpClientConfig,
  ) -> (ClientHandle, bool) {
    self.client_for_key(server_id, project, LaneKey::Poller, make());
    for index in 0..screen_lanes.get() {
      self.client_for_key(server_id, project, LaneKey::Screen(index), make());
    }
    let write = self.client_for_key(server_id, project, LaneKey::Write, make());
    self.prune_surplus_screen_lanes(server_id, project, screen_lanes);
    write
  }

  /// 本数を超えた画面読み取りレーンのクライアントをレジストリから外す。
  ///
  /// 設定で本数が減った場合に、余ったレーンのクライアントと子プロセスが残り続けない
  /// ようにします。外したハンドルは map のロックを解放してから破棄します。利用者が
  /// まだ保持している場合は、その操作が終わって最後のハンドルが消えた時点で
  /// 子プロセスが停止します。
  fn prune_surplus_screen_lanes(&self, server_id: &str, project: &str, screen_lanes: ScreenReadLanes) {
    let removed: Vec<Entry> = {
      let mut inner = self.inner.lock().expect("takt-mcp レジストリのロックが壊れています");
      let surplus: Vec<ClientKey> = inner
        .clients
        .keys()
        .filter(|(owner_server, owner_project, lane_key)| {
          owner_server.as_str() == server_id
            && owner_project.as_str() == project
            && matches!(lane_key, LaneKey::Screen(index) if *index >= screen_lanes.get())
        })
        .cloned()
        .collect();
      surplus.iter().filter_map(|key| inner.clients.remove(key)).collect()
    };
    drop(removed);
  }

  /// キーで指定したレーンのクライアントを取得または作成する共通処理。
  fn client_for_key(
    &self,
    server_id: &str,
    project: &str,
    lane_key: LaneKey,
    wanted: McpClientConfig,
  ) -> (ClientHandle, bool) {
    let key = (server_id.to_owned(), project.to_owned(), lane_key.clone());
    let (handle, created, replaced) = {
      let mut inner = self.inner.lock().expect("takt-mcp レジストリのロックが壊れています");
      if let Some(entry) = inner.clients.get(&key) {
        if entry.config == wanted {
          return (entry.client.clone(), false);
        }
      }
      let handle = Arc::new(AsyncMutex::new(McpClient::with_lane(wanted.clone(), &lane_key.label())));
      let replaced = inner.clients.insert(key, Entry { config: wanted, client: handle.clone() });
      // 作り直しの記録は、既存のクライアントを設定の違いで置き換えた場合だけに残します。
      // 新しいレーンの追加（画面読み取りレーンの増設など）では定期読み取りの接続先は
      // 変わっていないため、差分基準をリセットしません。
      if replaced.is_some() && lane_key == LaneKey::Poller {
        inner.rebuilt.insert((server_id.to_owned(), project.to_owned()));
      }
      (handle, true, replaced)
    };
    // 旧ハンドルは map のロックを解放してから破棄する。利用者がまだ保持している場合は、
    // その操作が終わって最後のハンドルが消えた時点で子プロセスが停止します。
    drop(replaced);
    (handle, created)
  }

  /// poller のハンドル・cwd・再構築記録・書き込み通し番号を同じロック内で取得する。
  ///
  /// RPC が別レーンを置き換えても、準備済みの poller の読取先と比較基準は変えません。
  /// 取得した cwd は道具とローカル run の両方に使い、周期冒頭の設定との混在を防ぎます。
  pub(crate) fn take_for_read(&self, server_id: &str, project: &str, write_server_id: &str) -> Option<ClientRead> {
    let key = (server_id.to_owned(), project.to_owned());
    let lane_key = (server_id.to_owned(), project.to_owned(), LaneKey::Poller);
    let mut inner = self.inner.lock().expect("takt-mcp レジストリのロックが壊れています");
    let entry = inner.clients.get(&lane_key).map(|entry| (entry.client.clone(), entry.config.cwd.clone()));
    let rebuilt = inner.rebuilt.remove(&key);
    // 通し番号は書き込み先（`write_server_id`）のものです。読み取り先と書き込み先が
    // 分かれる設定でも、書き込みゲートと同じ通し番号で読み取り結果の採用を判断します。
    let write_key = (write_server_id.to_owned(), project.to_owned());
    let write_sequence = inner.write_counts.get(&write_key).copied().unwrap_or(0);
    entry.map(|(client, cwd)| ClientRead { client, cwd, rebuilt, write_sequence })
  }

  /// 指定したプロジェクト以外のクライアントをレジストリから外す。
  ///
  /// 誰も保持していないクライアントはここで破棄され、子プロセスは
  /// [`McpClient`] の `kill_on_drop` で停止します。利用者がまだ同じハンドルを
  /// 保持している場合は、その操作が終わってハンドルが消えた時点で停止します。
  pub fn retain(&self, wanted: &[(String, String)]) {
    let mut inner = self.inner.lock().expect("takt-mcp レジストリのロックが壊れています");
    let wanted: BTreeSet<&ProjectKey> = wanted.iter().collect();
    inner.clients.retain(|(server_id, project, _), _| wanted.contains(&(server_id.clone(), project.clone())));
    inner.rebuilt.retain(|key| wanted.contains(key));
    inner.screen_next.retain(|key, _| wanted.contains(key));
    inner.write_counts.retain(|key, _| wanted.contains(key));
    inner.write_gates.retain(|key, _| wanted.contains(key));
  }

  /// プロジェクトの現在の書き込みの通し番号を返す。書き込みがまだ無ければ0です。
  pub fn write_sequence(&self, server_id: &str, project: &str) -> u64 {
    let key = (server_id.to_owned(), project.to_owned());
    let inner = self.inner.lock().expect("takt-mcp レジストリのロックが壊れています");
    inner.write_counts.get(&key).copied().unwrap_or(0)
  }

  /// 書き込みの成功を記録し、通し番号を1つ進めて新しい番号を返す。
  ///
  /// この関数は書き込みゲートを取得しません。ゲートを保持したまま呼んでも
  /// デッドロックしないため、書き込み経路はゲートの中でそのまま呼べます。
  pub fn record_write(&self, server_id: &str, project: &str) -> u64 {
    let key = (server_id.to_owned(), project.to_owned());
    let mut inner = self.inner.lock().expect("takt-mcp レジストリのロックが壊れています");
    let count = inner.write_counts.entry(key).or_insert(0);
    *count = count.saturating_add(1);
    *count
  }

  /// プロジェクトの書き込みゲートの取っ手を返す。登録が無ければ作ります。
  ///
  /// 書き込み経路は、書き込み RPC を発行する前から [`Self::record_write`] で通し番号を
  /// 進め終わるまでこのゲートを保持します。定期読み取りは、読み取り結果の古さの確認の
  /// 直前から、そのプロジェクトの出来事の記録と状態の反映が終わるまで同じゲートを
  /// 保持します。これにより、確認と反映の間に書き込みが割り込むことも、書き込みの
  /// 適用と番号の前進の間に読み取りが完了して古い結果が反映されることもありません。
  /// ゲートはプロジェクトごとであり、別プロジェクト同士は待たせ合いません。
  pub fn write_gate(&self, server_id: &str, project: &str) -> Arc<AsyncMutex<()>> {
    let key = (server_id.to_owned(), project.to_owned());
    let mut inner = self.inner.lock().expect("takt-mcp レジストリのロックが壊れています");
    inner.write_gates.entry(key).or_insert_with(|| Arc::new(AsyncMutex::new(()))).clone()
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

#[cfg(test)]
mod tests {
  //! レーンの分離とラウンドロビンと書き込みの通し番号を固定する。
  //!
  //! クライアントは子プロセスを起動しないまま作れるため、ハンドルの同一性だけで
  //! レーンの割り当てを観測します。

  use std::{path::PathBuf, sync::Arc, time::Duration};

  use super::{Lane, LaneKey, McpClientRegistry};
  use crate::{domain::ScreenReadLanes, mcp::McpClientConfig};

  fn config() -> McpClientConfig {
    McpClientConfig {
      command:         "node".to_owned(),
      args:            Vec::new(),
      env:             Default::default(),
      cwd:             PathBuf::from("/tmp"),
      request_timeout: Duration::from_secs(5),
      tool_timeouts:   Default::default(),
    }
  }

  #[test]
  fn lanes_have_distinct_clients() {
    let registry = McpClientRegistry::new();
    let (poller, _) = registry.client("local", "proj", Lane::Poller, config);
    let (write, _) = registry.client("local", "proj", Lane::Write, config);
    let screen = registry.screen_client("local", "proj", ScreenReadLanes::clamped(1), config);
    assert!(!Arc::ptr_eq(&poller, &write), "定期読み取りと書き込みは別のクライアントです");
    assert!(!Arc::ptr_eq(&poller, &screen), "定期読み取りと画面読み取りは別のクライアントです");
    assert!(!Arc::ptr_eq(&write, &screen), "書き込みと画面読み取りは別のクライアントです");
    let (poller_again, created) = registry.client("local", "proj", Lane::Poller, config);
    assert!(Arc::ptr_eq(&poller, &poller_again), "同じレーンは同じクライアントを返します");
    assert!(!created, "既存のクライアントは新規作成ではありません");
  }

  #[test]
  fn screen_clients_rotate_round_robin() {
    let registry = McpClientRegistry::new();
    let first = registry.screen_client("local", "proj", ScreenReadLanes::clamped(2), config);
    let second = registry.screen_client("local", "proj", ScreenReadLanes::clamped(2), config);
    let third = registry.screen_client("local", "proj", ScreenReadLanes::clamped(2), config);
    assert!(!Arc::ptr_eq(&first, &second), "2本の画面読み取りは別のクライアントです");
    assert!(Arc::ptr_eq(&first, &third), "3回目は1本目へ戻ります");
  }

  #[test]
  fn write_sequence_advances_per_project() {
    let registry = McpClientRegistry::new();
    assert_eq!(registry.write_sequence("local", "proj"), 0);
    assert_eq!(registry.record_write("local", "proj"), 1);
    assert_eq!(registry.record_write("local", "proj"), 2);
    assert_eq!(registry.write_sequence("local", "proj"), 2);
    assert_eq!(registry.write_sequence("local", "other"), 0, "通し番号はプロジェクトごとに独立です");
  }

  #[test]
  fn read_acquisition_keeps_the_poller_source_when_other_lanes_change() {
    let registry = McpClientRegistry::new();
    registry.ensure_lanes("local", "proj", ScreenReadLanes::clamped(1), config);
    let mut changed = config();
    changed.cwd = PathBuf::from("/tmp/changed");
    registry.client("local", "proj", Lane::Write, || changed.clone());
    registry.client("local", "proj", Lane::Screen, || changed.clone());
    registry.record_write("local", "proj");

    let read = registry.take_for_read("local", "proj", "local").expect("poller があります");
    let (poller, _) = registry.client("local", "proj", Lane::Poller, config);
    assert!(Arc::ptr_eq(&read.client, &poller), "書き込みレーンのハンドルを読み取りに使いません");
    assert_eq!(read.cwd, config().cwd);
    assert!(!read.rebuilt, "他レーンの置換では読取先の再構築を通知しません");
    assert_eq!(read.write_sequence, 1);
    registry.record_write("local", "proj");
    assert_eq!(read.write_sequence, 1, "取得済みの通し番号は取得後の書き込みでは変わりません");

    registry.client("local", "proj", Lane::Poller, || changed.clone());
    let read = registry.take_for_read("local", "proj", "local").expect("新しい poller があります");
    assert_eq!(read.cwd, changed.cwd);
    assert!(read.rebuilt);
    assert_eq!(read.write_sequence, 2);
    assert!(!registry.take_for_read("local", "proj", "local").expect("poller があります").rebuilt);
  }

  #[test]
  fn read_acquisition_requires_a_poller_lane() {
    let registry = McpClientRegistry::new();
    assert!(registry.take_for_read("local", "proj", "local").is_none());
    registry.client("local", "proj", Lane::Write, config);
    assert!(registry.take_for_read("local", "proj", "local").is_none(), "書き込みレーンだけでは読み取りできません");
  }

  /// プロジェクトの画面読み取りレーンの番号の一覧。レジストリの中身を直接観測します。
  fn screen_lane_indexes(registry: &McpClientRegistry, server_id: &str, project: &str) -> Vec<u32> {
    let inner = registry.inner.lock().expect("takt-mcp レジストリのロックが壊れています");
    inner
      .clients
      .keys()
      .filter_map(|(owner_server, owner_project, lane_key)| match lane_key {
        | LaneKey::Screen(index) if owner_server.as_str() == server_id && owner_project.as_str() == project => {
          Some(*index)
        },
        | _ => None,
      })
      .collect()
  }

  #[test]
  fn adding_a_screen_lane_does_not_mark_the_project_rebuilt() {
    let registry = McpClientRegistry::new();
    registry.ensure_lanes("local", "proj", ScreenReadLanes::clamped(1), config);
    assert!(
      !registry.take_for_read("local", "proj", "local").expect("poller があります").rebuilt,
      "初回のそろえでは作り直しとして記録しません"
    );
    registry.ensure_lanes("local", "proj", ScreenReadLanes::clamped(2), config);
    assert!(
      !registry.take_for_read("local", "proj", "local").expect("poller があります").rebuilt,
      "画面レーンの増設だけでは差分基準をリセットしません"
    );
  }

  #[test]
  fn replacing_clients_with_a_different_config_marks_the_project_rebuilt() {
    let registry = McpClientRegistry::new();
    registry.ensure_lanes("local", "proj", ScreenReadLanes::clamped(1), config);
    assert!(
      !registry.take_for_read("local", "proj", "local").expect("poller があります").rebuilt,
      "同じ設定のそろえでは作り直しとして記録しません"
    );
    let mut changed = config();
    changed.command = "node-changed".to_owned();
    registry.ensure_lanes("local", "proj", ScreenReadLanes::clamped(1), move || changed.clone());
    assert!(
      registry.take_for_read("local", "proj", "local").expect("poller があります").rebuilt,
      "設定が変わって置き換えた場合は作り直しとして記録します"
    );
  }

  #[test]
  fn reducing_screen_lanes_removes_surplus_clients() {
    let registry = McpClientRegistry::new();
    registry.ensure_lanes("local", "proj", ScreenReadLanes::clamped(3), config);
    assert_eq!(screen_lane_indexes(&registry, "local", "proj"), vec![0, 1, 2], "3本の画面レーンがそろいます");
    registry.ensure_lanes("local", "proj", ScreenReadLanes::clamped(1), config);
    assert_eq!(screen_lane_indexes(&registry, "local", "proj"), vec![0], "本数を超えた画面レーンは外します");
  }
}

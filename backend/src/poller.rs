//! 定期読み取りの調整。
//!
//! 各サイクルの前に設定ファイルを再読み込みし、変更後の間隔・閾値・プロジェクト一覧に従います。
//! 1プロジェクトの読み取り失敗はそのプロジェクトだけ遅延中（Delayed、接続中）とし、
//! 他のプロジェクトは止めません。
//! 連続失敗回数と遅延時間の両方を満たす最初の失敗で応答なし（Unresponsive）へ移し、
//! その間は起点を更新しません。 読み取りのたびに前回の状態との差分から出来事を検出し、SQLite
//! へ記録します。

use std::{
  collections::{BTreeMap, BTreeSet},
  path::PathBuf,
  sync::Arc,
  time::Duration,
};

use chrono::{DateTime, Utc};
use tokio::sync::{broadcast, watch};

use crate::{
  activity::ActivityStore,
  config::{load_config, Config, Project, ServerMeta, Thresholds, DEFAULT_INTERVAL_SECONDS},
  events::detect,
  mcp::{client_config, McpClientRegistry, ToolError, ToolSet},
  model::{
    ConnectionState, ConnectionTransition, EventKind, PendingDecisionFilter, PendingDecisionType, ProjectSnapshot,
  },
  store::StateStore,
};

/// 定期読み取りを1サイクル実行するための調整役。
pub struct Poller {
  config_path:    PathBuf,
  activity:       Arc<ActivityStore>,
  store:          Arc<StateStore>,
  registry:       McpClientRegistry,
  previous:       BTreeMap<String, ProjectSnapshot>,
  /// 最後に出来事の記録へ成功した接続状態。接続断・復帰の再検出を防ぐ比較基準。
  notified:       BTreeMap<String, ConnectionState>,
  /// 記録済みの切断から基準化したプロジェクトについて、その切断を記録した時刻。
  /// 復帰の出来事へ切断していた時間を入れるとき、状態ストアの起点が再起動で
  /// 消えている代わりに、この時刻から数えます。
  seeded_outages: BTreeMap<String, DateTime<Utc>>,
  /// 記録済みの接続の出来事への照会がまだ成功していないプロジェクト。
  /// 照会が一時的に失敗したサイクルでは接続状態の比較基準を確定せず、
  /// 照会が成功するまでサイクルのたびに照会を試し直します。確定を保留している
  /// 間は、読み取りの成否で比較基準を進めません。
  seed_pending:   BTreeSet<String>,
  changes:        broadcast::Sender<()>,
  last_signature: Option<StoreSignature>,
}

/// 1サイクルで読み取った件数と、記録した出来事の件数、次の待機に使う間隔。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CycleReport {
  pub projects_read:   usize,
  pub projects_failed: usize,
  pub events_recorded: usize,
  /// このサイクルで解決した読み取り間隔。次のサイクル待機にそのまま使います。
  pub interval:        Duration,
}

impl Poller {
  /// 設定ファイルのパスと記録先から調整役を作る。
  ///
  /// `changes` は、状態が変わったサイクルの末尾で1回だけ通知を送るチャネルです。
  /// WatchState はこの通知を購読して、状態を組み立て直してストリームへ送ります。
  pub fn new(
    config_path: PathBuf,
    activity: Arc<ActivityStore>,
    store: Arc<StateStore>,
    changes: broadcast::Sender<()>,
    registry: McpClientRegistry,
  ) -> Self {
    Poller {
      config_path,
      activity,
      store,
      registry,
      previous: BTreeMap::new(),
      notified: BTreeMap::new(),
      seeded_outages: BTreeMap::new(),
      seed_pending: BTreeSet::new(),
      changes,
      last_signature: None,
    }
  }

  /// 1サイクル分、全プロジェクトの状態を読み取って差し替え、出来事を記録する。
  ///
  /// 設定ファイルを再読み込みし、プロジェクト一覧の変化（追加・削除）を状態ストアと
  /// takt-mcp のクライアントのレジストリへ追随させます。読み取り間隔もこのサイクルで解決し、
  /// [`CycleReport::interval`] で返します。
  pub async fn read_cycle_once(&mut self) -> Result<CycleReport, crate::config::ConfigError> {
    let now = Utc::now();
    self.read_cycle_once_at(now).await
  }

  /// 指定した時刻を異常判定に使って1サイクル分を実行する。
  ///
  /// ログの無音と判断待ちの滞留は時刻から導出されるため、テストでは固定時刻を渡して
  /// 実時間に依存せず閾値の跨ぎを検証します。出来事の記録時刻と `last_read_at` は
  /// 従来どおり読み取り時点の実時刻です。
  pub async fn read_cycle_once_at(&mut self, now: DateTime<Utc>) -> Result<CycleReport, crate::config::ConfigError> {
    let config = load_config(&self.config_path)?;
    let before = match self.last_signature.take() {
      | Some(signature) => signature,
      | None => StoreSignature::capture(&self.store, now),
    };
    self.store.set_thresholds(config.thresholds);
    self.store.set_servers(config.server_meta.clone());
    self.store.set_projects(config.projects.clone());
    self.sync_clients(&config);
    let mut report = CycleReport {
      projects_read:   0,
      projects_failed: 0,
      events_recorded: 0,
      interval:        Duration::from_secs(config.interval_seconds),
    };
    // 記録に失敗したプロジェクトがあっても、残りのプロジェクトの読み取りと状態変化の通知を
    // 続けるため、最初の記録エラーだけを保持してサイクルの末尾で返します。
    let mut record_error: Option<crate::config::ConfigError> = None;
    for project in &config.projects {
      // 設定変更（読み取り側・書き込み RPC 経由の双方）でクライアントを作り直した
      // プロジェクトは、旧接続先の差分基準を新接続先へ投影しないよう読み取り前にリセットします。
      if self.registry.take_rebuilt_for(&project.server, &project.name) {
        self.reset_project_baseline(&project.name);
      }
      // 再起動や作り直しで比較基準が無いプロジェクトは、記録済みの接続の出来事から
      // 基準を作り、未対応の切断を取りこぼさないようにします。
      self.seed_connection_baseline(project);
      let read_at = Utc::now();
      let prev = self.previous.get(&project.name).cloned();
      match self.read_one(project, prev.as_ref()).await {
        | Ok(snapshot) => {
          // 接続断・復帰の比較基準は、最後に記録へ成功した状態（`notified`）に置きます。
          // 記録に失敗したサイクルでは基準を進めず、次サイクルで同じ差分を再検出します。
          let mut outage =
            self.store.success_transition(&project.name, read_at).and_then(|transition| transition.outage);
          if let Some(lost_at) = self.seeded_outages.get(&project.name) {
            // 記録済みの切断から基準化した場合は、状態ストアの起点ではなく記録の時刻から数えます。
            outage = Some(read_at - *lost_at);
          }
          let transition = self.notified_transition(&project.name, ConnectionState::Connected, outage);
          let events = detect(&project.server, &project.name, prev.as_ref(), &snapshot, transition, read_at);
          match self.record_events(&events) {
            | Ok(()) => {
              // 記録に成功したサイクルだけ、snapshot と差分基準を進めます。
              self.store.record_success(&project.name, snapshot.clone(), read_at);
              self.previous.insert(project.name.clone(), snapshot);
              // 記録済みの接続の出来事への照会がまだ成功していないプロジェクトは、
              // 空の出来事群の記録が成功しても接続状態の比較基準を確定しません。
              // 確定すると以後のサイクルで照会を試し直せず、未対応の切断に対応する
              // 復帰を取りこぼします。
              if !self.seed_pending.contains(&project.name) {
                self.notified.insert(project.name.clone(), ConnectionState::Connected);
                self.seeded_outages.remove(&project.name);
              }
              report.projects_read += 1;
              report.events_recorded += events.len();
            },
            | Err(error) => {
              record_error.get_or_insert(error);
            },
          }
        },
        | Err(error) => {
          // 接続状態は失敗のたびに進め、表示へ反映します。一方で接続断の出来事の比較基準は
          // 記録に成功するまで進めないため、記録に失敗した接続断は回復後に1回だけ記録されます。
          let failure = self.store.record_failure(&project.name, read_at);
          let next = failure.and_then(|transition| transition.next);
          // 比較基準がまだ無い場合は、状態ストア側の遷移前を比較基準に使います。基準が無いまま
          // 応答なしへ跨ぐと切断を記録できず、回復したときに復帰だけが残ってしまいます。
          // ただし、状態ストア側にも遷移前が無い初回接触は基準化のみとし、切断を出しません。
          // 記録済みの接続の出来事への照会がまだ成功していないプロジェクトは、
          // 未対応の切断が残っているか分からないため、接続の出来事を記録しません。
          let transition = if self.seed_pending.contains(&project.name) {
            None
          } else {
            next.and_then(|next| {
              self.notified_transition(&project.name, next, None).or_else(|| {
                failure.filter(|store| store.previous.is_some()).map(|store| ConnectionTransition {
                  previous: store.previous,
                  next:     Some(next),
                  outage:   None,
                })
              })
            })
          };
          let empty = ProjectSnapshot::empty();
          let next_snapshot = prev.as_ref().unwrap_or(&empty);
          let events = detect(&project.server, &project.name, prev.as_ref(), next_snapshot, transition, read_at);
          match self.record_events(&events) {
            | Ok(()) => {
              if let Some(next) = next {
                // 記録済みの切断から基準化したプロジェクトは、応答なしの基準を弱い状態で
                // 上書きしません。上書きすると、しきい値を跨いだときに記録済みの切断を
                // 二重に記録してしまいます。基準は読み取りの成功でだけ閉じます。
                let seeded_open = self.seeded_outages.contains_key(&project.name);
                if !self.seed_pending.contains(&project.name) && (!seeded_open || next == ConnectionState::Unresponsive)
                {
                  self.notified.insert(project.name.clone(), next);
                }
              }
              report.events_recorded += events.len();
            },
            | Err(record_failure) => {
              record_error.get_or_insert(record_failure);
            },
          }
          report.projects_failed += 1;
          tracing::warn!(project = %project.name, error = %error, "プロジェクトの読み取りに失敗しました。");
        },
      }
    }
    let after = StoreSignature::capture(&self.store, now);
    let changed = after != before;
    self.last_signature = Some(after);
    if changed {
      let _ = self.changes.send(());
    }
    match record_error {
      | Some(error) => Err(error),
      | None => Ok(report),
    }
  }

  fn record_events(&self, events: &[crate::model::ActivityEvent]) -> Result<(), crate::config::ConfigError> {
    self
      .activity
      .record_all(events)
      .map_err(|error| crate::config::ConfigError::Invalid(format!("出来事を記録できませんでした: {error}")))
  }

  /// 最後に記録へ成功した接続状態から今回の状態への遷移を返す。
  ///
  /// まだ一度も記録に成功していないプロジェクトは、比較基準が無いため `None`（基準化のみ）です。
  fn notified_transition(
    &self,
    project: &str,
    next: ConnectionState,
    outage: Option<chrono::Duration>,
  ) -> Option<ConnectionTransition> {
    self.notified.get(project).map(|previous| ConnectionTransition {
      previous: Some(*previous),
      next: Some(next),
      outage,
    })
  }

  /// 接続状態の比較基準がまだ無いプロジェクトについて、記録済みの接続の出来事から基準を作る。
  ///
  /// プロセスの再起動やクライアントの作り直しでメモリ上の基準が消えても、記録済みで
  /// まだ復帰が対応していない切断があれば「応答なし」として基準化します。これにより、
  /// 読み取りの失敗が続く間は切断を二重に記録せず、回復したときは切断へ対応する
  /// 復帰を記録できます。記録の読み取りに失敗した場合は「履歴なし」と区別して
  /// 基準の確定を保留し（`seed_pending`）、照会が成功するまでサイクルのたびに
  /// 照会を試し直します。保留の間は、読み取りの成否で比較基準を進めません。
  fn seed_connection_baseline(&mut self, project: &Project) {
    if self.notified.contains_key(&project.name) && !self.seed_pending.contains(&project.name) {
      return;
    }
    match self.activity.last_connection_event(&project.server, &project.name) {
      | Ok(last) => {
        self.seed_pending.remove(&project.name);
        if let Some((EventKind::ConnectionLost, lost_at)) = last {
          self.notified.insert(project.name.clone(), ConnectionState::Unresponsive);
          self.seeded_outages.insert(project.name.clone(), lost_at);
        }
      },
      | Err(_) => {
        self.seed_pending.insert(project.name.clone());
      },
    }
  }

  /// すべてのプロジェクトの子プロセスを終了する。
  pub async fn shutdown(&mut self) {
    self.registry.shutdown_all().await;
  }

  fn sync_clients(&self, config: &Config) {
    let wanted: Vec<(String, String)> =
      config.projects.iter().map(|project| (project.server.clone(), project.name.clone())).collect();
    self.registry.retain(&wanted);
    for project in &config.projects {
      let Some(server) = config.servers.get(&project.server) else {
        continue;
      };
      self.registry.client(&project.server, &project.name, || client_config(server, &project.cwd));
    }
  }

  /// クライアントを作り直したプロジェクトの差分基準と状態をリセットする。
  ///
  /// 旧接続先の snapshot と接続状態を新接続先の所属で投影しないよう、スロットを未読へ戻し、
  /// 前サイクルの差分基準も捨てます。新接続先の初回読み取りは基準化のみになります。
  fn reset_project_baseline(&mut self, project: &str) {
    self.previous.remove(project);
    self.notified.remove(project);
    self.seeded_outages.remove(project);
    self.seed_pending.remove(project);
    self.store.reset_project(project);
  }

  /// 1 プロジェクトを読み取る。補助読み取り（run・ゴール質問・通知・ゴールに属さない
  /// 問い合わせ）の道具エラーと輸送の期限切れは read_snapshot 内で前回値に縮退するため、
  /// ここで返る失敗はコア読み取り（ゴール・タスク）の失敗と、補助読み取りの期限切れ
  /// 以外の輸送エラー・解析エラーだけです。
  async fn read_one(
    &mut self,
    project: &Project,
    prev: Option<&ProjectSnapshot>,
  ) -> Result<ProjectSnapshot, ToolError> {
    let Some(client) = self.registry.get(&project.server, &project.name) else {
      return Err(ToolError::Parse(format!("プロジェクト {} の子プロセスがありません", project.name)));
    };
    let mut client = client.lock().await;
    ToolSet::new(&mut client, project.cwd.clone()).read_snapshot(prev).await
  }
}

/// サイクルの前後で観測可能な状態が変わったかを判定するための指紋。
///
/// WatchState
/// は「状態が変わるたび送る」ため、プロジェクトの並び・読み取り成否・状態・同時実行上限・閾値・
/// サーバーの
/// 表示情報に加えて、時刻から導出されるログの無音と判断待ちの滞留の真偽が変わったサイクルだけを
/// 変化とみなします。読めた時刻（last_read_at）は読み取りのたびに進むため、内容が同じサイクルを
/// 変化として扱わないよう指紋から除きます。
#[derive(Debug, PartialEq, Eq)]
struct StoreSignature {
  thresholds:        Thresholds,
  servers:           BTreeMap<String, ServerMeta>,
  projects:          Vec<ProjectSignature>,
  pending_decisions: Vec<PendingDecisionSignature>,
}

#[derive(Debug, PartialEq, Eq)]
struct ProjectSignature {
  name:             String,
  concurrency:      i32,
  connection_state: Option<ConnectionState>,
  snapshot:         Option<ProjectSnapshot>,
  silent_runs:      Vec<(String, bool)>,
}

#[derive(Debug, PartialEq, Eq)]
struct PendingDecisionSignature {
  decision_type: PendingDecisionType,
  project:       String,
  goal_id:       Option<String>,
  stale:         bool,
}

impl StoreSignature {
  fn capture(store: &StateStore, now: DateTime<Utc>) -> Self {
    let projects = store
      .project_views()
      .into_iter()
      .map(|view| {
        let silent_runs = view.runs.iter().map(|run| (run.run_slug.clone(), store.is_log_silent(run, now))).collect();
        ProjectSignature {
          name: view.name,
          concurrency: view.concurrency,
          connection_state: view.connection_state,
          snapshot: view.snapshot,
          silent_runs,
        }
      })
      .collect();
    let pending_decisions = store
      .pending_decisions(now, PendingDecisionFilter::All)
      .into_iter()
      .map(|decision| PendingDecisionSignature {
        decision_type: decision.decision_type,
        project:       decision.project,
        goal_id:       decision.goal_id,
        stale:         decision.stale,
      })
      .collect();
    StoreSignature { thresholds: store.thresholds(), servers: store.servers(), projects, pending_decisions }
  }
}

/// 読み取りサイクルを周期実行する。停止の合図が立つまで回ります。
///
/// 各サイクルの設定解決は 1 回で、read_cycle_once が返す間隔をそのまま次の待機に使います。
/// 設定読み込みに失敗したサイクルは既定の間隔で待ちます。待機中も停止要求を監視し、
/// 要求を受けたら間隔の満了を待たずにループを抜けて子プロセスを終了します。
pub async fn run_until_stopped(poller: &mut Poller, mut stop: watch::Receiver<bool>) {
  while !*stop.borrow() {
    let started = Utc::now();
    let interval = match poller.read_cycle_once().await {
      | Ok(report) => {
        tracing::debug!(
          projects_read = report.projects_read,
          projects_failed = report.projects_failed,
          events_recorded = report.events_recorded,
          "定期読み取りの1サイクルを終えました。"
        );
        report.interval
      },
      | Err(error) => {
        tracing::warn!(error = %error, "定期読み取りの1サイクルで設定を読み込めませんでした。");
        Duration::from_secs(DEFAULT_INTERVAL_SECONDS)
      },
    };
    tokio::select! {
      _ = sleep_until_next(started, interval) => {}
      _ = wait_for_stop(&mut stop) => break,
    }
  }
  poller.shutdown().await;
}

async fn wait_for_stop(stop: &mut watch::Receiver<bool>) {
  while !*stop.borrow() {
    if stop.changed().await.is_err() {
      return;
    }
  }
}

async fn sleep_until_next(started: DateTime<Utc>, interval: Duration) {
  let elapsed = Utc::now().signed_duration_since(started);
  let remaining = interval.as_millis().saturating_sub(elapsed.num_milliseconds().max(0) as u128);
  if remaining > 0 {
    tokio::time::sleep(Duration::from_millis(remaining as u64)).await;
  }
}

//! 定期読み取りの調整。
//!
//! 各サイクルの前に設定ファイルを再読み込みし、変更後の間隔・閾値・プロジェクト一覧に従います。
//! 1プロジェクトの読み取り失敗はそのプロジェクトだけ connected
//! 偽とし、他のプロジェクトを止めません。 読み取りのたびに前回の状態との差分から出来事を検出し、
//! SQLite へ記録します。

use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};

use chrono::{DateTime, Utc};
use tokio::sync::broadcast;

use crate::{
  activity::ActivityStore,
  config::{load_config, Config, Project, Thresholds, DEFAULT_INTERVAL_SECONDS},
  events::detect,
  mcp::{client_config, McpClient, ToolError, ToolSet},
  model::ProjectSnapshot,
  store::StateStore,
};

/// 定期読み取りを1サイクル実行するための調整役。
pub struct Poller {
  config_path:        PathBuf,
  activity:           Arc<ActivityStore>,
  store:              Arc<StateStore>,
  clients:            BTreeMap<String, McpClient>,
  previous:           BTreeMap<String, ProjectSnapshot>,
  previous_connected: BTreeMap<String, bool>,
  changes:            broadcast::Sender<()>,
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
  ) -> Self {
    Poller {
      config_path,
      activity,
      store,
      clients: BTreeMap::new(),
      previous: BTreeMap::new(),
      previous_connected: BTreeMap::new(),
      changes,
    }
  }

  /// 1サイクル分、全プロジェクトの状態を読み取って差し替え、出来事を記録する。
  ///
  /// 設定ファイルを再読み込みし、プロジェクト一覧の変化（追加・削除）を状態ストアと clients へ
  /// 追随させます。読み取り間隔もこのサイクルで解決し、[`CycleReport::interval`] で返します。
  pub async fn read_cycle_once(&mut self) -> Result<CycleReport, crate::config::ConfigError> {
    let config = load_config(&self.config_path)?;
    let before = StoreSignature::capture(&self.store);
    self.store.set_thresholds(config.thresholds);
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
      let read_at = Utc::now();
      let prev = self.previous.get(&project.name).cloned();
      let prev_connected = self.previous_connected.get(&project.name).copied().unwrap_or(false);
      match self.read_one(project, prev.as_ref()).await {
        | Ok(snapshot) => {
          let events = detect(&project.server, &project.name, prev.as_ref(), prev_connected, true, &snapshot, read_at);
          match self.record_events(&events) {
            | Ok(()) => {
              self.store.record_success(&project.name, snapshot.clone(), read_at);
              self.previous.insert(project.name.clone(), snapshot);
              self.previous_connected.insert(project.name.clone(), true);
              report.projects_read += 1;
              report.events_recorded += events.len();
            },
            | Err(error) => {
              record_error.get_or_insert(error);
            },
          }
        },
        | Err(error) => {
          if let Some(prev) = prev.as_ref() {
            let events = detect(&project.server, &project.name, Some(prev), prev_connected, false, prev, read_at);
            match self.record_events(&events) {
              | Ok(()) => report.events_recorded += events.len(),
              | Err(record_failure) => {
                record_error.get_or_insert(record_failure);
              },
            }
          }
          self.store.record_failure(&project.name);
          self.previous_connected.insert(project.name.clone(), false);
          report.projects_failed += 1;
          tracing::warn!(project = %project.name, error = %error, "プロジェクトの読み取りに失敗しました。");
        },
      }
    }
    if StoreSignature::capture(&self.store) != before {
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

  /// すべてのプロジェクトの子プロセスを終了する。
  pub async fn shutdown(&mut self) {
    for client in self.clients.values_mut() {
      client.shutdown().await;
    }
    self.clients.clear();
  }

  fn sync_clients(&mut self, config: &Config) {
    let wanted: Vec<&str> = config.projects.iter().map(|project| project.name.as_str()).collect();
    self.clients.retain(|project, _| wanted.contains(&project.as_str()));
    for project in &config.projects {
      let Some(server) = config.servers.get(&project.server) else {
        continue;
      };
      self.clients.entry(project.name.clone()).or_insert_with(|| McpClient::new(client_config(server, &project.cwd)));
    }
  }

  /// 1 プロジェクトを読み取る。補助読み取り（run・ゴール質問・通知・ゴールに属さない
  /// 問い合わせ）の道具エラーは read_snapshot 内で前回値に縮退するため、ここで返る失敗は
  /// コア読み取り（ゴール・タスク）の失敗と、補助読み取りの輸送エラー・解析エラーだけです。
  async fn read_one(
    &mut self,
    project: &Project,
    prev: Option<&ProjectSnapshot>,
  ) -> Result<ProjectSnapshot, ToolError> {
    let Some(client) = self.clients.get_mut(&project.name) else {
      return Err(ToolError::Parse(format!("プロジェクト {} の子プロセスがありません", project.name)));
    };
    ToolSet::new(client, project.cwd.clone()).read_snapshot(prev).await
  }
}

/// サイクルの前後で観測可能な状態が変わったかを判定するための指紋。
///
/// WatchState
/// は「状態が変わるたび送る」ため、プロジェクトの並び・読み取り成否・状態・閾値のいずれかが
/// 変わったサイクルだけを変化とみなします。読めた時刻（last_read_at）は読み取りのたびに進むため、
/// 内容が同じサイクルを変化として扱わないよう指紋から除きます。
#[derive(Debug, PartialEq, Eq)]
struct StoreSignature {
  thresholds: Thresholds,
  projects:   Vec<(String, bool, Option<ProjectSnapshot>)>,
}

impl StoreSignature {
  fn capture(store: &StateStore) -> Self {
    let projects = store.project_views().into_iter().map(|view| (view.name, view.connected, view.snapshot)).collect();
    StoreSignature { thresholds: store.thresholds(), projects }
  }
}

/// 読み取りサイクルを周期実行する。停止の合図が立つまで回ります。
///
/// 各サイクルの設定解決は 1 回で、read_cycle_once が返す間隔をそのまま次の待機に使います。
/// 設定読み込みに失敗したサイクルは既定の間隔で待ちます。
pub async fn run_until_stopped<F>(poller: &mut Poller, mut stop: F)
where
  F: FnMut() -> bool, {
  while !stop() {
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
    sleep_until_next(started, interval).await;
  }
  poller.shutdown().await;
}

async fn sleep_until_next(started: DateTime<Utc>, interval: Duration) {
  let elapsed = Utc::now().signed_duration_since(started);
  let remaining = interval.as_millis().saturating_sub(elapsed.num_milliseconds().max(0) as u128);
  if remaining > 0 {
    tokio::time::sleep(Duration::from_millis(remaining as u64)).await;
  }
}

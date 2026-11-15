//! 定期読み取りの調整。
//!
//! 各サイクルの前に設定ファイルを再読み込みし、変更後の間隔・閾値・プロジェクト一覧に従います。
//! 1プロジェクトの読み取り失敗はそのプロジェクトだけ connected
//! 偽とし、他のプロジェクトを止めません。 読み取りのたびに前回の状態との差分から出来事を検出し、
//! SQLite へ記録します。

use std::{
  collections::BTreeMap,
  path::PathBuf,
  sync::Arc,
  time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use tokio::sync::broadcast;

use crate::{
  activity::ActivityStore,
  config::{load_config, Config, Project, Thresholds, DEFAULT_INTERVAL_SECONDS},
  events::detect,
  mcp::{client_config, McpClient, ToolCallMetrics, ToolError, ToolSet},
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
    let started = Instant::now();
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
    let mut metrics = ToolCallMetrics::default();
    let mut handles = Vec::with_capacity(config.projects.len());
    for project in &config.projects {
      match self.clients.remove(&project.name) {
        | Some(mut client) => {
          let project = project.clone();
          let prev = self.previous.get(&project.name).cloned();
          handles.push(Some(tokio::spawn(async move {
            let read_at = Utc::now();
            let mut tools = ToolSet::new(&mut client, project.cwd.clone());
            let result = tools.read_snapshot(prev.as_ref()).await;
            let metrics = tools.take_metrics();
            (client, read_at, result, metrics)
          })));
        },
        | None => handles.push(None),
      }
    }
    for (project, handle) in config.projects.iter().zip(handles) {
      let prev = self.previous.get(&project.name).cloned();
      let prev_connected = self.previous_connected.get(&project.name).copied().unwrap_or(false);
      let Some(handle) = handle else {
        let error = ToolError::Parse(format!("プロジェクト {} の子プロセスがありません", project.name));
        self.record_failure(project, &error, prev.as_ref(), prev_connected, Utc::now(), &mut report)?;
        continue;
      };
      match handle.await {
        | Ok((client, read_at, result, project_metrics)) => {
          self.clients.insert(project.name.clone(), client);
          metrics.merge(project_metrics);
          match result {
            | Ok(snapshot) => {
              self.record_success(project, prev.as_ref(), prev_connected, read_at, snapshot, &mut report)?;
            },
            | Err(error) => {
              self.record_failure(project, &error, prev.as_ref(), prev_connected, read_at, &mut report)?;
            },
          }
        },
        | Err(error) => {
          let error =
            ToolError::Parse(format!("プロジェクト {} の読み取りタスクが異常終了しました: {error}", project.name));
          self.record_failure(project, &error, prev.as_ref(), prev_connected, Utc::now(), &mut report)?;
        },
      }
    }
    if StoreSignature::capture(&self.store) != before {
      let _ = self.changes.send(());
    }
    tracing::info!(
      cycle_ms = started.elapsed().as_millis(),
      projects = config.projects.len(),
      projects_read = report.projects_read,
      projects_failed = report.projects_failed,
      events_recorded = report.events_recorded,
      tool_calls = metrics.total_calls(),
      tool_failures = metrics.total_failures(),
      tool_timeouts = metrics.total_timeouts(),
      tools = %metrics,
      "定期読み取りの1サイクルを終えました。"
    );
    Ok(report)
  }

  fn record_events(&self, events: &[crate::model::ActivityEvent]) -> Result<(), crate::config::ConfigError> {
    for event in events {
      self
        .activity
        .record(event)
        .map_err(|error| crate::config::ConfigError::Invalid(format!("出来事を記録できませんでした: {error}")))?;
    }
    Ok(())
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

  /// 1 プロジェクトの読み取り成功を、設定順でストア・前回値・出来事へ反映する。
  fn record_success(
    &mut self,
    project: &Project,
    prev: Option<&ProjectSnapshot>,
    prev_connected: bool,
    read_at: DateTime<Utc>,
    snapshot: ProjectSnapshot,
    report: &mut CycleReport,
  ) -> Result<(), crate::config::ConfigError> {
    let events = detect(&project.server, &project.name, prev, prev_connected, true, &snapshot, read_at);
    self.store.record_success(&project.name, snapshot.clone(), read_at);
    self.previous.insert(project.name.clone(), snapshot);
    self.previous_connected.insert(project.name.clone(), true);
    self.record_events(&events)?;
    report.projects_read += 1;
    report.events_recorded += events.len();
    Ok(())
  }

  /// 1 プロジェクトの読み取り失敗を、設定順でストア・前回値・出来事へ反映する。
  fn record_failure(
    &mut self,
    project: &Project,
    error: &ToolError,
    prev: Option<&ProjectSnapshot>,
    prev_connected: bool,
    read_at: DateTime<Utc>,
    report: &mut CycleReport,
  ) -> Result<(), crate::config::ConfigError> {
    if let Some(prev) = prev {
      let events = detect(&project.server, &project.name, Some(prev), prev_connected, false, prev, read_at);
      self.record_events(&events)?;
      report.events_recorded += events.len();
    }
    self.store.record_failure(&project.name);
    self.previous_connected.insert(project.name.clone(), false);
    report.projects_failed += 1;
    tracing::warn!(project = %project.name, error = %error, "プロジェクトの読み取りに失敗しました。");
    Ok(())
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
      | Ok(report) => report.interval,
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

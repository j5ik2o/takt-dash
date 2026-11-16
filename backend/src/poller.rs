//! 定期読み取りの調整。
//!
//! 各サイクルの前に設定ファイルを再読み込みし、変更後の間隔・閾値・プロジェクト一覧に従います。
//! 1プロジェクトの読み取り失敗はそのプロジェクトだけ遅延中（Delayed、接続中）とし、
//! 他のプロジェクトは止めません。
//! 連続失敗回数と遅延時間の両方を満たす最初の失敗で応答なし（Unresponsive）へ移し、
//! その間は起点を更新しません。 読み取りのたびに前回の状態との差分から出来事を検出し、SQLite
//! へ記録します。

use std::{
  collections::BTreeMap,
  path::PathBuf,
  sync::Arc,
  time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use tokio::sync::{broadcast, watch};

use crate::{
  activity::ActivityStore,
  config::{load_config, Config, Project, ServerMeta, Thresholds, DEFAULT_INTERVAL_SECONDS},
  events::detect,
  mcp::{client_config, McpClientRegistry, ToolCallMetrics, ToolError, ToolSet},
  model::{ConnectionState, ConnectionTransition, PendingDecisionFilter, PendingDecisionType, ProjectSnapshot},
  run_progress::RunLogHandles,
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
  changes:        broadcast::Sender<()>,
  last_signature: Option<StoreSignature>,
  /// プロジェクトごとの run の集計と、その集計を作ったときの cwd。周期をまたいで持ち、
  /// run の本体ログを前回の続きから読むために使います。
  run_logs:       RunLogHandles<String>,
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
      changes,
      last_signature: None,
      run_logs: RunLogHandles::default(),
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
  ///
  /// プロジェクトごとの読み取りは子プロセスのクライアントを排他取得したうえで並列に走らせ、
  /// 設定順に結果を回収します。回収した道具呼び出しの集計はマージし、サイクルの末尾で
  /// 1行のログに出します。
  pub async fn read_cycle_once_at(&mut self, now: DateTime<Utc>) -> Result<CycleReport, crate::config::ConfigError> {
    let started = Instant::now();
    let config = load_config(&self.config_path)?;
    let before = match self.last_signature.take() {
      | Some(signature) => signature,
      | None => StoreSignature::capture(&self.store, now),
    };
    self.store.set_thresholds(config.thresholds);
    self.store.set_servers(config.server_meta.clone());
    self.store.set_projects(config.projects.clone());
    self.sync_clients(&config);
    self.run_logs.retain(|name| config.projects.iter().any(|project| &project.name == name));
    let mut report = CycleReport {
      projects_read:   0,
      projects_failed: 0,
      events_recorded: 0,
      interval:        Duration::from_secs(config.interval_seconds),
    };
    let mut metrics = ToolCallMetrics::default();
    // 記録に失敗したプロジェクトがあっても、残りのプロジェクトの読み取りと状態変化の通知を
    // 続けるため、最初の記録エラーだけを保持してサイクルの末尾で返します。
    let mut record_error: Option<crate::config::ConfigError> = None;
    let mut handles = Vec::with_capacity(config.projects.len());
    for project in &config.projects {
      // 設定変更（読み取り側・書き込み RPC 経由の双方）でクライアントを作り直した
      // プロジェクトは、旧接続先の差分基準を新接続先へ投影しないよう読み取り前にリセットします。
      if self.registry.take_rebuilt_for(&project.server, &project.name) {
        self.reset_project_baseline(&project.name);
      }
      let prev = self.previous.get(&project.name).cloned();
      let run_logs = self.run_logs.handle_for(&project.name, &project.cwd);
      match self.registry.get(&project.server, &project.name) {
        | Some(handle) => {
          let project = project.clone();
          handles.push(Some(tokio::spawn(async move {
            let read_at = Utc::now();
            let mut client = handle.lock().await;
            let mut tools = ToolSet::new(&mut client, project.cwd.clone());
            let result = tools.read_snapshot(prev.as_ref(), &run_logs).await;
            let metrics = tools.take_metrics();
            (read_at, result, metrics)
          })));
        },
        | None => handles.push(None),
      }
    }
    for (project, handle) in config.projects.iter().zip(handles) {
      let prev = self.previous.get(&project.name).cloned();
      let Some(handle) = handle else {
        let error = ToolError::Parse(format!("プロジェクト {} の子プロセスがありません", project.name));
        self.apply_failure(project, &error, prev.as_ref(), Utc::now(), &mut report, &mut record_error);
        continue;
      };
      match handle.await {
        | Ok((read_at, result, project_metrics)) => {
          metrics.merge(project_metrics);
          match result {
            | Ok(snapshot) => {
              // 接続断・復帰の比較基準は、最後に記録へ成功した状態（`notified`）に置きます。
              // 記録に失敗したサイクルでは基準を進めず、次サイクルで同じ差分を再検出します。
              let outage =
                self.store.success_transition(&project.name, read_at).and_then(|transition| transition.outage);
              let transition = self.notified_transition(&project.name, ConnectionState::Connected, outage);
              let events = detect(&project.server, &project.name, prev.as_ref(), &snapshot, transition, read_at);
              match self.record_events(&events) {
                | Ok(()) => {
                  // 記録に成功したサイクルだけ、snapshot と差分基準を進めます。
                  self.store.record_success(&project.name, snapshot.clone(), read_at);
                  self.previous.insert(project.name.clone(), snapshot);
                  self.notified.insert(project.name.clone(), ConnectionState::Connected);
                  report.projects_read += 1;
                  report.events_recorded += events.len();
                },
                | Err(error) => {
                  record_error.get_or_insert(error);
                },
              }
            },
            | Err(error) => {
              self.apply_failure(project, &error, prev.as_ref(), read_at, &mut report, &mut record_error);
            },
          }
        },
        | Err(error) => {
          let error =
            ToolError::Parse(format!("プロジェクト {} の読み取りタスクが異常終了しました: {error}", project.name));
          self.apply_failure(project, &error, prev.as_ref(), Utc::now(), &mut report, &mut record_error);
        },
      }
    }
    let after = StoreSignature::capture(&self.store, now);
    let changed = after != before;
    self.last_signature = Some(after);
    if changed {
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
    match record_error {
      | Some(error) => Err(error),
      | None => Ok(report),
    }
  }

  /// 1 プロジェクトの読み取り失敗を、設定順でストア・前回値・出来事へ反映する。
  ///
  /// 接続断の記録に失敗したサイクルは接続状態の比較基準を据え置き、次サイクルで同じ
  /// 差分を再検出します。前回の状態を持たない初回は再検出する差分がないため、現行どおり
  /// 接続状態を偽として記録します。
  fn apply_failure(
    &mut self,
    project: &Project,
    error: &ToolError,
    prev: Option<&ProjectSnapshot>,
    read_at: DateTime<Utc>,
    report: &mut CycleReport,
    record_error: &mut Option<crate::config::ConfigError>,
  ) {
    // 接続状態は失敗のたびに進め、表示へ反映します。一方で接続断の出来事の比較基準は
    // 記録に成功するまで進めないため、記録に失敗した接続断は回復後に1回だけ記録されます。
    let next = self.store.record_failure(&project.name, read_at).and_then(|transition| transition.next);
    let transition = next.and_then(|next| self.notified_transition(&project.name, next, None));
    let empty = ProjectSnapshot::empty();
    let next_snapshot = prev.unwrap_or(&empty);
    let events = detect(&project.server, &project.name, prev, next_snapshot, transition, read_at);
    match self.record_events(&events) {
      | Ok(()) => {
        if let Some(next) = next {
          self.notified.insert(project.name.clone(), next);
        }
        report.events_recorded += events.len();
      },
      | Err(record_failure) => {
        record_error.get_or_insert(record_failure);
      },
    }
    report.projects_failed += 1;
    tracing::warn!(project = %project.name, error = %error, "プロジェクトの読み取りに失敗しました。");
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
  /// 前サイクルの差分基準と run の集計も捨てます。新接続先の初回読み取りは基準化のみになります。
  fn reset_project_baseline(&mut self, project: &str) {
    self.previous.remove(project);
    self.run_logs.remove(project);
    self.notified.remove(project);
    self.store.reset_project(project);
  }
}

/// サイクルの前後で観測可能な状態が変わったかを判定するための指紋。
///
/// WatchState
/// は「状態が変わるたび送る」ため、プロジェクトの並び・読み取り成否・状態・閾値・サーバーの
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

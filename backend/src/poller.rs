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
  time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use tokio::sync::{broadcast, watch};

use crate::{
  activity::ActivityStore,
  config::{load_config, Config, HttpMcpServer, McpServer, Project, ServerMeta, Thresholds, DEFAULT_INTERVAL_SECONDS},
  domain::PullRequestNumber,
  events::{detect, detect_with_terminations},
  mcp::{client_config, HttpReadHandle, McpClientRegistry, ReadToolSet, ToolCallMetrics, ToolError},
  model::{
    ActivityEvent, ConnectionState, ConnectionTransition, EventKind, ExecutionStatus, GoalEntry, GoalStatus,
    PendingDecisionFilter, PendingDecisionType, ProjectSnapshot, TaskEntry, TerminalReads,
  },
  pull_request::{probe_pull_request_with, PullRequestProbe},
  run_progress::RunLogHandles,
  store::{is_human_merge_wait, FinishRequest, StateStore},
  termination::request_finish_goal,
};

/// 読み取り 1 回のタイムアウト。常駐サーバーとの往復に十分な余裕を持たせます。
const READ_TIMEOUT_SECONDS: u64 = 15;

/// 定期読み取りを1サイクル実行するための調整役。
pub struct Poller {
  config_path:     PathBuf,
  activity:        Arc<ActivityStore>,
  store:           Arc<StateStore>,
  registry:        McpClientRegistry,
  reads:           BTreeMap<String, ProjectRead>,
  previous:        BTreeMap<String, ProjectSnapshot>,
  /// 最後に出来事の記録へ成功した接続状態。接続断・復帰の再検出を防ぐ比較基準。
  notified:        BTreeMap<String, ConnectionState>,
  /// 記録済みの切断から基準化したプロジェクトについて、その切断を記録した時刻。
  /// 復帰の出来事へ切断していた時間を入れるとき、状態ストアの起点が再起動で
  /// 消えている代わりに、この時刻から数えます。
  seeded_outages:  BTreeMap<String, DateTime<Utc>>,
  /// 記録済みの接続の出来事への照会がまだ成功していないプロジェクト。
  /// 照会が一時的に失敗したサイクルでは接続状態の比較基準を確定せず、
  /// 照会が成功するまでサイクルのたびに照会を試し直します。確定を保留している
  /// 間は、読み取りの成否で比較基準を進めません。
  seed_pending:    BTreeSet<String>,
  /// 照会の保留中に状態ストアが応答なしへ跨いだプロジェクトについて、その跨ぎの
  /// 遷移と時刻。保留中は接続の出来事を記録しないため、このままでは跨ぎが失われ、
  /// 照会が成功した後に比較基準だけが応答なしとして確定して、回復したときに
  /// 切断の無い復帰だけが残ります。照会が成功した時点で、履歴に未対応の切断が
  /// 無ければこの切断を1件だけ記録して調整します（`seed_connection_baseline`）。
  deferred_lost:   BTreeMap<String, (DateTime<Utc>, ConnectionTransition)>,
  /// 検出したが記録に失敗した接続断の出来事。比較基準（`notified`）は記録に成功するまで
  /// 進めないため、読み取りの失敗が続く間は同じ差分を再検出できます。しかし記録より先に
  /// 読み取りが回復すると、基準も今回の状態も接続中になり、差分からは接続断を再検出
  /// できません。検出した出来事そのものをここへ残し、記録に成功するまで周期のたびに
  /// 同じ1件を記録し直します。
  unrecorded_lost: BTreeMap<String, ActivityEvent>,
  changes:         broadcast::Sender<()>,
  last_signature:  Option<StoreSignature>,
  #[cfg(test)]
  read_barrier:    Option<rebuild_tests::ReadBarrier>,
  /// プロジェクトごとの run の集計と、その集計を作ったときの cwd。周期をまたいで持ち、
  /// run の本体ログを前回の続きから読むために使います。
  run_logs:        RunLogHandles<String>,
  /// PR の状態確認に使う gh の実行ファイルのパス。本番は PATH 上の `gh`、テストは偽の gh を
  /// 指します（[`Poller::with_gh_program`]）。
  gh_program:      PathBuf,
}

/// 1 プロジェクトの読み取りハンドルと、そのハンドルを作った接続先。接続先が変わったら
/// ハンドルを作り直し、セッションと差分基準をリセットします。
struct ProjectRead {
  server: HttpMcpServer,
  cwd:    PathBuf,
  handle: Arc<tokio::sync::Mutex<HttpReadHandle>>,
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
      reads: BTreeMap::new(),
      previous: BTreeMap::new(),
      notified: BTreeMap::new(),
      seeded_outages: BTreeMap::new(),
      seed_pending: BTreeSet::new(),
      deferred_lost: BTreeMap::new(),
      unrecorded_lost: BTreeMap::new(),
      changes,
      last_signature: None,
      #[cfg(test)]
      read_barrier: None,
      run_logs: RunLogHandles::default(),
      gh_program: PathBuf::from("gh"),
    }
  }

  /// PR の状態確認に使う gh の実行ファイルを差し替える（テスト用）。
  ///
  /// 本番は PATH 上の `gh` を使います。テストは偽の gh を指して、各状態と各失敗を
  /// 決定的に再現します。
  pub fn with_gh_program(mut self, program: PathBuf) -> Self {
    self.gh_program = program;
    self
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
    for previous in self.store.project_views() {
      if !config.projects.iter().any(|project| project.name == previous.name && project.server == previous.server_id) {
        self.reset_project_baseline(&previous.name);
      }
    }
    self.store.set_projects(config.projects.clone());
    #[cfg(test)]
    self.pause_read(rebuild_tests::ReadBoundary::AfterProjectsPublished).await;
    self.sync_clients(&config);
    self.sync_reads(&config);
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
      #[cfg(test)]
      self.pause_read(rebuild_tests::ReadBoundary::BeforeAcquisition).await;
      let acquired = self.registry.take_for_read(&project.server, &project.name, &project.write_server);
      // 他レーンの置換ではなく、実際の読取先の変更に合わせて比較基準を捨てます。
      if acquired.as_ref().is_some_and(|read| read.rebuilt) {
        self.reset_project_baseline(&project.name);
      }
      // 書き込みの通し番号は、読取先の取得と同じ時点で固定します。取得後に書き込みが
      // 成功した周期の読み取り結果は、書き込み前の状態を映すため採用しません。
      // HTTP 読み取りのプロジェクトはレジストリに読取先が無いため、その場で読みます。
      let write_sequence_at_start = match &acquired {
        | Some(read) => read.write_sequence,
        | None => self.registry.write_sequence(&project.write_server, &project.name),
      };
      #[cfg(test)]
      self.pause_read(rebuild_tests::ReadBoundary::AfterAcquisition).await;
      // 再起動や作り直しで比較基準が無いプロジェクトは、記録済みの接続の出来事から
      // 基準を作り、未対応の切断を取りこぼさないようにします。
      self.seed_connection_baseline(project);
      self.forget_answered_terminal_questions(&project.name);
      let prev = self.previous.get(&project.name).cloned();
      let http_read = self.reads.get(&project.name).map(|read| (read.handle.clone(), read.cwd.clone()));
      if let Some((handle, cwd)) = http_read {
        // 読み取りが HTTP 接続先のプロジェクトは、常駐サーバーのハンドルから読みます。
        // 書き込みの通し番号は書き込み先（`write_server`）のものを使います。
        let run_logs = self.run_logs.handle_for(&project.name, &cwd);
        let task = tokio::spawn(async move {
          let read_at = Utc::now();
          let mut handle = handle.lock().await;
          let mut tools = ReadToolSet::new(&mut *handle, cwd);
          let result = tools.read_snapshot(prev.as_ref(), &run_logs).await;
          let metrics = tools.take_metrics();
          (read_at, result, metrics)
        });
        handles.push(Some((task, write_sequence_at_start)));
        continue;
      }
      match acquired {
        | Some(read) => {
          let run_logs = self.run_logs.handle_for(&project.name, &read.cwd);
          let cwd = read.cwd;
          let handle = read.client;
          let task = tokio::spawn(async move {
            let read_at = Utc::now();
            let mut client = handle.lock().await;
            let mut tools = ReadToolSet::new(&mut *client, cwd);
            let result = tools.read_snapshot(prev.as_ref(), &run_logs).await;
            let metrics = tools.take_metrics();
            (read_at, result, metrics)
          });
          handles.push(Some((task, write_sequence_at_start)));
        },
        | None => handles.push(None),
      }
    }
    // このサイクルで読み取りを採用したプロジェクトと、採用時点の書き込み通し番号。
    // 終了依頼は、採用したプロジェクトについてだけ、通し番号が変わっていない条件で
    // 行います。
    let mut adopted_sequences: BTreeMap<String, u64> = BTreeMap::new();
    for (project, handle) in config.projects.iter().zip(handles) {
      let prev = self.previous.get(&project.name).cloned();
      let Some((handle, write_sequence_at_start)) = handle else {
        let error = ToolError::Parse(format!("プロジェクト {} の子プロセスがありません", project.name));
        self.apply_failure(project, &error, prev.as_ref(), Utc::now(), &mut report, &mut record_error);
        continue;
      };
      match handle.await {
        | Ok((read_at, result, project_metrics)) => {
          metrics.merge(project_metrics);
          match result {
            | Ok(snapshot) => {
              #[cfg(test)]
              self.pause_read(rebuild_tests::ReadBoundary::BeforeApply).await;
              // 書き込みゲートを保持したまま、古さの確認と反映を行います。書き込み側は
              // 書き込み RPC の発行前から通し番号の記録まで同じゲートを保持するため、
              // 確認と反映の間に書き込みが割り込むことはありません。
              let gate_server = &project.write_server;
              let write_gate = self.registry.write_gate(gate_server, &project.name);
              let _write_gate = write_gate.lock().await;
              // 読み取りの開始後に書き込みが成功していたら、この結果は書き込み前の状態を
              // 映している可能性があるため、store へ反映せず、差分基準（previous）も進めず、
              // 出来事の検出も行わずに捨てます。出来事の検出は、書き込み後の状態を読む
              // 次のサイクルで行われます。接続状態もこの結果では進めません。
              let write_sequence = self.registry.write_sequence(gate_server, &project.name);
              if read_result_is_stale(write_sequence_at_start, write_sequence) {
                tracing::info!(
                  project = %project.name,
                  write_sequence_at_start,
                  write_sequence,
                  "読み取りの開始後に書き込みがあったため、この読み取り結果を捨てます。"
                );
                continue;
              }
              // 接続断・復帰の比較基準は、最後に記録へ成功した状態（`notified`）に置きます。
              // 記録に失敗したサイクルでは基準を進めず、次サイクルで同じ差分を再検出します。
              let mut outage =
                self.store.success_transition(&project.name, read_at).and_then(|transition| transition.outage);
              if let Some(lost_at) = self.seeded_outages.get(&project.name) {
                // 記録済みの切断から基準化した場合は、
                // 状態ストアの起点ではなく記録の時刻から数えます。
                outage = Some(read_at - *lost_at);
              }
              let mut transition = self.notified_transition(&project.name, ConnectionState::Connected, outage);
              // 記録に失敗した接続断を保持している場合は、その接続断からの復帰として扱います。
              // 接続断を先頭に置いて同じトランザクションで記録するため、接続断だけ、または
              // 復帰だけが残ることはありません。
              let unrecorded_lost = self.unrecorded_lost.get(&project.name).cloned();
              if let Some(lost) = &unrecorded_lost {
                transition = Some(ConnectionTransition {
                  previous: Some(ConnectionState::Unresponsive),
                  next:     Some(ConnectionState::Connected),
                  outage:   Some(read_at - lost.occurred_at),
                });
              }
              // 判断待ちの表示補正のため、この snapshot のマージ待ちゴールの PR 状態を
              // 確認して記録します。出来事の検出と判断待ちの投影は、この記録を使って判定
              // します。確認は書き込みゲートの保持中に行い、確認と反映の間に書き込みが
              // 割り込まないようにします。
              self.refresh_pull_request_states(project, &snapshot).await;
              let terminations = self.store.terminations(&project.name);
              let mut events = detect_with_terminations(
                &project.server,
                &project.name,
                prev.as_ref(),
                &snapshot,
                transition,
                terminations.as_ref(),
                read_at,
              );
              if let Some(lost) = unrecorded_lost {
                events.insert(0, lost);
              }
              match self.record_events(&events) {
                | Ok(()) => {
                  self.unrecorded_lost.remove(&project.name);
                  // 記録に成功したサイクルだけ、snapshot と差分基準を進めます。
                  self.store.record_success(&project.name, snapshot.clone(), read_at);
                  self.previous.insert(project.name.clone(), snapshot);
                  adopted_sequences.insert(project.name.clone(), write_sequence);
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
    // snapshot の反映の後に、PR が閉じていると確認できた中止済みゴールへ、manager 経由の
    // 終了依頼を投函します。表示補正のための PR 状態の確認は、反映の前に済ませています。
    self.auto_terminate_closed_goals(&config, &adopted_sequences).await;
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

  /// マージ待ちゴールの PR 状態を確認し、判断待ちの表示補正の記録へ反映する。
  ///
  /// 対象は、人のマージ待ちの条件を満たし、PR 番号を持つゴールです。自動終了の対象である
  /// 中止済みゴールに限りません。確認できた状態だけを記録し、確認できない場合は記録を
  /// 変えません（誤って隠しません）。終了依頼は行いません。依頼済みのゴールは、状態が
  /// 変わっても表示へ影響しないため確認を省きます。この確認は、採用した snapshot への
  /// 出来事の検出より前に行います。今回の確認結果が、出来事の検出と判断待ちの投影の
  /// 両方へ同じ周期で効くためです。
  async fn refresh_pull_request_states(&mut self, project: &Project, snapshot: &ProjectSnapshot) {
    let merge_tasks: Vec<&TaskEntry> = snapshot.tasks.iter().filter(|task| task.merge_task).collect();
    for goal in &snapshot.goals {
      if goal.status != GoalStatus::AwaitingMerge {
        continue;
      }
      let Some(pull_request) = goal.pull_request_number else {
        continue;
      };
      if !is_human_merge_wait(goal.status, &goal.goal_id, &merge_tasks) {
        continue;
      }
      let already_requested = self
        .store
        .termination(&project.name, &goal.goal_id)
        .is_some_and(|record| record.finish_request == FinishRequest::Requested);
      if already_requested {
        continue;
      }
      let probe = probe_pull_request_with(&self.gh_program, &project.cwd, pull_request).await;
      if let PullRequestProbe::Confirmed(state) = probe {
        self.store.record_pull_request(&project.name, goal.goal_id.clone(), Some(state));
      }
    }
  }

  /// 中止済みのマージ待ちゴールへ、manager 経由の終了依頼を投函する。
  ///
  /// 対象は、PR が閉じていると確認済みで、まだ依頼していない（または前回の依頼が失敗した）
  /// ゴールに限ります。依頼は、このサイクルで読み取りを採用したプロジェクトについてだけ
  /// 行います。設定 `autoTerminateClosedGoals` が false の場合と、manager 用エントリが
  /// 未設定の場合は依頼を行いません。表示補正は [`Poller::refresh_pull_request_states`] が
  /// 設定に依存せず担います。
  async fn auto_terminate_closed_goals(&mut self, config: &Config, adopted_sequences: &BTreeMap<String, u64>) {
    if !config.auto_terminate_closed_goals {
      return;
    }
    let Some(manager_server) = config.manager_server.clone() else {
      return;
    };
    let Some(McpServer::Stdio(manager)) = config.servers.get(&manager_server) else {
      return;
    };
    let manager = manager.clone();
    for project in &config.projects {
      let Some(adopted_sequence) = adopted_sequences.get(&project.name) else {
        continue;
      };
      let Some(snapshot) = self.previous.get(&project.name).cloned() else {
        continue;
      };
      let merge_tasks: Vec<&TaskEntry> = snapshot.tasks.iter().filter(|task| task.merge_task).collect();
      for goal in &snapshot.goals {
        let Some(pull_request) = goal.pull_request_number else {
          continue;
        };
        if !self.is_auto_termination_candidate(project, goal, &merge_tasks) {
          continue;
        }
        self.terminate_goal_if_closed(project, &manager_server, &manager, goal, pull_request, *adopted_sequence).await;
      }
    }
  }

  /// 自動終了の対象条件（awaiting_merge・aborted・人のマージ待ち・未依頼）を満たすか。
  fn is_auto_termination_candidate(&self, project: &Project, goal: &GoalEntry, merge_tasks: &[&TaskEntry]) -> bool {
    if goal.status != GoalStatus::AwaitingMerge || goal.execution_status != ExecutionStatus::Aborted {
      return false;
    }
    if !is_human_merge_wait(goal.status, &goal.goal_id, merge_tasks) {
      return false;
    }
    match self.store.termination(&project.name, &goal.goal_id) {
      // 依頼済み（Requested）は再投函しません。失敗（Failed）はこのサイクルでは再試行せず、
      // 次の読み取りサイクルで再評価します。表示は PR の確認状態で決まります。
      | Some(record) => {
        record.finish_request == FinishRequest::NotRequested
          || matches!(record.finish_request, FinishRequest::Failed(_))
      },
      | None => true,
    }
  }

  /// 1ゴールの終了依頼を、書き込みとの競合を避けて投函する。
  ///
  /// 書き込みゲートを保持し、読み取りの採用時点の書き込み通し番号が変わっていないことを
  /// 確かめます。変わっている場合は、マージ承認などの書き込みが割り込んだ可能性があるため、
  /// このサイクルでは依頼せず、次のサイクルの読み取りで再評価します。PR の状態も依頼の
  /// 直前に確認し直し、閉じていると再確認できた場合だけ、manager 用エントリへ
  /// `takt_finish_goal` を1回投函します。manager のクライアントは manager 用エントリの
  /// ID で登録します。読み取り先の ID で登録すると、manager 用エントリを設定から外しても
  /// 子プロセスが残り続けます。
  async fn terminate_goal_if_closed(
    &mut self,
    project: &Project,
    manager_server: &str,
    manager: &crate::config::StdioMcpServer,
    goal: &GoalEntry,
    pull_request: PullRequestNumber,
    adopted_sequence: u64,
  ) {
    let write_gate = self.registry.write_gate(&project.write_server, &project.name);
    let _write_gate = write_gate.lock().await;
    if self.registry.write_sequence(&project.write_server, &project.name) != adopted_sequence {
      tracing::info!(
        project = %project.name,
        goal_id = %goal.goal_id,
        "読み取りの採用後に書き込みがあったため、このサイクルの終了依頼を見送ります。"
      );
      return;
    }
    let probe = probe_pull_request_with(&self.gh_program, &project.cwd, pull_request).await;
    let confirmed = match probe {
      | PullRequestProbe::Confirmed(state) => {
        self.store.record_pull_request(&project.name, goal.goal_id.clone(), Some(state));
        if !state.is_closed() {
          return;
        }
        state
      },
      | PullRequestProbe::Unknown => return,
    };
    // 終了の理由は、確認したプルリクエストの状態を含む文にします（takt_finish_goal の必須入力）。
    let reason = format!(
      "プルリクエスト #{pull_request} の状態は gh の確認で {confirmed} でした。ゴールは中止済みでマージ待ちのまま残っているため、自動終了を依頼します。"
    );
    let handle = self
      .registry
      .client(manager_server, &project.name, crate::mcp::Lane::Manager, || client_config(manager, &project.cwd));
    let finish_request = {
      let mut client = handle.0.lock().await;
      request_finish_goal(&mut client, &project.cwd, &goal.goal_id, &reason).await
    };
    let record = match finish_request {
      | Ok(()) => FinishRequest::Requested,
      | Err(reason) => {
        tracing::warn!(
          project = %project.name,
          goal_id = %goal.goal_id,
          reason = %reason,
          "終了依頼 (takt_finish_goal) に失敗しました。プルリクエストが閉じているためゴールは判断待ちに出しません。依頼は次の読み取りサイクルで再試行します。"
        );
        FinishRequest::Failed(reason)
      },
    };
    self.store.record_finish_request(&project.name, goal.goal_id.clone(), record);
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
    let failure = self.store.record_failure(&project.name, read_at);
    let next = failure.and_then(|transition| transition.next);
    // 比較基準がまだ無い場合は、状態ストア側の遷移前を比較基準に使います。基準が無いまま
    // 応答なしへ跨ぐと切断を記録できず、回復したときに復帰だけが残ってしまいます。
    // ただし、状態ストア側にも遷移前が無い初回接触は基準化のみとし、切断を出しません。
    // 記録済みの接続の出来事への照会がまだ成功していないプロジェクトは、
    // 未対応の切断が残っているか分からないため、接続の出来事を記録しません。
    let transition = if self.seed_pending.contains(&project.name) {
      // 保留中は接続の出来事を記録しませんが、応答なしへ跨いだ事実は失わずに
      // 残し、照会が成功した時点で調整します。初回接触（遷移前が無い場合）は
      // 通常の経路と同じく基準化のみとし、対象にしません。
      if let Some(store_transition) = failure {
        let crossed = store_transition.previous.is_some()
          && store_transition.previous != Some(ConnectionState::Unresponsive)
          && store_transition.next == Some(ConnectionState::Unresponsive);
        if crossed {
          self.deferred_lost.entry(project.name.clone()).or_insert((read_at, store_transition));
        }
      }
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
    let next_snapshot = prev.unwrap_or(&empty);
    let mut events = detect(&project.server, &project.name, prev, next_snapshot, transition, read_at);
    // 記録に失敗した接続断を保持している場合は、再検出した接続断の代わりに保持している
    // 1件を記録し、最初に検出した時刻を保ちます。
    if let Some(lost) = self.unrecorded_lost.get(&project.name) {
      events.retain(|event| event.kind != EventKind::ConnectionLost);
      events.insert(0, lost.clone());
    }
    match self.record_events(&events) {
      | Ok(()) => {
        self.unrecorded_lost.remove(&project.name);
        if let Some(next) = next {
          // 記録済みの切断から基準化したプロジェクトは、応答なしの基準を弱い状態で
          // 上書きしません。上書きすると、しきい値を跨いだときに記録済みの切断を
          // 二重に記録してしまいます。基準は読み取りの成功でだけ閉じます。
          let seeded_open = self.seeded_outages.contains_key(&project.name);
          if !self.seed_pending.contains(&project.name) && (!seeded_open || next == ConnectionState::Unresponsive) {
            self.notified.insert(project.name.clone(), next);
          }
        }
        report.events_recorded += events.len();
      },
      | Err(record_failure) => {
        if let Some(lost) = events.into_iter().find(|event| event.kind == EventKind::ConnectionLost) {
          self.unrecorded_lost.entry(project.name.clone()).or_insert(lost);
        }
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

  /// 接続状態の比較基準がまだ無いプロジェクトについて、記録済みの接続の出来事から基準を作る。
  ///
  /// プロセスの再起動やクライアントの作り直しでメモリ上の基準が消えても、記録済みで
  /// まだ復帰が対応していない切断があれば「応答なし」として基準化します。これにより、
  /// 読み取りの失敗が続く間は切断を二重に記録せず、回復したときは切断へ対応する
  /// 復帰を記録できます。記録の読み取りに失敗した場合は「履歴なし」と区別して
  /// 基準の確定を保留し（`seed_pending`）、照会が成功するまでサイクルのたびに
  /// 照会を試し直します。保留の間は、読み取りの成否で比較基準を進めません。
  /// 保留の間に状態ストアが応答なしへ跨いだ場合は、その跨ぎを `deferred_lost` へ
  /// 残し、照会が成功した時点で、履歴に未対応の切断が無ければその切断を1件だけ
  /// 記録して調整します。
  fn seed_connection_baseline(&mut self, project: &Project) {
    if self.notified.contains_key(&project.name) && !self.seed_pending.contains(&project.name) {
      return;
    }
    match self.activity.last_connection_event(&project.server, &project.name) {
      | Ok(last) => {
        self.seed_pending.remove(&project.name);
        if let Some((EventKind::ConnectionLost, lost_at)) = last {
          // 履歴の未対応の切断が、保留中に跨いだ分も含めてこの障害の記録です。
          // 二重に記録しないよう、保留中の跨ぎは破棄します。
          self.deferred_lost.remove(&project.name);
          self.notified.insert(project.name.clone(), ConnectionState::Unresponsive);
          self.seeded_outages.insert(project.name.clone(), lost_at);
        } else if let Some((crossed_at, crossing)) = self.deferred_lost.get(&project.name).copied() {
          // 履歴に未対応の切断が無く、保留中に応答なしへ跨いでいた場合は、その
          // 切断をこの時点で1件だけ記録し、比較基準を応答なしとして確定します。
          // 記録に失敗した場合は保留へ戻し、次のサイクルで照会から試し直します。
          // 保留へ戻すことで、通常の経路が記録の無いまま基準だけを確定することも
          // 防ぎます。
          let empty = ProjectSnapshot::empty();
          let events = detect(&project.server, &project.name, None, &empty, Some(crossing), crossed_at);
          match self.record_events(&events) {
            | Ok(()) => {
              self.deferred_lost.remove(&project.name);
              self.notified.insert(project.name.clone(), ConnectionState::Unresponsive);
              self.seeded_outages.insert(project.name.clone(), crossed_at);
            },
            | Err(_) => {
              self.seed_pending.insert(project.name.clone());
            },
          }
        }
      },
      | Err(_) => {
        self.seed_pending.insert(project.name.clone());
      },
    }
  }

  /// すべてのプロジェクトの子プロセスを終了し、常駐 HTTP サーバーに作ったセッションを閉じる。
  pub async fn shutdown(&mut self) {
    self.registry.shutdown_all().await;
    for read in std::mem::take(&mut self.reads).into_values() {
      read.handle.lock().await.close().await;
    }
  }

  fn sync_clients(&self, config: &Config) {
    // レジストリは、読み取りと書き込みが使う stdio の子プロセスに加えて、書き込みゲートと
    // 書き込みの通し番号も（サーバー、プロジェクト）の組ごとに管理する。保持対象は
    // 接続方法によらず、設定にあるプロジェクトの読み取り先（`server`）と書き込み先
    // （`write_server`、未設定なら `server`）の組すべてにする。書き込み先が HTTP 接続先の
    // プロジェクトで保持対象から外すと、定期読み取りのたびに書き込みゲートと通し番号が
    // 消え、書き込み前の読み取り結果が書き込み後の状態を上書きする事故を防げなくなる。
    // 子プロセスの保持と用意は stdio の組だけに行う。読み取り先と書き込み先が
    // 異なる stdio エントリを指す場合は、双方へレーンを用意する。書き込み先だけを残すと、
    // 読み取りが「子プロセスがありません」で毎回失敗する。読み取りが HTTP 接続先の
    // プロジェクトでは、読み取り用の子プロセスは起動しない。
    let mut wanted = BTreeSet::new();
    for project in &config.projects {
      for server_id in [&project.server, &project.write_server] {
        wanted.insert((server_id.clone(), project.name.clone()));
      }
      // manager 用エントリは終了依頼の投函先で、プロジェクトごとに manager レーンの
      // 子プロセスを持ちます。対象プロジェクトのキーを保持対象に加えます。
      // 子プロセスの起動は終了依頼のときまで遅延させます。
      if let Some(manager_server) = &config.manager_server {
        if matches!(config.servers.get(manager_server), Some(McpServer::Stdio(_))) {
          wanted.insert((manager_server.clone(), project.name.clone()));
        }
      }
    }
    let http_servers = config
      .servers
      .iter()
      .filter(|(_, server)| matches!(server, McpServer::Http(_)))
      .map(|(id, _)| id.clone())
      .collect();
    self.registry.retain_configured(&wanted, http_servers);
    for project in &config.projects {
      let mut warmed: Vec<&String> = Vec::new();
      for server_id in [&project.server, &project.write_server] {
        if warmed.contains(&server_id) {
          continue;
        }
        warmed.push(server_id);
        let Some(McpServer::Stdio(server)) = config.servers.get(server_id) else {
          continue;
        };
        let (write_handle, write_created) =
          self
            .registry
            .ensure_lanes(server_id, &project.name, config.screen_read_lanes, || client_config(server, &project.cwd));
        // 書き込みレーンを温めるのは、読み取りも stdio のプロジェクトだけです。
        // 読み取りが HTTP のプロジェクトでは、読み取りの周期で子プロセスを起動しません。
        let reads_over_stdio = matches!(config.servers.get(&project.server), Some(McpServer::Stdio(_)));
        if write_created && server_id == &project.write_server && reads_over_stdio {
          // 書き込みレーンは起動時に温めます。最初の書き込みが利用者の操作の裏で
          // 子プロセスの起動と初期化を待たないようにするためです。温めは裏で進め、
          // サイクルの進行は止めません。失敗した場合は、最初の書き込みのときに
          // 通常の再試行の経路で扱います。
          let project_name = project.name.clone();
          tokio::spawn(async move {
            let mut client = write_handle.lock().await;
            if let Err(error) = client.warm_up().await {
              tracing::warn!(
                project = %project_name,
                error = %error,
                "書き込みレーンの子プロセスを温められませんでした。最初の書き込みで再試行します。"
              );
            }
          });
        }
      }
    }
  }

  /// HTTP 接続先のプロジェクトの読み取りハンドルを、設定に追随させる。
  ///
  /// 接続先（URL とトークン）とプロジェクトの cwd が変わらないプロジェクトはハンドルを
  /// 使い回します。どちらかが変わったプロジェクトはハンドルを作り直し、セッションと
  /// 差分基準をリセットします。HTTP から stdio へ切り替わってハンドルを失った
  /// プロジェクトも、旧接続先の差分基準を新しい読み取り元へ投影しないようリセット
  /// します。stdio 接続先のプロジェクトは読み取りハンドルを持ちません。
  fn sync_reads(&mut self, config: &Config) {
    let mut rebuilt = Vec::new();
    let mut next = BTreeMap::new();
    for project in &config.projects {
      let Some(McpServer::Http(server)) = config.servers.get(&project.server) else {
        continue;
      };
      match self.reads.remove(&project.name) {
        | Some(read) if &read.server == server && read.cwd == project.cwd => {
          next.insert(project.name.clone(), read);
        },
        | _ => {
          let handle = HttpReadHandle::new(
            McpServer::Http(server.clone()).connection(),
            Duration::from_secs(READ_TIMEOUT_SECONDS),
          );
          next.insert(project.name.clone(), ProjectRead {
            server: server.clone(),
            cwd:    project.cwd.clone(),
            handle: Arc::new(tokio::sync::Mutex::new(handle)),
          });
          rebuilt.push(project.name.clone());
        },
      }
    }
    let dropped: Vec<String> = self.reads.keys().filter(|name| !next.contains_key(*name)).cloned().collect();
    self.reads = next;
    for project in rebuilt.into_iter().chain(dropped) {
      self.reset_project_baseline(&project);
    }
  }

  /// 問い合わせへの回答が成功したゴールを、前回の snapshot の「終端で質問を読めた」ゴールから外す。
  ///
  /// 外したゴールは、状態が変わらない終端のゴールでもこの周期で質問を読み直します。読み直せれば
  /// 再び載り、次の周期からは従来どおり読み取りを省きます。
  fn forget_answered_terminal_questions(&mut self, project: &str) {
    let answered = self.store.take_invalidated_goal_questions(project);
    if answered.is_empty() {
      return;
    }
    if let Some(previous) = self.previous.get_mut(project) {
      previous.terminal_reads.questions.retain(|goal_id| !answered.contains(goal_id));
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
    self.seeded_outages.remove(project);
    self.seed_pending.remove(project);
    self.deferred_lost.remove(project);
    self.unrecorded_lost.remove(project);
    self.store.reset_project(project);
  }
}

/// サイクルの前後で観測可能な状態が変わったかを判定するための指紋。
///
/// WatchState
/// は「状態が変わるたび送る」ため、プロジェクトの並び・読み取り成否・状態・同時実行上限・閾値・
/// サーバーの
/// 表示情報に加えて、時刻から導出されるログの無音と判断待ちの滞留の真偽が変わったサイクルだけを
/// 変化とみなします。読めた時刻（last_read_at）は読み取りのたびに進むため、内容が同じサイクルを
/// 変化として扱わないよう指紋から除きます。終端のゴールで補助読み取りに成功したかの記録
/// （`terminal_reads`）も、次の周期で読み直すかを決めるための内部の記録で画面には出ないため、
/// 指紋から除きます。
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
  // 書き込み先は、サーバーの「書き込み専用」の印を決める。書き込み先だけが変わる
  // 設定変更でも画面の表示が変わるため、指紋に含める。
  write_server_id:  String,
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
          snapshot: view
            .snapshot
            .map(|snapshot| ProjectSnapshot { terminal_reads: TerminalReads::default(), ..snapshot }),
          silent_runs,
          write_server_id: view.write_server_id,
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

/// 読み取り結果が古いかどうかを、書き込みの通し番号で判定する。
///
/// 読み取り開始時点の番号より現在の番号が進んでいたら、読み取りの途中で書き込みが
/// 成功しており、結果は書き込み前の状態を映している可能性があるため古いと判定します。
fn read_result_is_stale(write_sequence_at_start: u64, write_sequence_now: u64) -> bool {
  write_sequence_now > write_sequence_at_start
}

#[cfg(test)]
mod tests {
  //! 書き込みの通し番号による古い読み取り結果の判定と、書き込みゲートによる
  //! 確認と反映の直列化を固定する。

  use super::{read_result_is_stale, McpClientRegistry};

  #[test]
  fn read_result_is_fresh_without_writes() {
    assert!(!read_result_is_stale(0, 0), "書き込みが無ければ結果は新しい");
    assert!(!read_result_is_stale(3, 3), "番号が同じなら結果は新しい");
  }

  #[test]
  fn read_result_is_stale_after_a_write() {
    assert!(read_result_is_stale(0, 1), "読み取り開始後に書き込みがあれば結果は古い");
    assert!(read_result_is_stale(3, 4), "番号が1つでも進んでいれば結果は古い");
  }

  /// 書き込み側がゲートを保持している間は poller の反映の段が進まず、書き込みの
  /// 完了後に読み取り結果が古いものとして捨てられることを、チャネルの合図だけで
  /// 同期して観測します（時間の長さには依存しません）。
  #[tokio::test]
  async fn poller_apply_waits_for_the_write_gate_and_discards_the_stale_result() {
    let registry = McpClientRegistry::new();
    // 読み取り開始時点の番号。この読み取り結果は書き込み前の状態を映しています。
    let sequence_at_start = registry.write_sequence("local", "proj");

    // 書き込み側は、ゲートを保持したまま合図があるまで通し番号を進めません
    // （gRPC の書き込み経路が、書き込み RPC の発行前から記録までゲートを保持する形）。
    let (held_tx, held_rx) = tokio::sync::oneshot::channel::<()>();
    let (proceed_tx, proceed_rx) = tokio::sync::oneshot::channel::<()>();
    let write_registry = registry.clone();
    let write_task = tokio::spawn(async move {
      let gate = write_registry.write_gate("local", "proj");
      let _guard = gate.lock().await;
      let _ = held_tx.send(());
      let _ = proceed_rx.await;
      write_registry.record_write("local", "proj");
    });
    held_rx.await.expect("書き込み側がゲートを保持します");

    // poller 側は、反映の段の入口で合図してからゲートを取得し、取得後に古さを判定します。
    let (entering_tx, entering_rx) = tokio::sync::oneshot::channel::<()>();
    let (decided_tx, mut decided_rx) = tokio::sync::oneshot::channel::<bool>();
    let apply_registry = registry.clone();
    let apply_task = tokio::spawn(async move {
      let gate = apply_registry.write_gate("local", "proj");
      let _ = entering_tx.send(());
      let _guard = gate.lock().await;
      let now = apply_registry.write_sequence("local", "proj");
      let _ = decided_tx.send(read_result_is_stale(sequence_at_start, now));
    });
    entering_rx.await.expect("poller 側が反映の段に入ります");
    // 判定の送信はゲートの取得の先にしか無いため、書き込み側がゲートを保持している
    // 間は決して届きません。届いていないことはタイミングではなく構造で決まります。
    assert!(
      matches!(decided_rx.try_recv(), Err(tokio::sync::oneshot::error::TryRecvError::Empty)),
      "書き込みが完了するまで、反映の段はゲートを取得できません"
    );

    // 書き込みを完了させます。通し番号が進んでからゲートが解放されます。
    let _ = proceed_tx.send(());
    write_task.await.expect("書き込み側のタスクが終わります");
    let stale = decided_rx.await.expect("反映の段が判定を返します");
    apply_task.await.expect("poller 側のタスクが終わります");
    assert!(stale, "書き込み前の状態を映す結果は、古いものとして捨てられます");
    assert_eq!(registry.write_sequence("local", "proj"), 1, "書き込みの通し番号は1つ進んでいます");
  }

  /// poller の反映の段がゲートを保持している間は、書き込み側の record_write が
  /// 進まないことを、チャネルの合図だけで同期して観測します。
  #[tokio::test]
  async fn record_write_waits_while_the_poller_apply_holds_the_gate() {
    let registry = McpClientRegistry::new();
    // poller 側が反映の段のゲートを保持します。
    let gate = registry.write_gate("local", "proj");
    let apply_guard = gate.lock().await;

    // 書き込み側は、入口で合図してからゲートを取得し、取得後に通し番号を進めます
    // （gRPC の書き込み経路と同じ順序）。
    let (entering_tx, entering_rx) = tokio::sync::oneshot::channel::<()>();
    let (recorded_tx, mut recorded_rx) = tokio::sync::oneshot::channel::<u64>();
    let write_registry = registry.clone();
    let write_task = tokio::spawn(async move {
      let gate = write_registry.write_gate("local", "proj");
      let _ = entering_tx.send(());
      let _guard = gate.lock().await;
      let sequence = write_registry.record_write("local", "proj");
      let _ = recorded_tx.send(sequence);
    });
    entering_rx.await.expect("書き込み側がゲートの取得へ入ります");
    // 記録の送信はゲートの取得の先にしか無いため、反映の段がゲートを保持している
    // 間は決して届きません。
    assert!(
      matches!(recorded_rx.try_recv(), Err(tokio::sync::oneshot::error::TryRecvError::Empty)),
      "反映が終わるまで、書き込みの記録は進みません"
    );
    assert_eq!(registry.write_sequence("local", "proj"), 0, "通し番号も進んでいません");

    // 反映が終わってゲートが解放されると、書き込みの記録が進みます。
    drop(apply_guard);
    let sequence = recorded_rx.await.expect("書き込みの記録が終わります");
    write_task.await.expect("書き込み側のタスクが終わります");
    assert_eq!(sequence, 1, "記録された通し番号は1です");
    assert_eq!(registry.write_sequence("local", "proj"), 1, "通し番号は1つ進んでいます");
  }

  /// 書き込みゲートはプロジェクト（とサーバー）ごとに独立しており、ある
  /// プロジェクトの保持が別のプロジェクトを待たせないことを観測します。
  #[tokio::test]
  async fn write_gates_are_independent_per_project() {
    let registry = McpClientRegistry::new();
    let gate = registry.write_gate("local", "proj");
    let _guard = gate.lock().await;
    let other_project = registry.write_gate("local", "other");
    assert!(other_project.try_lock().is_ok(), "別プロジェクトのゲートは保持中のゲートに影響されません");
    let other_server = registry.write_gate("remote", "proj");
    assert!(other_server.try_lock().is_ok(), "別サーバーのゲートも独立です");
  }
}

#[cfg(test)]
#[path = "poller_rebuild_tests.rs"]
mod rebuild_tests;

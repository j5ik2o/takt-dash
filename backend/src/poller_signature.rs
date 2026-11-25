//! サイクルの前後で観測可能な状態が変わったかを判定する指紋。
//!
//! [`crate::poller::Poller`] が周期ごとに取り直し、内容が同じサイクルを変化として扱わない
//! ようにするために使います。

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

use crate::{
  config::{ServerMeta, Thresholds},
  model::{ConnectionState, PendingDecisionFilter, PendingDecisionType, ProjectSnapshot, TerminalReads},
  store::StateStore,
};

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
pub(crate) struct StoreSignature {
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
  pub(crate) fn capture(store: &StateStore, now: DateTime<Utc>) -> Self {
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

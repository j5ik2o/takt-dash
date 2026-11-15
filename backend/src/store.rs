//! 集約した状態の保持と投影。
//!
//! プロジェクトごとに、直近に読めた状態（snapshot）と読めた時刻（last_read_at）を保持します。
//! 応答がない間は snapshot と last_read_at を保持し、connected だけを偽にします。
//! 判断待ち・滞留・ログの無音は正規状態から都度計算します。

use std::{
  collections::BTreeMap,
  sync::{Arc, Mutex},
};

use chrono::{DateTime, Duration, Utc};

use crate::{
  config::{Project, ServerMeta, Thresholds},
  model::{
    GoalStatus, PendingDecision, PendingDecisionFilter, PendingDecisionType, ProjectSnapshot, ProjectView, RunEntry,
    TaskEntry, TaskStatus,
  },
};

/// 1プロジェクト分の保持スロット。読み取り成功で更新し、失敗では connected だけを変えます。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectSlot {
  pub connected:    bool,
  pub last_read_at: Option<DateTime<Utc>>,
  pub snapshot:     Option<ProjectSnapshot>,
}

impl ProjectSlot {
  /// まだ一度も読めていないスロット。
  pub fn unread() -> Self {
    ProjectSlot { connected: false, last_read_at: None, snapshot: None }
  }
}

/// 集約した状態を保持する共有ストア。
#[derive(Debug, Clone)]
pub struct StateStore {
  inner: Arc<Mutex<StoreInner>>,
}

#[derive(Debug)]
struct StoreInner {
  servers:    BTreeMap<String, ServerMeta>,
  projects:   Vec<Project>,
  thresholds: Thresholds,
  slots:      BTreeMap<String, ProjectSlot>,
}

impl StateStore {
  /// サーバー・プロジェクト・閾値を持つストアを作る。各プロジェクトのスロットは未読で初期化します。
  pub fn new(servers: BTreeMap<String, ServerMeta>, projects: Vec<Project>, thresholds: Thresholds) -> Self {
    let mut slots = BTreeMap::new();
    for project in &projects {
      slots.insert(project.name.clone(), ProjectSlot::unread());
    }
    StateStore { inner: Arc::new(Mutex::new(StoreInner { servers, projects, thresholds, slots })) }
  }

  /// 読み取りが成功したプロジェクトの状態を新しい snapshot と読めた時刻で置き換える。
  pub fn record_success(&self, project: &str, snapshot: ProjectSnapshot, read_at: DateTime<Utc>) {
    let mut inner = self.inner.lock().expect("状態ストアのロックが壊れています");
    if let Some(slot) = inner.slots.get_mut(project) {
      slot.connected = true;
      slot.last_read_at = Some(read_at);
      slot.snapshot = Some(snapshot);
    }
  }

  /// 稼働中に再読み込みした設定の閾値を反映する。次の投影から新しい閾値に従う。
  pub fn set_thresholds(&self, thresholds: Thresholds) {
    let mut inner = self.inner.lock().expect("状態ストアのロックが壊れています");
    inner.thresholds = thresholds;
  }

  /// 稼働中に再読み込みした設定のプロジェクト一覧を反映する。
  ///
  /// 追加されたプロジェクトは未読スロットで投影に載り、削除されたプロジェクトはスロットごと
  /// 投影から消えます。既存プロジェクトのスロット（直近の snapshot と読めた時刻）は保持します。
  pub fn set_projects(&self, projects: Vec<Project>) {
    let mut inner = self.inner.lock().expect("状態ストアのロックが壊れています");
    let wanted: Vec<&str> = projects.iter().map(|project| project.name.as_str()).collect();
    inner.slots.retain(|name, _| wanted.contains(&name.as_str()));
    for project in &projects {
      inner.slots.entry(project.name.clone()).or_insert_with(ProjectSlot::unread);
    }
    inner.projects = projects;
  }

  /// 読み取りが失敗したプロジェクトを接続断として扱う。snapshot と last_read_at は保持する。
  pub fn record_failure(&self, project: &str) {
    let mut inner = self.inner.lock().expect("状態ストアのロックが壊れています");
    if let Some(slot) = inner.slots.get_mut(project) {
      slot.connected = false;
    }
  }

  /// 指定したプロジェクトのスロットを返す。
  pub fn slot(&self, project: &str) -> Option<ProjectSlot> {
    self.inner.lock().expect("状態ストアのロックが壊れています").slots.get(project).cloned()
  }

  /// プロジェクトの一覧と各状態を、設定の順で返す。
  pub fn project_views(&self) -> Vec<ProjectView> {
    let inner = self.inner.lock().expect("状態ストアのロックが壊れています");
    inner
      .projects
      .iter()
      .map(|project| {
        let slot = inner.slots.get(&project.name).cloned().unwrap_or_else(ProjectSlot::unread);
        let (goals, tasks, runs) = match &slot.snapshot {
          | Some(snapshot) => (snapshot.goals.clone(), snapshot.tasks.clone(), snapshot.runs.clone()),
          | None => (Vec::new(), Vec::new(), Vec::new()),
        };
        ProjectView {
          name: project.name.clone(),
          about: project.about.clone(),
          server_id: project.server.clone(),
          connected: slot.connected,
          last_read_at: slot.last_read_at,
          goals,
          tasks,
          runs,
          snapshot: slot.snapshot,
        }
      })
      .collect()
  }

  /// 読み取り対象のサーバーの表示情報を返す。
  pub fn servers(&self) -> BTreeMap<String, ServerMeta> {
    self.inner.lock().expect("状態ストアのロックが壊れています").servers.clone()
  }

  /// 異常判定の閾値を返す。稼働中の設定変更を反映した現在の値です。
  pub fn thresholds(&self) -> Thresholds {
    self.inner.lock().expect("状態ストアのロックが壊れています").thresholds
  }

  /// runSlug を持つ run のプロジェクトを探し、その投影を返す。
  ///
  /// GetRunDetail は読み先の cwd とサーバーを状態ストアの run
  /// から解決するため、この投影を使います。
  pub fn find_run_project(&self, run_slug: &str) -> Option<ProjectView> {
    self.project_views().into_iter().find(|view| view.runs.iter().any(|run| run.run_slug == run_slug))
  }

  /// 判断待ちの一覧を、`filter` の見方で組み立てる。
  pub fn pending_decisions(&self, now: DateTime<Utc>, filter: PendingDecisionFilter) -> Vec<PendingDecision> {
    let inner = self.inner.lock().expect("状態ストアのロックが壊れています");
    let stale_threshold = Duration::minutes(i64::from(inner.thresholds.pending_decision_stale_minutes));
    let mut decisions = Vec::new();
    for project in &inner.projects {
      let Some(slot) = inner.slots.get(&project.name) else {
        continue;
      };
      let Some(snapshot) = &slot.snapshot else {
        continue;
      };
      if matches!(filter, PendingDecisionFilter::All | PendingDecisionFilter::Inquiry) {
        decisions.extend(inquiry_decisions(project, snapshot, now, stale_threshold));
      }
      if matches!(filter, PendingDecisionFilter::All | PendingDecisionFilter::AwaitingMerge) {
        decisions.extend(merge_decisions(project, snapshot, now, stale_threshold));
      }
    }
    decisions
  }

  /// ログの無音の判定を、run ごとに返す。
  pub fn is_log_silent(&self, run: &RunEntry, now: DateTime<Utc>) -> bool {
    let inner = self.inner.lock().expect("状態ストアのロックが壊れています");
    is_log_silent(run, now, inner.thresholds.run_log_silent_minutes)
  }
}

fn is_log_silent(run: &RunEntry, now: DateTime<Utc>, silent_minutes: u32) -> bool {
  now.signed_duration_since(run.last_log_at) > Duration::minutes(i64::from(silent_minutes))
}

fn inquiry_decisions(
  project: &Project,
  snapshot: &ProjectSnapshot,
  now: DateTime<Utc>,
  stale_threshold: Duration,
) -> Vec<PendingDecision> {
  snapshot
    .inquiries
    .iter()
    .filter(|inquiry| !inquiry.answered)
    .map(|inquiry| {
      let wait = now.signed_duration_since(inquiry.created_at);
      PendingDecision {
        decision_type:       PendingDecisionType::Inquiry,
        server_id:           project.server.clone(),
        project:             project.name.clone(),
        goal_id:             inquiry.goal_id.clone(),
        inquiry_id:          Some(inquiry.inquiry_id.clone()),
        inquiry_kind:        Some(inquiry.kind),
        created_at:          inquiry.created_at,
        body:                inquiry.body.clone(),
        options:             inquiry.options.clone(),
        pull_request_number: None,
        pull_request_url:    None,
        stale:               wait >= stale_threshold,
      }
    })
    .collect()
}

fn merge_decisions(
  project: &Project,
  snapshot: &ProjectSnapshot,
  now: DateTime<Utc>,
  stale_threshold: Duration,
) -> Vec<PendingDecision> {
  let merge_tasks: Vec<&TaskEntry> = snapshot.tasks.iter().filter(|task| task.merge_task).collect();
  snapshot
    .goals
    .iter()
    .filter(|goal| is_human_merge_wait(goal.status, &goal.goal_id, &merge_tasks))
    .map(|goal| {
      let created_at = goal.awaiting_merge_at.unwrap_or(now);
      let wait = now.signed_duration_since(created_at);
      PendingDecision {
        decision_type: PendingDecisionType::AwaitingMerge,
        server_id: project.server.clone(),
        project: project.name.clone(),
        goal_id: Some(goal.goal_id.clone()),
        // マージ待ちは問い合わせではないため、問い合わせの ID と種類を持たない。
        inquiry_id: None,
        inquiry_kind: None,
        created_at,
        body: goal.objective.clone(),
        options: Vec::new(),
        pull_request_number: goal.pull_request_number,
        pull_request_url: goal.pull_request_url.clone(),
        stale: wait >= stale_threshold,
      }
    })
    .collect()
}

/// takt の activity.ts と同じ規則で、人のマージ待ちかどうかを判定する。
///
/// awaiting_merge で、ゴールの仕上げタスクに pending/running がなく、
/// 配列末尾の仕上げタスクの状態が failed/exceeded/pr_failed でもないときに人のマージ待ちです。
pub fn is_human_merge_wait(status: GoalStatus, goal_id: &str, merge_tasks: &[&TaskEntry]) -> bool {
  if status != GoalStatus::AwaitingMerge {
    return false;
  }
  let goal_merge_tasks: Vec<&TaskEntry> =
    merge_tasks.iter().copied().filter(|task| task.goal_id.as_deref() == Some(goal_id)).collect();
  if goal_merge_tasks.iter().any(|task| matches!(task.status, TaskStatus::Pending | TaskStatus::Running)) {
    return false;
  }
  let last_status = goal_merge_tasks.last().map(|task| task.status);
  !matches!(last_status, Some(TaskStatus::Failed) | Some(TaskStatus::Exceeded) | Some(TaskStatus::PrFailed))
}

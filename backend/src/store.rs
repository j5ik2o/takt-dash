//! 集約した状態の保持と投影。
//!
//! プロジェクトごとに、直近に読めた状態（snapshot）と読めた時刻（last_read_at）を保持します。
//! 1回の読み取り失敗は Delayed（接続中）とし、連続失敗回数と遅延時間の両方を満たす最初の失敗で
//! Unresponsive（応答なし）へ移します。Unresponsive が続く間は snapshot と last_read_at を保持し、
//! 起点も更新しません。判断待ち・滞留・ログの無音は正規状態から都度計算します。

use std::{
  collections::{BTreeMap, BTreeSet},
  sync::{Arc, Mutex},
};

use chrono::{DateTime, Duration, Utc};

use crate::{
  config::{Project, ServerMeta, Thresholds},
  domain::{GoalId, ProjectName, PullRequestState, RunSlug, ServerId},
  model::{
    ConnectionState, ConnectionTransition, GoalStatus, PendingDecision, PendingDecisionFilter, PendingDecisionType,
    ProjectSnapshot, ProjectView, RegistrationProposalEntry, RunEntry, TaskEntry, TaskStatus,
  },
};

/// 1プロジェクト分の保持スロット。読み取り成功で更新し、失敗では接続状態だけを変えます。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectSlot {
  /// まだ一度も読めていない場合は None。
  pub connection_state:     Option<ConnectionState>,
  /// 直近の読み取りに連続して失敗した回数。
  pub consecutive_failures: u32,
  /// 応答なしへ移った時刻。応答なしが続く間は更新しません。
  pub unresponsive_since:   Option<DateTime<Utc>>,
  pub last_read_at:         Option<DateTime<Utc>>,
  pub snapshot:             Option<ProjectSnapshot>,
}

impl ProjectSlot {
  /// まだ一度も読めていないスロット。
  pub fn unread() -> Self {
    ProjectSlot {
      connection_state:     None,
      consecutive_failures: 0,
      unresponsive_since:   None,
      last_read_at:         None,
      snapshot:             None,
    }
  }

  /// 接続中または遅延中なら真。応答なしと未読は偽です。
  pub fn is_connected(&self) -> bool {
    matches!(self.connection_state, Some(ConnectionState::Connected) | Some(ConnectionState::Delayed))
  }
}

/// 成功時の遷移を、状態を変えずに組み立てる。
fn success_transition_of(slot: &ProjectSlot, read_at: DateTime<Utc>) -> ConnectionTransition {
  let previous = slot.connection_state;
  let outage = if previous == Some(ConnectionState::Unresponsive) {
    slot.unresponsive_since.map(|since| read_at.signed_duration_since(since))
  } else {
    None
  };
  ConnectionTransition { previous, next: Some(ConnectionState::Connected), outage }
}

/// この失敗の後の接続状態を返す。`slot.consecutive_failures` は今回の失敗を含む回数です。
fn connection_state_on_failure(slot: &ProjectSlot, thresholds: Thresholds, now: DateTime<Utc>) -> ConnectionState {
  if slot.connection_state == Some(ConnectionState::Unresponsive) {
    return ConnectionState::Unresponsive;
  }
  let count_reached = slot.consecutive_failures >= thresholds.connection_failure_count;
  let delay = Duration::seconds(i64::from(thresholds.connection_delay_seconds));
  let delay_elapsed = slot.last_read_at.is_none_or(|last_read_at| now.signed_duration_since(last_read_at) >= delay);
  if count_reached && delay_elapsed {
    ConnectionState::Unresponsive
  } else {
    ConnectionState::Delayed
  }
}

/// ゴールごとの自動終了の記録。サイクルを跨いで保持し、判断待ちの射影が読みます。
///
/// PR の状態は確認できたときだけ持ち、確認できないときは `None` のままです。終了依頼の状態は
/// 依頼済みか、直前の依頼の失敗理由を持ちます。`set_projects` でプロジェクトが外れたら破棄します。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GoalTermination {
  /// gh で確認できた PR の状態。未確認は None。
  pub pull_request:   Option<PullRequestState>,
  /// manager への終了依頼の状態。
  pub finish_request: FinishRequest,
}

/// manager への終了依頼の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum FinishRequest {
  /// まだ依頼していない。
  #[default]
  NotRequested,
  /// 依頼済み。takt がゴールを終端にした後も、このプロジェクトの記録が削除される
  /// （[`StateStore::set_projects`]）まで判断待ちに出し続けません。
  Requested,
  /// 直近の依頼が失敗した。理由を保持して表示を維持します。失敗した依頼は次の読み取り
  /// サイクルで候補に戻して再評価します（同じサイクルでは再試行しません）。
  Failed(String),
}

impl GoalTermination {
  /// このゴールを判断待ちから外すか。PR が閉じているか、終了依頼を投函済みなら外します。
  ///
  /// 依頼が失敗しただけ（[`FinishRequest::Failed`]）では外しません。状態を確認できなかった
  /// 場合は誤って隠さず、従来どおり表示を維持します。
  pub fn suppresses_display(&self) -> bool {
    self.finish_request == FinishRequest::Requested || self.pull_request.is_some_and(PullRequestState::is_closed)
  }
}

/// 集約した状態を保持する共有ストア。
#[derive(Debug, Clone)]
pub struct StateStore {
  inner: Arc<Mutex<StoreInner>>,
}

#[derive(Debug)]
struct StoreInner {
  servers:             BTreeMap<String, ServerMeta>,
  projects:            Vec<Project>,
  thresholds:          Thresholds,
  slots:               BTreeMap<String, ProjectSlot>,
  /// 問い合わせへの回答が成功し、次の周期で質問を読み直すゴール（プロジェクトごと）。
  /// 状態が変わらない終端のゴールでも、ここに載ったゴールは質問の読み取りを省きません。
  questions_to_reread: BTreeMap<String, BTreeSet<GoalId>>,
  /// 自動終了のための、プロジェクトごとのゴール単位の記録（PR の状態と終了依頼）。
  terminations:        BTreeMap<String, BTreeMap<GoalId, GoalTermination>>,
}

impl StateStore {
  /// サーバー・プロジェクト・閾値を持つストアを作る。各プロジェクトのスロットは未読で初期化します。
  pub fn new(servers: BTreeMap<String, ServerMeta>, projects: Vec<Project>, thresholds: Thresholds) -> Self {
    let mut slots = BTreeMap::new();
    for project in &projects {
      slots.insert(project.name.clone(), ProjectSlot::unread());
    }
    StateStore {
      inner: Arc::new(Mutex::new(StoreInner {
        servers,
        projects,
        thresholds,
        slots,
        questions_to_reread: BTreeMap::new(),
        terminations: BTreeMap::new(),
      })),
    }
  }

  /// 読み取りが成功したときの接続状態の遷移を、状態を変えずに返す。
  ///
  /// `record_success` で確定する前の遷移を、出来事の検出に使います。出来事の記録に失敗した
  /// サイクルでは `record_success` を呼ばず、差分基準を進めません。
  pub fn success_transition(&self, project: &str, read_at: DateTime<Utc>) -> Option<ConnectionTransition> {
    let inner = self.inner.lock().expect("状態ストアのロックが壊れています");
    let slot = inner.slots.get(project)?;
    Some(success_transition_of(slot, read_at))
  }

  /// 読み取りが成功したプロジェクトの状態を新しい snapshot と読めた時刻で置き換える。
  ///
  /// 応答なしから戻った場合は、応答なしだった時間を `outage` に入れた遷移を返します。
  pub fn record_success(
    &self,
    project: &str,
    snapshot: ProjectSnapshot,
    read_at: DateTime<Utc>,
  ) -> Option<ConnectionTransition> {
    let mut inner = self.inner.lock().expect("状態ストアのロックが壊れています");
    let slot = inner.slots.get_mut(project)?;
    let transition = success_transition_of(slot, read_at);
    slot.connection_state = Some(ConnectionState::Connected);
    slot.consecutive_failures = 0;
    slot.unresponsive_since = None;
    slot.last_read_at = Some(read_at);
    slot.snapshot = Some(snapshot);
    Some(transition)
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
    inner.questions_to_reread.retain(|name, _| wanted.contains(&name.as_str()));
    inner.terminations.retain(|name, _| wanted.contains(&name.as_str()));
    for project in &projects {
      inner.slots.entry(project.name.clone()).or_insert_with(ProjectSlot::unread);
    }
    inner.projects = projects;
  }

  /// 問い合わせへの回答が成功したゴールを、次の周期で質問を読み直すゴールとして記録する。
  ///
  /// 状態が変わらない終端のゴールは質問の読み取りを省き、前回の問い合わせを引き継ぎます。回答の後も
  /// 省くと、回答済みになった問い合わせが判断待ちに残るため、回答したゴールだけを読み直させます。
  pub fn invalidate_goal_questions(&self, project: &str, goal_id: GoalId) {
    let mut inner = self.inner.lock().expect("状態ストアのロックが壊れています");
    inner.questions_to_reread.entry(project.to_owned()).or_default().insert(goal_id);
  }

  /// 次の周期で質問を読み直すゴールを取り出し、記録から消す。
  pub fn take_invalidated_goal_questions(&self, project: &str) -> BTreeSet<GoalId> {
    let mut inner = self.inner.lock().expect("状態ストアのロックが壊れています");
    inner.questions_to_reread.remove(project).unwrap_or_default()
  }

  /// プロジェクトのゴールについて、PR の状態の確認結果を記録する。
  pub fn record_pull_request(&self, project: &str, goal_id: GoalId, pull_request: Option<PullRequestState>) {
    let mut inner = self.inner.lock().expect("状態ストアのロックが壊れています");
    let record = inner.terminations.entry(project.to_owned()).or_default().entry(goal_id).or_default();
    if let Some(state) = pull_request {
      record.pull_request = Some(state);
    }
  }

  /// プロジェクトのゴールについて、終了依頼の状態を記録する。
  pub fn record_finish_request(&self, project: &str, goal_id: GoalId, finish_request: FinishRequest) {
    let mut inner = self.inner.lock().expect("状態ストアのロックが壊れています");
    let record = inner.terminations.entry(project.to_owned()).or_default().entry(goal_id).or_default();
    record.finish_request = finish_request;
  }

  /// プロジェクトのゴールの自動終了の記録を返す。まだ記録が無ければ None。
  pub fn termination(&self, project: &str, goal_id: &GoalId) -> Option<GoalTermination> {
    self
      .inner
      .lock()
      .expect("状態ストアのロックが壊れています")
      .terminations
      .get(project)
      .and_then(|records| records.get(goal_id))
      .cloned()
  }

  /// プロジェクトの自動終了の記録をまとめて返す。出来事の検出が判断待ちと同じ規則を
  /// 共有するために使います。記録が無ければ None。
  pub fn terminations(&self, project: &str) -> Option<BTreeMap<GoalId, GoalTermination>> {
    self.inner.lock().expect("状態ストアのロックが壊れています").terminations.get(project).cloned()
  }

  /// 稼働中に再読み込みした設定のサーバー表示情報を反映する。
  ///
  /// 追加・改名・削除をすべて置き換え、次の投影から新しい表示情報に従います。
  pub fn set_servers(&self, servers: BTreeMap<String, ServerMeta>) {
    let mut inner = self.inner.lock().expect("状態ストアのロックが壊れています");
    inner.servers = servers;
  }

  /// 接続先の設定が変わったプロジェクトのスロットを未読へ戻す。
  ///
  /// 旧接続先の snapshot・読めた時刻・接続状態を新接続先へ持ち越さないために使います。プロジェクト
  /// の一覧とスロットの位置は保持し、内容だけを未読へ戻します。
  pub fn reset_project(&self, project: &str) {
    let mut inner = self.inner.lock().expect("状態ストアのロックが壊れています");
    if let Some(slot) = inner.slots.get_mut(project) {
      *slot = ProjectSlot::unread();
    }
  }

  /// 読み取りが失敗したプロジェクトの接続状態を進める。snapshot と last_read_at は保持する。
  ///
  /// 連続失敗回数がしきい値以上で、最後に読めてから遅延の秒数以上経過したとき（未読なら回数だけ）に
  /// 応答なしへ移ります。応答なしが続く間は起点を更新しません。
  pub fn record_failure(&self, project: &str, now: DateTime<Utc>) -> Option<ConnectionTransition> {
    let mut inner = self.inner.lock().expect("状態ストアのロックが壊れています");
    let thresholds = inner.thresholds;
    let slot = inner.slots.get_mut(project)?;
    let previous = slot.connection_state;
    slot.consecutive_failures = slot.consecutive_failures.saturating_add(1);
    let next = connection_state_on_failure(slot, thresholds, now);
    if next == ConnectionState::Unresponsive && previous != Some(ConnectionState::Unresponsive) {
      slot.unresponsive_since = Some(now);
    }
    slot.connection_state = Some(next);
    Some(ConnectionTransition { previous, next: Some(next), outage: None })
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
          write_server_id: project.write_server.clone(),
          concurrency: project.concurrency,
          connected: slot.is_connected(),
          connection_state: slot.connection_state,
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
  /// から解決するため、この投影を使います。実行中の run 一覧に無くても、
  /// 完了タスクが runSlug または sourceRunSlug を保持していれば解決できます。run が完了すると
  /// run 一覧から消えるため、詳細 URL を同じまま取得できるようにします。
  pub fn find_run_project(&self, run_slug: &RunSlug) -> Option<ProjectView> {
    self.project_views().into_iter().find(|view| {
      view.runs.iter().any(|run| run.run_slug == run_slug.as_str())
        || view.tasks.iter().any(|task| {
          task.run_slug.as_ref() == Some(run_slug) || task.source_run_slug.as_deref() == Some(run_slug.as_str())
        })
    })
  }

  /// goal_id を持つゴールのプロジェクトを探し、その投影を返す。
  ///
  /// GetGoalDetail は読み先の cwd とサーバーを状態ストアのゴール
  /// から解決するため、この投影を使います。
  pub fn find_goal_project(&self, goal_id: &str) -> Option<ProjectView> {
    self.project_views().into_iter().find(|view| view.goals.iter().any(|goal| goal.goal_id.as_str() == goal_id))
  }

  /// 判断待ちの一覧を、`filter` の見方で組み立てる。
  pub fn pending_decisions(&self, now: DateTime<Utc>, filter: PendingDecisionFilter) -> Vec<PendingDecision> {
    let inner = self.inner.lock().expect("状態ストアのロックが壊れています");
    let stale_threshold = Duration::minutes(i64::from(inner.thresholds.pending_decision_stale_minutes.get()));
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
        decisions.extend(merge_decisions(
          project,
          snapshot,
          inner.terminations.get(&project.name),
          now,
          stale_threshold,
        ));
      }
      if matches!(filter, PendingDecisionFilter::All) {
        decisions.extend(registration_decisions(project, snapshot, now));
      }
    }
    decisions
  }

  /// ゴール登録の承認待ちを、指定したサーバー・プロジェクトの分だけ返す。
  ///
  /// `pending_decisions` の承認待ち射影と同じデータ源（各プロジェクトの
  /// snapshot.proposals）を読みます。
  pub fn registration_proposals(&self, server_id: &ServerId, project: &ProjectName) -> Vec<RegistrationProposalEntry> {
    let inner = self.inner.lock().expect("状態ストアのロックが壊れています");
    inner
      .projects
      .iter()
      .filter(|entry| entry.server == server_id.as_str() && entry.name == project.as_str())
      .filter_map(|entry| inner.slots.get(&entry.name))
      .filter_map(|slot| slot.snapshot.as_ref())
      .flat_map(|snapshot| snapshot.proposals.iter().cloned())
      .collect()
  }

  /// ログの無音の判定を、run ごとに返す。
  pub fn is_log_silent(&self, run: &RunEntry, now: DateTime<Utc>) -> bool {
    self.is_log_silent_at(run.last_log_at, now)
  }

  /// 最後のログの時刻が分からない場合を「無音」と判定しない無音判定。最後のログの時刻が
  /// 無い run の詳細取得で使います。
  pub fn is_log_silent_at(&self, last_log_at: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    let inner = self.inner.lock().expect("状態ストアのロックが壊れています");
    is_log_silent_at(last_log_at, now, inner.thresholds.run_log_silent_minutes.get())
  }
}

fn is_log_silent_at(last_log_at: Option<DateTime<Utc>>, now: DateTime<Utc>, silent_minutes: i32) -> bool {
  let Some(last_log_at) = last_log_at else {
    return false;
  };
  now.signed_duration_since(last_log_at) > Duration::minutes(i64::from(silent_minutes))
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
        goal_id:             inquiry.goal_id.as_ref().map(|goal_id| goal_id.as_str().to_owned()),
        inquiry_kind:        Some(inquiry.kind),
        created_at:          inquiry.created_at,
        body:                inquiry.body.clone(),
        options:             inquiry.options.clone(),
        pull_request_number: None,
        pull_request_url:    None,
        stale:               wait >= stale_threshold,
        inquiry_id:          Some(inquiry.inquiry_id.clone()),
        revision:            None,
        merge_task_last:     None,
      }
    })
    .collect()
}

fn merge_decisions(
  project: &Project,
  snapshot: &ProjectSnapshot,
  terminations: Option<&BTreeMap<GoalId, GoalTermination>>,
  now: DateTime<Utc>,
  stale_threshold: Duration,
) -> Vec<PendingDecision> {
  let merge_tasks: Vec<&TaskEntry> = snapshot.tasks.iter().filter(|task| task.merge_task).collect();
  snapshot
    .goals
    .iter()
    .filter(|goal| {
      let termination = terminations.and_then(|records| records.get(&goal.goal_id));
      // 終了依頼済み、または PR が閉じていると確認できたゴールは判断待ちに出しません。
      if termination.is_some_and(GoalTermination::suppresses_display) {
        return false;
      }
      is_human_merge_wait(goal.status, &goal.goal_id, &merge_tasks)
    })
    .map(|goal| {
      let created_at = goal.awaiting_merge_at.unwrap_or(now);
      let wait = now.signed_duration_since(created_at);
      let merge_task_last =
        merge_tasks.iter().rev().find(|task| task.goal_id.as_ref() == Some(&goal.goal_id)).map(|task| task.status);
      PendingDecision {
        decision_type: PendingDecisionType::AwaitingMerge,
        server_id: project.server.clone(),
        project: project.name.clone(),
        goal_id: Some(goal.goal_id.as_str().to_owned()),
        // マージ待ちは問い合わせではないため、問い合わせの種類は持たない。
        inquiry_kind: None,
        created_at,
        body: goal.objective.clone(),
        options: Vec::new(),
        pull_request_number: goal.pull_request_number,
        pull_request_url: goal.pull_request_url.clone(),
        stale: wait >= stale_threshold,
        inquiry_id: None,
        revision: None,
        merge_task_last,
      }
    })
    .collect()
}

/// ゴール登録の承認待ちを判断待ちの一覧へ射影する。
///
/// 承認待ちは作成時刻を持たないため、滞留の判定はせず `created_at` は射影の時刻を、`stale`
/// は偽を入れます。本文には提案の要約を使い、承認・却下に使う revision を持ちます。
fn registration_decisions(project: &Project, snapshot: &ProjectSnapshot, now: DateTime<Utc>) -> Vec<PendingDecision> {
  snapshot
    .proposals
    .iter()
    .map(|proposal| PendingDecision {
      decision_type:       PendingDecisionType::GoalRegistration,
      server_id:           project.server.clone(),
      project:             project.name.clone(),
      goal_id:             None,
      inquiry_kind:        None,
      created_at:          now,
      body:                proposal.summary.clone(),
      options:             Vec::new(),
      pull_request_number: None,
      pull_request_url:    None,
      stale:               false,
      inquiry_id:          None,
      revision:            Some(proposal.revision),
      merge_task_last:     None,
    })
    .collect()
}

/// フロントエンドの isHumanMergeWaiting と同じ規則で、人のマージ待ちかどうかを判定する。
///
/// awaiting_merge で、ゴールの仕上げタスクに pending/running がなければ人のマージ待ちです。
/// 仕上げタスクが失敗系の状態（failed/exceeded/pr_failed）で終わった場合は、仕上げが止まって
/// 人の判断が要る状態として含めます（issue #231）。
pub fn is_human_merge_wait(status: GoalStatus, goal_id: &GoalId, merge_tasks: &[&TaskEntry]) -> bool {
  if status != GoalStatus::AwaitingMerge {
    return false;
  }
  let goal_merge_tasks: Vec<&TaskEntry> =
    merge_tasks.iter().copied().filter(|task| task.goal_id.as_ref() == Some(goal_id)).collect();
  !goal_merge_tasks.iter().any(|task| matches!(task.status, TaskStatus::Pending | TaskStatus::Running))
}

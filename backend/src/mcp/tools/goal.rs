//! ゴール・タスク・問い合わせ・通知・登録提案の応答解析。

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::{
  super::client::ToolCallError,
  parse::{domain_value, optional_string, parse_time, string_field, Shortened},
  ToolError,
};
use crate::{
  domain::{GoalId, InquiryId, ProjectName, PullRequestNumber, RunSlug, TaskName},
  model::{
    ExecutionStatus, GoalEntry, GoalQuestionEntry, GoalStatus, GoalWorkLogEntry, InquiryEntry, InquiryKind,
    NotificationEntry, PreviewText, ProjectSnapshot, TaskDetail, TaskEntry, TaskStatus,
  },
};

/// 前回 snapshot の問い合わせのうち、`matches` に合うものを解析済みの形で取り出す。
/// 道具エラー時に該当項目だけを引き継ぐために使います。前回が無ければ空です。
pub(super) fn prev_inquiries(
  prev: Option<&ProjectSnapshot>,
  matches: impl Fn(Option<&GoalId>) -> bool,
) -> Vec<ParsedInquiry> {
  let Some(prev) = prev else {
    return Vec::new();
  };
  prev
    .inquiries
    .iter()
    .filter(|entry| matches(entry.goal_id.as_ref()))
    .map(|entry| ParsedInquiry {
      inquiry_id: entry.inquiry_id.clone(),
      goal_id:    entry.goal_id.clone(),
      kind:       entry.kind,
      body:       entry.body.clone(),
      options:    entry.options.clone(),
      answered:   entry.answered,
      created_at: Some(entry.created_at),
    })
    .collect()
}

/// ゴールのマージ待ちの起点時刻を解像する。
///
/// `awaiting_merge` でないゴールは、通知から取れた起点をそのまま持ちます（無ければ `None`）。
/// `awaiting_merge` でも、そのゴールの仕上げタスクが未着手・実行中のあいだは、まだ人がマージを
/// 待っていないので起点を持ちません（#90）。takt は awaiting_merge 通知を仕上げタスクの投入より
/// 前に記録するため、通知だけで起点を決めると、仕上げタスクが長く走ったときに stale が早まります。
///
/// 仕上げタスクが終わった後は、通知の起点・そのゴールの仕上げタスクの `completed_at`・前回 snapshot
/// の同じゴールの起点のうち最も遅いものを起点にします。前回の起点を候補に入れるので、仕上げタスクが
/// 一覧から外れても起点は前へ戻らず、上流の値が同じなら読み取りのたびに変わりません。候補が無い
/// 初回観測では、その読み取りの観測時刻で起点を一度確定し、以降の読み取りで保持します（待ち時間が
/// 読み取りごとに現在時刻へリセットされて滞留しなくなることを避けます）。`completed_at`
/// の無い終わった 仕上げタスクがあり、前回の起点も無いときは、
/// 終わった時刻が分からないため観測時刻も候補に入れます。
pub(super) fn resolve_awaiting_merge_at(
  goal: &GoalEntry,
  notification_start: Option<DateTime<Utc>>,
  tasks: &[TaskEntry],
  prev: Option<&ProjectSnapshot>,
  observed_at: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
  if goal.status != GoalStatus::AwaitingMerge {
    return notification_start;
  }
  if tasks.iter().any(|task| task.is_active_merge_task_of(&goal.goal_id)) {
    return None;
  }
  let merge_tasks: Vec<&TaskEntry> =
    tasks.iter().filter(|task| task.merge_task && task.goal_id.as_ref() == Some(&goal.goal_id)).collect();
  let previous_start = prev
    .and_then(|snapshot| snapshot.goals.iter().find(|entry| entry.goal_id == goal.goal_id))
    .and_then(|entry| entry.awaiting_merge_at);
  let finished_at_unknown = merge_tasks.iter().any(|task| task.completed_at.is_none());
  let observed = (finished_at_unknown && previous_start.is_none()).then_some(observed_at);
  let candidates =
    merge_tasks.iter().map(|task| task.completed_at).chain([notification_start, previous_start, observed]);
  Some(candidates.flatten().max().unwrap_or(observed_at))
}

/// takt_get_goal が指定 path の項目を持たないときに返す道具エラーか。
///
/// 実 takt-mcp は存在しない項目を `Goal read failed: Selected field does not exist` の
/// 道具エラーで返すため、これだけを任意項目の空として扱います。ほかの道具エラーは
/// 失敗のまま伝播させます。
pub(super) fn is_missing_goal_field(error: &ToolCallError) -> bool {
  error.tool == "takt_get_goal" && error.message.trim_end().ends_with("Selected field does not exist")
}

pub(super) fn parse_goal_status(value: Option<&Value>) -> Result<GoalStatus, ToolError> {
  match value.and_then(Value::as_str) {
    | Some("created") => Ok(GoalStatus::Created),
    | Some("awaiting_merge") => Ok(GoalStatus::AwaitingMerge),
    | Some("completed") => Ok(GoalStatus::Completed),
    | other => Err(ToolError::Parse(format!("未知のゴールの状態です: {other:?}"))),
  }
}

pub(super) fn parse_goal_execution_status(value: Option<&Value>) -> Result<ExecutionStatus, ToolError> {
  match value.and_then(Value::as_str) {
    | Some("active") => Ok(ExecutionStatus::Active),
    | Some("paused") => Ok(ExecutionStatus::Paused),
    | Some("aborted") => Ok(ExecutionStatus::Aborted),
    | other => Err(ToolError::Parse(format!("未知のゴールの実行状態です: {other:?}"))),
  }
}

fn parse_task_status(value: Option<&Value>) -> Result<TaskStatus, ToolError> {
  match value.and_then(Value::as_str) {
    | Some("pending") => Ok(TaskStatus::Pending),
    | Some("running") => Ok(TaskStatus::Running),
    | Some("completed") => Ok(TaskStatus::Completed),
    | Some("failed") => Ok(TaskStatus::Failed),
    | Some("exceeded") => Ok(TaskStatus::Exceeded),
    | Some("pr_failed") => Ok(TaskStatus::PrFailed),
    | other => Err(ToolError::Parse(format!("未知のタスクの状態です: {other:?}"))),
  }
}

/// 一覧の要素の目的を、takt-mcp の印とともに読む。
///
/// takt-mcp の一覧は目的を 256 バイトで切った preview にし、要素の `omissions.objective.totalBytes`
/// に元の文のバイト数を入れます。返した preview より元の文が長ければ、切ってあります。ページが
/// 応答の上限で目的を短くしたとき（`goals[].objective`）も切ってあります。`totalBytes` が無い
/// 応答（古い takt-mcp）は、ページの印だけで決めます。文の形からは推定しません。
pub(super) fn parse_goal_objective(value: &Value, shortened: &Shortened) -> PreviewText {
  let mut objective = shortened.preview(value, "objective");
  let total_bytes = value
    .get("omissions")
    .and_then(|omissions| omissions.get("objective"))
    .and_then(|objective| objective.get("totalBytes"))
    .and_then(Value::as_u64);
  if total_bytes.is_some_and(|total| total > objective.text.len() as u64) {
    objective.truncated = true;
  }
  objective
}

pub(super) fn parse_goal(value: &Value, shortened: &Shortened) -> Result<GoalEntry, ToolError> {
  let goal_id = domain_value("ゴール ID", GoalId::try_from(string_field(value, "id")?.as_str()))?;
  let status = parse_goal_status(value.get("status"))?;
  let execution_status = parse_goal_execution_status(value.get("executionStatus"))?;
  let pull_request = value.get("pullRequest");
  let pull_request_number = match pull_request.and_then(|value| value.get("number")).and_then(Value::as_i64) {
    | Some(number) => Some(domain_value("プルリクエスト番号", PullRequestNumber::try_from(number))?),
    | None => None,
  };
  let pull_request_url = pull_request.and_then(|value| value.get("url")).and_then(Value::as_str).map(str::to_owned);
  Ok(GoalEntry {
    goal_id,
    status,
    execution_status,
    objective: parse_goal_objective(value, shortened),
    integration_branch: optional_string(value, "integrationBranch").unwrap_or_default(),
    pull_request_number,
    pull_request_url,
    awaiting_merge_at: None,
    paused_reason: optional_string(value, "pausedReason"),
  })
}

pub(super) fn parse_task(value: &Value) -> Result<TaskEntry, ToolError> {
  let status = parse_task_status(value.get("status"))?;
  let started_at = parse_time(value.get("startedAt"))?;
  let completed_at = parse_time(value.get("completedAt"))?;
  let name = domain_value("タスク名", TaskName::try_from(string_field(value, "name")?.as_str()))?;
  let goal_id = match optional_string(value, "goalId") {
    | Some(text) if text.is_empty() => None,
    | Some(text) => Some(domain_value("ゴール ID", GoalId::try_from(text.as_str()))?),
    | None => None,
  };
  let run_slug = match optional_string(value, "runSlug") {
    | Some(text) if text.is_empty() => None,
    | Some(text) => Some(domain_value("run slug", RunSlug::try_from(text.as_str()))?),
    | None => None,
  };
  Ok(TaskEntry {
    name,
    goal_id,
    status,
    started_at,
    completed_at,
    run_slug,
    source_run_slug: optional_string(value, "sourceRunSlug"),
    current_step: optional_string(value, "currentStep"),
    merge_task: value.get("mergeTask").and_then(Value::as_bool).unwrap_or(false),
  })
}

pub(super) fn parse_goal_work_log(value: &Value, shortened: &Shortened) -> Result<GoalWorkLogEntry, ToolError> {
  let recorded_at = parse_time(value.get("recordedAt"))?
    .ok_or_else(|| ToolError::Parse("作業記録の recordedAt がありません".to_owned()))?;
  Ok(GoalWorkLogEntry {
    work_log_id: string_field(value, "id")?,
    recorded_at,
    summary: shortened.display(optional_string(value, "summary").unwrap_or_default(), "summary"),
  })
}

pub(super) fn parse_goal_question(value: &Value, shortened: &Shortened) -> Result<GoalQuestionEntry, ToolError> {
  let answer = value.get("answer");
  let options = value
    .get("options")
    .and_then(Value::as_array)
    .map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect())
    .unwrap_or_default();
  Ok(GoalQuestionEntry {
    question_id: string_field(value, "id")?,
    kind: optional_string(value, "kind").unwrap_or_default(),
    status: optional_string(value, "status").unwrap_or_default(),
    body: shortened.display(optional_string(value, "body").unwrap_or_default(), "body"),
    options,
    answer_text: answer
      .and_then(|answer| optional_string(answer, "text"))
      .map(|text| shortened.display(text, "answer.text")),
    answer_decision: answer.and_then(|answer| optional_string(answer, "decision")),
    answer_report: answer.and_then(|answer| optional_string(answer, "report")),
    created_at: parse_time(value.get("createdAt"))?,
    answered_at: answer.map(|answer| parse_time(answer.get("answeredAt"))).transpose()?.flatten(),
  })
}

/// タスクに紐づくゴールから得る、タスク詳細に載せる補助情報。
#[derive(Debug, Default)]
pub(super) struct TaskLinkFields {
  pub(super) branch:           Option<String>,
  pub(super) pull_request_url: Option<String>,
}

pub(super) fn parse_task_detail(
  project: &ProjectName,
  value: &Value,
  links: TaskLinkFields,
) -> Result<TaskDetail, ToolError> {
  let task_name = domain_value("タスク名", TaskName::try_from(string_field(value, "name")?.as_str()))?;
  // goalId・runSlug・sourceRunSlug が空文字のタスクは未所属とし、一覧（parse_task）と
  // 同じく None で表します。空でない値は値オブジェクトの検証を通し、不正な値は
  // 解析エラーにします（parse_goal のプルリクエスト番号と同じ扱い）。
  let goal_id = match optional_string(value, "goalId") {
    | Some(text) if text.is_empty() => None,
    | Some(text) => Some(domain_value("ゴール ID", GoalId::try_from(text.as_str()))?),
    | None => None,
  };
  let run_slug = match optional_string(value, "runSlug") {
    | Some(text) if text.is_empty() => None,
    | Some(text) => Some(domain_value("run slug", RunSlug::try_from(text.as_str()))?),
    | None => None,
  };
  let source_run_slug = match optional_string(value, "sourceRunSlug") {
    | Some(text) if text.is_empty() => None,
    | Some(text) => Some(domain_value("run slug", RunSlug::try_from(text.as_str()))?),
    | None => None,
  };
  let mut linked_run_slugs: Vec<RunSlug> = Vec::new();
  for slug in run_slug.iter().chain(source_run_slug.iter()) {
    if !linked_run_slugs.contains(slug) {
      linked_run_slugs.push(slug.clone());
    }
  }
  let pull_request_number = match value.get("prNumber").and_then(Value::as_i64) {
    | Some(number) => Some(domain_value("プルリクエスト番号", PullRequestNumber::try_from(number))?),
    | None => None,
  };
  Ok(TaskDetail {
    project: project.clone(),
    task_name,
    goal_id,
    status: parse_task_status(value.get("status"))?,
    summary: optional_string(value, "summary").unwrap_or_default(),
    created_at: parse_time(value.get("createdAt"))?,
    started_at: parse_time(value.get("startedAt"))?,
    completed_at: parse_time(value.get("completedAt"))?,
    linked_run_slugs,
    run_slug,
    pull_request_number,
    branch: links.branch,
    pull_request_url: links.pull_request_url,
  })
}

/// 解析直後の問い合わせ。`created_at` は応答での有無を保持し、前回値の引き継ぎと
/// 初回観測の既定は `read_snapshot` の解像で決めます。
pub(super) struct ParsedInquiry {
  inquiry_id: InquiryId,
  goal_id:    Option<GoalId>,
  kind:       InquiryKind,
  body:       PreviewText,
  options:    Vec<String>,
  answered:   bool,
  created_at: Option<DateTime<Utc>>,
}

/// 1件の問い合わせを解析する。`approval` は判断待ちの種類（問い合わせとマージ待ち）に
/// 対応する値が無いため、判定対象外として `Ok(None)` を返して読み飛ばします。未知の種類は
/// 解析エラーにします。
pub(super) fn parse_inquiry(
  value: &Value,
  goal_id: Option<&GoalId>,
  shortened: &Shortened,
) -> Result<Option<ParsedInquiry>, ToolError> {
  let kind = match value.get("kind").and_then(Value::as_str) {
    | Some("question") => InquiryKind::Question,
    | Some("operation_request") => InquiryKind::OperationRequest,
    | Some("approval") => return Ok(None),
    | other => return Err(ToolError::Parse(format!("未知の問い合わせの種類です: {other:?}"))),
  };
  let answered = match value.get("status").and_then(Value::as_str) {
    | Some("pending") => false,
    | Some("answered") | Some("withdrawn") => true,
    | other => return Err(ToolError::Parse(format!("未知の問い合わせの状態です: {other:?}"))),
  };
  let options = value
    .get("options")
    .and_then(Value::as_array)
    .map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect())
    .unwrap_or_default();
  let inquiry_id = domain_value("inquiry ID", InquiryId::try_from(string_field(value, "id")?.as_str()))?;
  let goal_id = match goal_id {
    | Some(goal_id) => Some(goal_id.clone()),
    | None => match optional_string(value, "goalId") {
      | Some(text) if text.is_empty() => None,
      | Some(text) => Some(domain_value("ゴール ID", GoalId::try_from(text.as_str()))?),
      | None => None,
    },
  };
  Ok(Some(ParsedInquiry {
    inquiry_id,
    goal_id,
    kind,
    body: shortened.preview(value, "body"),
    options,
    answered,
    created_at: parse_time(value.get("createdAt"))?,
  }))
}

/// 解析済みの問い合わせを、`created_at` を解像して model 型へ変換する。
///
/// 応答に `createdAt` があればそれを使います。無い場合は、前回 snapshot の同一 inquiry_id の
/// created_at を引き継ぎ、初回観測なら読み取り時刻を既定にします。都度読み取り時刻へ既定化して
/// 滞留判定が毎サイクルリセットされることを避けます。
pub(super) fn resolve_inquiry(
  parsed: ParsedInquiry,
  prev: Option<&ProjectSnapshot>,
  observed_at: DateTime<Utc>,
) -> InquiryEntry {
  let created_at = parsed
    .created_at
    .or_else(|| {
      prev.and_then(|snapshot| {
        snapshot.inquiries.iter().find(|entry| entry.inquiry_id == parsed.inquiry_id).map(|entry| entry.created_at)
      })
    })
    .unwrap_or(observed_at);
  InquiryEntry {
    inquiry_id: parsed.inquiry_id,
    goal_id: parsed.goal_id,
    kind: parsed.kind,
    body: parsed.body,
    options: parsed.options,
    answered: parsed.answered,
    created_at,
  }
}

pub(super) fn parse_notification(
  goal_id: &GoalId,
  value: &Value,
  shortened: &Shortened,
) -> Result<NotificationEntry, ToolError> {
  let recorded_at = parse_time(value.get("recordedAt"))?
    .ok_or_else(|| ToolError::Parse("通知の recordedAt がありません".to_owned()))?;
  Ok(NotificationEntry {
    notification_id: string_field(value, "id")?,
    goal_id: goal_id.clone(),
    kind: string_field(value, "kind")?,
    body: shortened.display(optional_string(value, "body").unwrap_or_default(), "body"),
    recorded_at,
  })
}

#[cfg(test)]
mod tests {
  //! マージ待ちの起点の規則を、生成した入力で確かめる（設計: docs/design/issue-90.md の「PBT
  //! の性質」）。

  use chrono::{DateTime, Duration, TimeZone, Utc};
  use proptest::{collection::vec, option, prelude::*};

  use super::resolve_awaiting_merge_at;
  use crate::{
    domain::{GoalId, TaskName},
    model::{ExecutionStatus, GoalEntry, GoalStatus, ProjectSnapshot, TaskEntry, TaskStatus},
  };

  fn at(minutes: i64) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 13, 0, 0, 0).single().expect("基準の時刻は有効です") + Duration::minutes(minutes)
  }

  fn goal_id(value: &str) -> GoalId {
    GoalId::try_from(value).expect("テストのゴール ID は有効です")
  }

  fn goal(status: GoalStatus) -> GoalEntry {
    GoalEntry {
      goal_id: goal_id("g1"),
      status,
      execution_status: ExecutionStatus::Active,
      objective: crate::model::PreviewText::default(),
      integration_branch: String::new(),
      pull_request_number: None,
      pull_request_url: None,
      awaiting_merge_at: None,
      paused_reason: None,
    }
  }

  fn previous(start: Option<DateTime<Utc>>) -> ProjectSnapshot {
    ProjectSnapshot {
      goals: vec![GoalEntry { awaiting_merge_at: start, ..goal(GoalStatus::AwaitingMerge) }],
      ..ProjectSnapshot::empty()
    }
  }

  fn time() -> impl Strategy<Value = DateTime<Utc>> {
    (0_i64..1_000).prop_map(at)
  }

  fn goal_status() -> impl Strategy<Value = GoalStatus> {
    prop_oneof![Just(GoalStatus::Created), Just(GoalStatus::AwaitingMerge), Just(GoalStatus::Completed)]
  }

  fn task_status() -> impl Strategy<Value = TaskStatus> {
    prop_oneof![
      Just(TaskStatus::Pending),
      Just(TaskStatus::Running),
      Just(TaskStatus::Completed),
      Just(TaskStatus::Failed),
      Just(TaskStatus::Exceeded),
      Just(TaskStatus::PrFailed),
    ]
  }

  /// 対象ゴール（g1）・別ゴール（g2）・ゴール無しのタスク。仕上げかどうかと終了時刻も生成する。
  fn task() -> impl Strategy<Value = TaskEntry> {
    (
      prop_oneof![Just(Some("g1")), Just(Some("g2")), Just(None)],
      task_status(),
      any::<bool>(),
      option::of(time()),
      0_u32..1_000,
    )
      .prop_map(|(goal, status, merge_task, completed_at, number)| TaskEntry {
        name: TaskName::try_from(format!("t{number}").as_str()).expect("テストのタスク名は有効です"),
        goal_id: goal.map(goal_id),
        status,
        started_at: None,
        completed_at,
        run_slug: None,
        source_run_slug: None,
        current_step: None,
        merge_task,
      })
  }

  /// 対象ゴールに関わらないタスク（別ゴール・ゴール無し・仕上げでない）。
  fn unrelated_task() -> impl Strategy<Value = TaskEntry> {
    (task(), any::<bool>()).prop_map(|(mut task, other_goal)| {
      if task.goal_id.as_ref() == Some(&goal_id("g1")) {
        if other_goal {
          task.goal_id = Some(goal_id("g2"));
        } else {
          task.merge_task = false;
        }
      }
      task
    })
  }

  fn blank_task() -> TaskEntry {
    TaskEntry {
      name:            TaskName::try_from("merge").expect("テストのタスク名は有効です"),
      goal_id:         None,
      status:          TaskStatus::Pending,
      started_at:      None,
      completed_at:    None,
      run_slug:        None,
      source_run_slug: None,
      current_step:    None,
      merge_task:      false,
    }
  }

  /// pending/running を終わりの状態（completed）に置き換える。
  fn finished(mut task: TaskEntry) -> TaskEntry {
    if matches!(task.status, TaskStatus::Pending | TaskStatus::Running) {
      task.status = TaskStatus::Completed;
    }
    task
  }

  #[derive(Debug, Clone)]
  struct Case {
    status:       GoalStatus,
    notification: Option<DateTime<Utc>>,
    tasks:        Vec<TaskEntry>,
    prev:         Option<Option<DateTime<Utc>>>,
    observed_at:  DateTime<Utc>,
  }

  impl Case {
    fn resolve(&self) -> Option<DateTime<Utc>> {
      let prev = self.prev.map(previous);
      resolve_awaiting_merge_at(&goal(self.status), self.notification, &self.tasks, prev.as_ref(), self.observed_at)
    }

    fn goal_merge_tasks(&self) -> impl Iterator<Item = &TaskEntry> {
      self.tasks.iter().filter(|task| task.merge_task && task.goal_id.as_ref() == Some(&goal_id("g1")))
    }

    fn merge_task_active(&self) -> bool {
      self.goal_merge_tasks().any(|task| matches!(task.status, TaskStatus::Pending | TaskStatus::Running))
    }

    fn previous_start(&self) -> Option<DateTime<Utc>> {
      self.prev.flatten()
    }
  }

  fn case() -> impl Strategy<Value = Case> {
    (goal_status(), option::of(time()), vec(task(), 0..=5), option::of(option::of(time())), time()).prop_map(
      |(status, notification, tasks, prev, observed_at)| Case { status, notification, tasks, prev, observed_at },
    )
  }

  proptest! {
    /// 性質1：awaiting_merge で、対象ゴールの仕上げタスクに pending/running があれば起点を持たない。
    #[test]
    fn no_start_while_a_merge_task_is_active(
      case in case(),
      active in prop_oneof![Just(TaskStatus::Pending), Just(TaskStatus::Running)],
      position in any::<prop::sample::Index>(),
    ) {
      let mut tasks = case.tasks.clone();
      let mut merge = blank_task();
      merge.goal_id = Some(goal_id("g1"));
      merge.merge_task = true;
      merge.status = active;
      tasks.insert(position.index(tasks.len() + 1), merge);
      let case = Case { status: GoalStatus::AwaitingMerge, tasks, ..case };
      prop_assert!(case.merge_task_active());
      prop_assert_eq!(case.resolve(), None);
    }

    /// 性質2：awaiting_merge で仕上げタスクが終わっていれば、起点は通知・どの completed_at・前回の起点よりも前にならない。
    #[test]
    fn start_is_not_before_any_known_wait_start(case in case()) {
      let tasks = case.tasks.iter().cloned().map(finished).collect();
      let case = Case { status: GoalStatus::AwaitingMerge, tasks, ..case };
      let start = case.resolve();
      prop_assert!(start.is_some());
      let start = start.unwrap();
      let known = case.goal_merge_tasks().map(|task| task.completed_at).chain([case.notification, case.previous_start()]);
      for bound in known.flatten() {
        prop_assert!(start >= bound, "起点 {start} が {bound} より前です");
      }
    }

    /// 性質3：1回目の結果を前回の起点にして、観測時刻だけを後へずらして解き直しても、同じ値になる。
    #[test]
    fn start_does_not_reset_on_later_reads(case in case(), later in 1_i64..10_000) {
      let first = case.resolve();
      let again = Case { prev: Some(first), observed_at: case.observed_at + Duration::minutes(later), ..case.clone() };
      prop_assert_eq!(again.resolve(), first);
    }

    /// 性質4：対象ゴール以外のタスクと仕上げでないタスクを足しても、結果は変わらない。
    #[test]
    fn unrelated_tasks_do_not_change_the_start(case in case(), extra in vec(unrelated_task(), 1..=5)) {
      let mut tasks = case.tasks.clone();
      tasks.extend(extra);
      let widened = Case { tasks, ..case.clone() };
      prop_assert_eq!(widened.resolve(), case.resolve());
    }

    /// 性質5：結果は、通知・completed_at・前回の起点・観測時刻のどれかと一致する。候補が無ければ観測時刻。
    #[test]
    fn start_comes_from_a_candidate_or_the_observation(case in case()) {
      let Some(start) = case.resolve() else {
        return Ok(());
      };
      let mut candidates: Vec<DateTime<Utc>> =
        case.goal_merge_tasks().filter_map(|task| task.completed_at).collect();
      candidates.extend(case.notification);
      candidates.extend(case.previous_start());
      if candidates.is_empty() && case.status == GoalStatus::AwaitingMerge {
        prop_assert_eq!(start, case.observed_at);
      } else {
        prop_assert!(candidates.contains(&start) || start == case.observed_at);
      }
    }

    /// 性質6：awaiting_merge でないゴールの結果は、通知の起点と一致する（修正前と同じ）。
    #[test]
    fn non_awaiting_goal_keeps_the_notification_start(
      case in case(),
      status in prop_oneof![Just(GoalStatus::Created), Just(GoalStatus::Completed)],
    ) {
      let case = Case { status, ..case };
      prop_assert_eq!(case.resolve(), case.notification);
    }
  }
}

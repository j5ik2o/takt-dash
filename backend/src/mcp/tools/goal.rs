//! ゴール・タスク・問い合わせ・通知・登録提案の応答解析。

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::{
  super::client::ToolCallError,
  parse::{optional_string, parse_time, string_field},
  ToolError,
};
use crate::model::{
  ExecutionStatus, GoalEntry, GoalQuestionEntry, GoalStatus, GoalWorkLogEntry, InquiryEntry, InquiryKind,
  NotificationEntry, ProjectSnapshot, TaskDetail, TaskEntry, TaskStatus,
};

/// 前回 snapshot の問い合わせのうち、`matches` に合うものを解析済みの形で取り出す。
/// 道具エラー時に該当項目だけを引き継ぐために使います。前回が無ければ空です。
pub(super) fn prev_inquiries(
  prev: Option<&ProjectSnapshot>,
  matches: impl Fn(Option<&str>) -> bool,
) -> Vec<ParsedInquiry> {
  let Some(prev) = prev else {
    return Vec::new();
  };
  prev
    .inquiries
    .iter()
    .filter(|entry| matches(entry.goal_id.as_deref()))
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
/// 通知から取れた起点が最も優先です。通知が無い `awaiting_merge` のゴールは、前回 snapshot の
/// 同じゴールの起点を引き継ぎ、前回が無い（初回観測）または前回に起点が無い場合は、その読み
/// 取りの観測時刻を起点にします。初回観測でも起点を一度確定して以降の読み取りで保持するため、
/// 待ち時間が読み取りごとに現在時刻へリセットされて滞留しなくなることを避けます。
/// `awaiting_merge` でないゴールは起点を持ちません。
pub(super) fn resolve_awaiting_merge_at(
  goal: &GoalEntry,
  notification_start: Option<DateTime<Utc>>,
  prev: Option<&ProjectSnapshot>,
  observed_at: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
  if let Some(recorded_at) = notification_start {
    return Some(recorded_at);
  }
  if goal.status != GoalStatus::AwaitingMerge {
    return None;
  }
  let previous_start = prev
    .and_then(|snapshot| snapshot.goals.iter().find(|entry| entry.goal_id == goal.goal_id))
    .and_then(|entry| entry.awaiting_merge_at);
  Some(previous_start.unwrap_or(observed_at))
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

pub(super) fn parse_goal(value: &Value) -> Result<GoalEntry, ToolError> {
  let goal_id = string_field(value, "id")?;
  let status = parse_goal_status(value.get("status"))?;
  let execution_status = parse_goal_execution_status(value.get("executionStatus"))?;
  let pull_request = value.get("pullRequest");
  let pull_request_number = pull_request.and_then(|value| value.get("number")).and_then(Value::as_i64);
  let pull_request_url = pull_request.and_then(|value| value.get("url")).and_then(Value::as_str).map(str::to_owned);
  Ok(GoalEntry {
    goal_id,
    status,
    execution_status,
    objective: optional_string(value, "objective").unwrap_or_default(),
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
  Ok(TaskEntry {
    name: string_field(value, "name")?,
    goal_id: optional_string(value, "goalId"),
    status,
    started_at,
    run_slug: optional_string(value, "runSlug"),
    current_step: optional_string(value, "currentStep"),
    merge_task: value.get("mergeTask").and_then(Value::as_bool).unwrap_or(false),
  })
}

pub(super) fn parse_goal_work_log(value: &Value) -> Result<GoalWorkLogEntry, ToolError> {
  let recorded_at = parse_time(value.get("recordedAt"))?
    .ok_or_else(|| ToolError::Parse("作業記録の recordedAt がありません".to_owned()))?;
  Ok(GoalWorkLogEntry {
    work_log_id: string_field(value, "id")?,
    recorded_at,
    summary: optional_string(value, "summary").unwrap_or_default(),
  })
}

pub(super) fn parse_goal_question(value: &Value) -> Result<GoalQuestionEntry, ToolError> {
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
    body: optional_string(value, "body").unwrap_or_default(),
    options,
    answer_text: answer.and_then(|answer| optional_string(answer, "text")),
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

pub(super) fn parse_task_detail(project: &str, value: &Value, links: TaskLinkFields) -> Result<TaskDetail, ToolError> {
  let run_slug = optional_string(value, "runSlug");
  let source_run_slug = optional_string(value, "sourceRunSlug");
  let mut linked_run_slugs: Vec<String> = Vec::new();
  for slug in run_slug.iter().chain(source_run_slug.iter()) {
    if !linked_run_slugs.contains(slug) {
      linked_run_slugs.push(slug.clone());
    }
  }
  Ok(TaskDetail {
    project: project.to_owned(),
    task_name: string_field(value, "name")?,
    goal_id: optional_string(value, "goalId"),
    status: parse_task_status(value.get("status"))?,
    summary: optional_string(value, "summary").unwrap_or_default(),
    created_at: parse_time(value.get("createdAt"))?,
    started_at: parse_time(value.get("startedAt"))?,
    completed_at: parse_time(value.get("completedAt"))?,
    linked_run_slugs,
    run_slug,
    pull_request_number: value.get("prNumber").and_then(Value::as_i64),
    branch: links.branch,
    pull_request_url: links.pull_request_url,
  })
}

/// 解析直後の問い合わせ。`created_at` は応答での有無を保持し、前回値の引き継ぎと
/// 初回観測の既定は `read_snapshot` の解像で決めます。
pub(super) struct ParsedInquiry {
  inquiry_id: String,
  goal_id:    Option<String>,
  kind:       InquiryKind,
  body:       String,
  options:    Vec<String>,
  answered:   bool,
  created_at: Option<DateTime<Utc>>,
}

/// 1件の問い合わせを解析する。`approval` は判断待ちの種類（問い合わせとマージ待ち）に
/// 対応する値が無いため、判定対象外として `Ok(None)` を返して読み飛ばします。未知の種類は
/// 解析エラーにします。
pub(super) fn parse_inquiry(value: &Value, goal_id: Option<&str>) -> Result<Option<ParsedInquiry>, ToolError> {
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
  Ok(Some(ParsedInquiry {
    inquiry_id: string_field(value, "id")?,
    goal_id: goal_id.map(str::to_owned).or_else(|| optional_string(value, "goalId")),
    kind,
    body: optional_string(value, "body").unwrap_or_default(),
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

pub(super) fn parse_notification(goal_id: &str, value: &Value) -> Result<NotificationEntry, ToolError> {
  let recorded_at = parse_time(value.get("recordedAt"))?
    .ok_or_else(|| ToolError::Parse("通知の recordedAt がありません".to_owned()))?;
  Ok(NotificationEntry {
    notification_id: string_field(value, "id")?,
    goal_id: goal_id.to_owned(),
    kind: string_field(value, "kind")?,
    body: optional_string(value, "body").unwrap_or_default(),
    recorded_at,
  })
}

/// ゴール登録提案の `summary` を表示本文へ変換する。
///
/// 文字列はそのまま本文にします。オブジェクトは既知の項目だけを日本語の文として組み立て、
/// 未知のキーと欠落は無視します。どちらでもない値だけを解析エラーにします。
pub(super) fn registration_summary_text(value: Option<&Value>) -> Result<String, ToolError> {
  match value {
    | None | Some(Value::Null) => Ok(String::new()),
    | Some(Value::String(text)) => Ok(text.clone()),
    | Some(Value::Object(object)) => Ok(compose_registration_summary(object)),
    | Some(_) => Err(ToolError::Parse("summary が文字列でもオブジェクトでもありません".to_owned())),
  }
}

/// `summary` オブジェクトから本文へ含める既知の項目と見出し。
const REGISTRATION_SUMMARY_FIELDS: [(&str, &str); 6] = [
  ("objective", "目的"),
  ("outOfScope", "対象外"),
  ("acceptanceCriteria", "受入条件"),
  ("constraints", "制約"),
  ("startBranch", "開始ブランチ"),
  ("integrationBranch", "統合ブランチ"),
];

fn compose_registration_summary(object: &serde_json::Map<String, Value>) -> String {
  let mut lines = Vec::new();
  for (key, label) in REGISTRATION_SUMMARY_FIELDS {
    if let Some(text) = registration_summary_field(object.get(key)) {
      lines.push(format!("{label}: {text}"));
    }
  }
  lines.join("\n")
}

/// `summary` オブジェクトの項目値を本文用の文字列にする。文字列と文字列の配列だけを受け付け、
/// それ以外の型と空の配列は無視します。
fn registration_summary_field(value: Option<&Value>) -> Option<String> {
  match value? {
    | Value::String(text) => Some(text.clone()),
    | Value::Array(items) => {
      let texts: Vec<&str> = items.iter().filter_map(Value::as_str).collect();
      if texts.is_empty() {
        None
      } else {
        Some(texts.join("、"))
      }
    },
    | _ => None,
  }
}

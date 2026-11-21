//! 取得した応答から、CLI が表示する日本語の文字列を組み立てます。

use crate::{
  domain::{
    self, GoalId, InquiryId, InquiryKind, PendingDecisionType, ProjectName, PullRequestNumber, RegistrationRevision,
    RunStatus, ServerId, StaleMinutes, TaskCount,
  },
  proto::{
    self,
    v1::{
      AnswerInquiryResponse, ApproveGoalMergeResponse, ApproveGoalRegistrationResponse, DashboardState,
      ExecutionStatus, GetConversationResponse, GetGoalDetailResponse, GetGoalsResponse, Goal, GoalQuestion,
      GoalRegistrationProposal, GoalStatus, RejectGoalRegistrationResponse, TaskStatus,
    },
  },
};

/// 概要の表示内容を日本語の文字列へ組み立てる。
///
/// 取得した `DashboardState` だけから表示を作り、`println!` へ直書きしません。表示する項目は、
/// サーバーの接続状態、プロジェクト、ゴールとタスクの件数、run、判断待ちの件数と内訳、
/// 異常判定の閾値、最後に読めた時刻です。
pub(crate) fn render_overview(state: &DashboardState) -> Result<String, domain::DomainError> {
  let mut running_runs = 0_usize;
  for run in &state.runs {
    if RunStatus::try_from(run.status)? == RunStatus::Running {
      running_runs += 1;
    }
  }
  let mut inquiries = 0_usize;
  let mut awaiting_merge = 0_usize;
  let mut goal_registration = 0_usize;
  for decision in &state.pending_decisions {
    match PendingDecisionType::try_from(decision.r#type)? {
      | PendingDecisionType::Inquiry => inquiries += 1,
      | PendingDecisionType::AwaitingMerge => awaiting_merge += 1,
      | PendingDecisionType::GoalRegistration => goal_registration += 1,
    }
  }
  let servers: Vec<String> = state
    .servers
    .iter()
    .map(|server| {
      let state_label = if server.connected {
        "接続中"
      } else if server.write_only {
        "書き込み専用"
      } else {
        "応答なし"
      };
      format!("- {}（{}、{}）", server.name, server.kind, state_label)
    })
    .collect();
  let projects: Vec<String> = state
    .projects
    .iter()
    .map(|project| {
      let name = ProjectName::try_from(project.name.as_str())?;
      Ok(format!("- {name}（{}）", project.about))
    })
    .collect::<Result<_, domain::DomainError>>()?;
  let goals: Vec<String> = state.goals.iter().map(render_goal_line).collect::<Result<_, _>>()?;
  let runs: Vec<String> = state.runs.iter().map(render_run_line).collect::<Result<_, _>>()?;
  let pending_detail: Vec<String> =
    state.pending_decisions.iter().map(render_pending_detail).collect::<Result<_, _>>()?;
  let stale_minutes = match state.thresholds.as_ref() {
    | Some(thresholds) => Some(StaleMinutes::try_from(thresholds.pending_decision_stale_minutes)?),
    | None => None,
  };
  let silent_minutes =
    state.thresholds.as_ref().map(|thresholds| thresholds.run_log_silent_minutes).unwrap_or_default();
  Ok(format!(
    "概要\n\
     読んだ時刻: {}\n\
     サーバー: {}台\n\
     {}\n\
     プロジェクト: {}件\n\
     {}\n\
     ゴール: {}件\n\
     {}\n\
     実行中の run: {}件\n\
     {}\n\
     判断待ち: {}件（未回答の問い合わせ {}件、人のマージ待ち {}件、ゴール登録の承認待ち {}件）\n\
     {}\n\
     異常判定の閾値: 判断待ちの滞留 {}分、ログの無音 {}分\n",
    format_timestamp(state.read_at.as_ref()),
    state.servers.len(),
    join_lines(&servers),
    state.projects.len(),
    join_lines(&projects),
    state.goals.len(),
    join_lines(&goals),
    running_runs,
    join_lines(&runs),
    state.pending_decisions.len(),
    inquiries,
    awaiting_merge,
    goal_registration,
    join_lines(&pending_detail),
    stale_minutes.map_or(0, StaleMinutes::get),
    silent_minutes,
  ))
}

fn goal_display_name(goal: &Goal) -> String {
  if goal.name_truncated {
    format!("{}…", goal.name)
  } else {
    goal.name.clone()
  }
}

/// ゴール1件の表示行を作る。プロジェクト名・ゴール ID・件数を検証済みの型へ通す。
fn render_goal_line(goal: &proto::v1::Goal) -> Result<String, domain::DomainError> {
  let project = ProjectName::try_from(goal.project.as_str())?;
  let goal_id = GoalId::try_from(goal.goal_id.as_str())?;
  let counts = goal.task_counts.unwrap_or_default();
  let completed = TaskCount::try_from(counts.completed)?;
  let failed = TaskCount::try_from(counts.failed)?;
  let pending = TaskCount::try_from(counts.pending)?;
  let running = TaskCount::try_from(counts.running)?;
  Ok(format!(
    "- {project}/{goal_id}「{}」完了{completed}件・失敗{failed}件（未着手{pending}件・実行中{running}件）",
    goal_display_name(goal)
  ))
}

/// run 1件の表示行を作る。プロジェクト名を検証済みの型へ通す。
fn render_run_line(run: &proto::v1::Run) -> Result<String, domain::DomainError> {
  let project = ProjectName::try_from(run.project.as_str())?;
  Ok(format!("- {project}/{}（{}）", run.task_name, run.summary))
}

/// 判断待ち1件の概要行を作る。
fn render_pending_detail(decision: &proto::v1::PendingDecision) -> Result<String, domain::DomainError> {
  let project = ProjectName::try_from(decision.project.as_str())?;
  let server_id = ServerId::try_from(decision.server_id.as_str())?;
  let kind = decision_label(PendingDecisionType::try_from(decision.r#type)?, decision.inquiry_kind)?;
  Ok(format!("- {project}/{server_id}: {kind} {}", decision.body))
}

/// 判断待ちの種類を表示ラベルへ変換する。問い合わせのときだけ問い合わせの種類を検証する。
fn decision_label(decision_type: PendingDecisionType, inquiry_kind: i32) -> Result<&'static str, domain::DomainError> {
  match decision_type {
    | PendingDecisionType::AwaitingMerge => Ok("人のマージ待ち"),
    | PendingDecisionType::GoalRegistration => Ok("承認待ち"),
    | PendingDecisionType::Inquiry => match InquiryKind::try_from(inquiry_kind)? {
      | InquiryKind::OperationRequest => Ok("問い合わせ（作業の依頼）"),
      | InquiryKind::Question => Ok("問い合わせ（質問）"),
    },
  }
}

/// 会話の表示内容を日本語の文字列へ組み立てる。
///
/// 発言は seq の昇順に並べ、発話者・時刻・本文を出す。unavailable_reason は会話が無い理由、
/// skipped は理由と件数として表示する。
pub(crate) fn render_conversation(response: &GetConversationResponse) -> Result<String, domain::DomainError> {
  let project = ProjectName::try_from(response.project.as_str())?;
  let mut lines = vec![
    format!("会話: {project}"),
    format!("セッション: {}", response.session_id),
    format!("プロバイダ: {}", response.provider),
  ];
  if let Some(reason) = response.unavailable_reason.as_deref() {
    lines.push(format!("会話を表示できません: {reason}"));
  }
  for skipped in &response.skipped {
    lines.push(format!("スキップした行: {} {}件", skipped.reason, skipped.count));
  }

  let mut messages = response.messages.clone();
  messages.sort_by_key(|message| message.seq);
  messages.dedup_by_key(|message| message.seq);
  if messages.is_empty() {
    if response.unavailable_reason.is_none() {
      lines.push("（会話はありません）".to_owned());
    }
  } else {
    lines.extend(messages.iter().map(|message| {
      format!(
        "- [{}] {}: {}",
        format_timestamp(message.occurred_at.as_ref()),
        conversation_speaker_label(message.speaker),
        message.text
      )
    }));
  }
  Ok(lines.join("\n"))
}

/// 発話者の数値を表示名へ変換する。未知の値は「不明」にする。
fn conversation_speaker_label(speaker: i32) -> &'static str {
  match proto::v1::ConversationSpeaker::try_from(speaker) {
    | Ok(proto::v1::ConversationSpeaker::User) => "ユーザー",
    | Ok(proto::v1::ConversationSpeaker::Manager) => "manager",
    | Ok(proto::v1::ConversationSpeaker::System) => "システム",
    | _ => "不明",
  }
}

/// ゴールの状態を表示名へ変換する。未知の値は「不明」にする。
fn goal_status_label(status: i32) -> &'static str {
  match GoalStatus::try_from(status) {
    | Ok(GoalStatus::Created) => "作成済み",
    | Ok(GoalStatus::AwaitingMerge) => "人のマージ待ち",
    | Ok(GoalStatus::Completed) => "完了",
    | _ => "不明",
  }
}

/// ゴールの実行状態を表示名へ変換する。未知の値は「不明」にする。
fn execution_status_label(status: i32) -> &'static str {
  match ExecutionStatus::try_from(status) {
    | Ok(ExecutionStatus::Active) => "稼働中",
    | Ok(ExecutionStatus::Paused) => "一時停止中",
    | Ok(ExecutionStatus::Aborted) => "中止",
    | _ => "不明",
  }
}

/// タスクの状態を表示名へ変換する。未知の値は「不明」にする。
fn task_status_label(status: i32) -> &'static str {
  match TaskStatus::try_from(status) {
    | Ok(TaskStatus::Pending) => "未着手",
    | Ok(TaskStatus::Running) => "実行中",
    | Ok(TaskStatus::Completed) => "完了",
    | Ok(TaskStatus::Failed) => "失敗",
    | Ok(TaskStatus::Exceeded) => "上限超過",
    | Ok(TaskStatus::PrFailed) => "PR 失敗",
    | _ => "不明",
  }
}

/// 質問の種類を表示名へ変換する。未知の値は原値のまま返す。
fn question_kind_label(kind: &str) -> String {
  match kind {
    | "question" => "質問".to_owned(),
    | "operation_request" => "作業の依頼".to_owned(),
    | "approval" => "承認".to_owned(),
    | other => other.to_owned(),
  }
}

/// 質問の状態を表示名へ変換する。未知の値は原値のまま返す。
fn question_status_label(status: &str) -> String {
  match status {
    | "pending" => "未回答".to_owned(),
    | "answered" => "回答済み".to_owned(),
    | "withdrawn" => "取り下げ".to_owned(),
    | other => other.to_owned(),
  }
}

/// 質問への回答（可否）を表示名へ変換する。未知の値は原値のまま返す。
fn answer_decision_label(decision: &str) -> String {
  match decision {
    | "allow" => "許可".to_owned(),
    | "deny" => "否認".to_owned(),
    | other => other.to_owned(),
  }
}

/// 質問への回答（作業の報告）を表示名へ変換する。未知の値は原値のまま返す。
fn answer_report_label(report: &str) -> String {
  match report {
    | "completed" => "実施した".to_owned(),
    | "unable" => "対応できない".to_owned(),
    | other => other.to_owned(),
  }
}

/// ゴール一覧の表示内容を日本語の文字列へ組み立てる。
///
/// 応答順に1行ずつ、サーバー・プロジェクト・ゴールID・名前・状態・実行状態・タスクの4種の
/// 件数・マージ待ちの有無・プルリクエストを出す。空の一覧は `join_lines` が「（なし）」にする。
pub(crate) fn render_goals(response: &GetGoalsResponse) -> String {
  let lines: Vec<String> = response.goals.iter().map(render_goal_row).collect();
  join_lines(&lines)
}

/// ゴール一覧の1行を作る。マージ待ちは `awaiting_merge_at` の有無だけで判定する。
fn render_goal_row(goal: &Goal) -> String {
  let counts = goal.task_counts.unwrap_or_default();
  let merge_wait = if goal.awaiting_merge_at.is_some() { "あり" } else { "なし" };
  let pull_request = match &goal.pull_request {
    | Some(pull_request) => format!("#{} {}", pull_request.number, pull_request.url),
    | None => "なし".to_owned(),
  };
  format!(
    "- {}/{}/{}「{}」状態: {}、実行状態: {}、タスク: 完了{}件・失敗{}件（未着手{}件・実行中{}件）、\
     マージ待ち: {}、プルリクエスト: {}",
    goal.server_id,
    goal.project,
    goal.goal_id,
    goal_display_name(goal),
    goal_status_label(goal.status),
    execution_status_label(goal.execution_status),
    counts.completed,
    counts.failed,
    counts.pending,
    counts.running,
    merge_wait,
    pull_request
  )
}

/// ゴール詳細の表示内容を日本語の文字列へ組み立てる。
///
/// 目的・対象外・受け入れ条件・制約・ブランチ・統合ブランチ・状態・実行状態・作業記録・
/// 質問と回答・紐づくタスクを出す。空の一覧は「（なし）」にする。
pub(crate) fn render_goal_detail(detail: &GetGoalDetailResponse) -> String {
  let work_log: Vec<String> = detail
    .work_log
    .iter()
    .map(|log| format!("- {} {}", format_timestamp(log.recorded_at.as_ref()), log.summary))
    .collect();
  let questions: Vec<String> = detail.questions.iter().map(render_goal_question).collect();
  let tasks: Vec<String> = detail
    .tasks
    .iter()
    .map(|task| {
      let run_slug = task.run_slug.as_deref().filter(|run_slug| !run_slug.is_empty()).unwrap_or("なし");
      format!("- {}（{}、run: {}）", task.task_name, task_status_label(task.status), run_slug)
    })
    .collect();
  [
    format!("ゴール: {}", detail.goal_id),
    format!("サーバー: {}", detail.server_id),
    format!("プロジェクト: {}", detail.project),
    format!("状態: {}", goal_status_label(detail.status)),
    format!("実行状態: {}", execution_status_label(detail.execution_status)),
    format!("目的: {}", detail.objective),
    format!("対象外:\n{}", render_string_items(&detail.out_of_scope)),
    format!("受け入れ条件:\n{}", render_string_items(&detail.acceptance_criteria)),
    format!("制約:\n{}", render_string_items(&detail.constraints)),
    format!("ブランチ: {}", detail.branch),
    format!("統合ブランチ: {}", detail.integration_branch),
    format!("作業記録:\n{}", join_lines(&work_log)),
    format!("質問:\n{}", join_lines(&questions)),
    format!("タスク:\n{}", join_lines(&tasks)),
  ]
  .join("\n")
}

/// ゴールの質問1件を表示する。回答があれば回答本文を併記する。
fn render_goal_question(question: &GoalQuestion) -> String {
  let mut line = format!(
    "- {}（{}）: {}",
    question_kind_label(&question.kind),
    question_status_label(&question.status),
    question.body
  );
  if let Some(answer) = question.answer_text.as_deref() {
    line.push_str(&format!("/回答: {answer}"));
  }
  if let Some(decision) = question.answer_decision.as_deref() {
    line.push_str(&format!("/回答: {}", answer_decision_label(decision)));
  }
  if let Some(report) = question.answer_report.as_deref() {
    line.push_str(&format!("/回答: {}", answer_report_label(report)));
  }
  line
}

/// 文字列の一覧を箇条書きにし、空の一覧は「（なし）」にする。
fn render_string_items(items: &[String]) -> String {
  let lines: Vec<String> = items.iter().map(|item| format!("- {item}")).collect();
  join_lines(&lines)
}

/// 判断待ちの一覧を、指定したサーバーとプロジェクトに絞って表示する。
///
/// 回答・マージの承認へそのまま渡せる識別子（問い合わせ ID・ゴール ID・プルリクエスト番号・
/// リビジョン）を固定ラベルで出す。滞留期限は proto に専用フィールドが無いため、作成時刻と
/// 設定の滞留分数から算出して併記します。承認待ちの登録提案は backend が作成時刻を射影時刻で
/// 返すため、滞留期限の算出対象にしません。
pub(crate) fn render_decisions(
  state: &DashboardState,
  server_id: &ServerId,
  project: &ProjectName,
) -> Result<String, domain::DomainError> {
  let stale_minutes = match state.thresholds.as_ref() {
    | Some(thresholds) => Some(StaleMinutes::try_from(thresholds.pending_decision_stale_minutes)?),
    | None => None,
  };
  let decisions: Vec<&proto::v1::PendingDecision> = state
    .pending_decisions
    .iter()
    .filter(|decision| decision.server_id == server_id.as_str() && decision.project == project.as_str())
    .collect();
  let lines: Vec<String> =
    decisions.iter().map(|decision| render_decision(decision, stale_minutes)).collect::<Result<_, _>>()?;
  Ok(format!("判断待ち: {}件（{}/{}）\n{}", decisions.len(), server_id, project, join_lines(&lines)))
}

/// 判断待ち1件の表示を作る。該当しない項目の行は出さない。
fn render_decision(
  decision: &proto::v1::PendingDecision,
  stale_minutes: Option<StaleMinutes>,
) -> Result<String, domain::DomainError> {
  let decision_type = PendingDecisionType::try_from(decision.r#type)?;
  let kind = decision_label(decision_type, decision.inquiry_kind)?;
  let mut lines = vec![format!("- {kind}: {}", decision.body)];
  if !decision.inquiry_id.is_empty() {
    let inquiry_id = InquiryId::try_from(decision.inquiry_id.as_str())?;
    lines.push(format!("問い合わせID: {inquiry_id}"));
  }
  if let Some(goal_id) = decision.goal_id.as_deref() {
    let goal_id = GoalId::try_from(goal_id)?;
    lines.push(format!("ゴールID: {goal_id}"));
  }
  if let Some(pull_request) = &decision.pull_request {
    let number = PullRequestNumber::try_from(pull_request.number)?;
    lines.push(format!("プルリクエスト番号: {number}"));
  }
  if let Some(revision) = decision.revision {
    let revision = RegistrationRevision::try_from(revision)?;
    lines.push(format!("リビジョン: {revision}"));
  }
  if decision_type != PendingDecisionType::GoalRegistration {
    lines.push(format!("作成時刻: {}", format_timestamp(decision.created_at.as_ref())));
    lines.push(format!(
      "滞留期限: {}",
      stale_deadline(decision.created_at.as_ref(), stale_minutes.map_or(0, StaleMinutes::get))
    ));
  }
  if !decision.options.is_empty() {
    lines.push(format!("選択肢: {}", decision.options.join("、")));
  }
  Ok(lines.join("\n"))
}

/// 判断待ちの滞留期限（作成時刻 + 滞留分数）を表示用の文字列にする。
fn stale_deadline(created_at: Option<&prost_types::Timestamp>, stale_minutes: i32) -> String {
  let Some(created_at) = created_at else {
    return "不明".to_owned();
  };
  let Some(at) = chrono::DateTime::from_timestamp(created_at.seconds, 0) else {
    return "不明".to_owned();
  };
  (at + chrono::Duration::minutes(i64::from(stale_minutes))).format("%Y-%m-%d %H:%M:%S UTC").to_string()
}

/// 一覧を改行で連結し、空の一覧は空であることが分かる表示にする。
fn join_lines(lines: &[String]) -> String {
  if lines.is_empty() {
    "（なし）".to_owned()
  } else {
    lines.join("\n")
  }
}

/// proto の時刻を、タイムゾーンに依存しない UTC の表記にする。
///
/// 表示は日本語に揃えるため、秒だけの数値ではなく日時として出す。
fn format_timestamp(timestamp: Option<&prost_types::Timestamp>) -> String {
  let Some(timestamp) = timestamp else {
    return "不明".to_owned();
  };
  match chrono::DateTime::from_timestamp(timestamp.seconds, 0) {
    | Some(at) => at.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
    | None => "不明".to_owned(),
  }
}

/// 問い合わせへの回答の結果を表示する。
pub(crate) fn render_answer(response: &AnswerInquiryResponse) -> Result<String, domain::DomainError> {
  let inquiry_id = InquiryId::try_from(response.inquiry_id.as_str())?;
  Ok(format!("回答を記録しました。\n問い合わせ: {inquiry_id}"))
}

/// マージの承認の結果を表示する。
pub(crate) fn render_merge(response: &ApproveGoalMergeResponse) -> String {
  format!("マージを承認しました。\nタスク: {}\nワークフロー: {}", response.task_name, response.workflow)
}

/// ゴール登録の承認待ちの一覧を表示する。
pub(crate) fn render_registrations(proposals: &[GoalRegistrationProposal]) -> Result<String, domain::DomainError> {
  let lines: Vec<String> = proposals
    .iter()
    .map(|proposal| {
      let server_id = ServerId::try_from(proposal.server_id.as_str())?;
      let project = ProjectName::try_from(proposal.project.as_str())?;
      let revision = RegistrationRevision::try_from(proposal.revision)?;
      Ok(format!(
        "- {server_id}/{project}（リビジョン {revision}）: {}（{}）",
        proposal.body, proposal.repository_directory
      ))
    })
    .collect::<Result<_, domain::DomainError>>()?;
  Ok(format!("ゴール登録の承認待ち: {}件\n{}", proposals.len(), join_lines(&lines)))
}

/// ゴール登録の承認の結果を表示する。
pub(crate) fn render_approve_registration(
  response: &ApproveGoalRegistrationResponse,
) -> Result<String, domain::DomainError> {
  let revision = RegistrationRevision::try_from(response.revision)?;
  let goal_id = GoalId::try_from(response.goal_id.as_str())?;
  Ok(format!("ゴール登録を承認しました。\nリビジョン: {revision}\nゴール: {goal_id}"))
}

/// ゴール登録の却下の結果を表示する。
pub(crate) fn render_reject_registration(
  response: &RejectGoalRegistrationResponse,
) -> Result<String, domain::DomainError> {
  let revision = RegistrationRevision::try_from(response.revision)?;
  Ok(format!("ゴール登録を却下しました。\nリビジョン: {revision}"))
}

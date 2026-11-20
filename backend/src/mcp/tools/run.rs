//! takt_get_run の応答から run を作る。
//!
//! run は通常、run の保存場所（[`crate::run_progress`]）から読みます。ローカルに保存場所を読めない
//! プロジェクト（SSH 越しに扱うリモートのサーバーのプロジェクトなど）の run だけ、takt-mcp の
//! takt_get_run の応答から作ります。同じプロジェクトの run のレポート本文も、takt_get_run の
//! view report から読みます。指示書の本文は view instructions へ instructionId を指定して
//! 読みます。run 詳細では、指示書の一覧（view instructions）と、要約には含まれないタスク本文
//! （view task）も同じ道具から読みます。

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{json, Value};

use super::{
  parse::{checked_i32, domain_value, optional_string, parse_time, string_field},
  ToolError, ToolSetCore,
};
use crate::{
  domain::{InstructionId, ReportFilename, RunSlug},
  model::{ReportEntry, RunEntry, RunStatus, RunStepEntry, StepLogEntry, TaskEntry},
  run_log::{Instruction, InstructionState, RunContent},
  run_progress::task_summary,
};

/// ログの末尾に残す行数。ローカルの集計（run_progress の LOG_TAIL_RECORDS）と同じです。
const LOG_TAIL_RECORDS: usize = 20;

/// takt_get_run から読んだ run。stepLogs は全ページを集めたものです。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteRun {
  pub run_slug: RunSlug,
  pub session_id: String,
  /// 応答の summary。無ければ taskText の1行目（takt のタスク要約と同じ規則）です。
  pub summary: String,
  pub task_text: String,
  pub workflow: Option<String>,
  pub status: RunStatus,
  pub current_step: Option<String>,
  pub current_iteration: Option<i32>,
  pub phase: Option<i32>,
  pub started_at: Option<DateTime<Utc>>,
  pub current_step_started_at: Option<DateTime<Utc>>,
  pub current_step_execution_count: Option<i32>,
  pub steps: Vec<RunStepEntry>,
  pub step_logs: Vec<StepLogEntry>,
  pub reports: Vec<ReportEntry>,
  /// view instructions から読んだ指示書の一覧（要約だけ。本文は含みません）。
  /// 詳細の読み取りだけが埋め、一覧の読み取りでは空です。
  pub instructions: Vec<Instruction>,
  /// 応答の lastLogAt。
  last_log_at: Option<DateTime<Utc>>,
}

impl RemoteRun {
  /// 一覧の run を作る。ステップ位置は workflow の一覧を持つ呼び出し側が補います。
  pub fn run_entry(&self, task: &TaskEntry) -> RunEntry {
    let started_at = self.started_at(Some(task));
    RunEntry {
      run_slug: self.run_slug.as_str().to_owned(),
      session_id: self.session_id.clone(),
      task_name: task.name.clone(),
      goal_id: task.goal_id.clone(),
      summary: self.summary.clone(),
      status: self.status,
      current_step: self.current_step.clone(),
      current_step_number: None,
      total_steps: None,
      current_iteration: self.current_iteration,
      started_at,
      current_step_started_at: self.current_step_started_at,
      last_log_at: self.resolve_last_log_at(started_at),
      steps: self.steps.clone(),
      reports: self.reports.clone(),
    }
  }

  /// run の開始時刻。応答の開始時刻を使い、無ければタスクの開始時刻を使います。
  pub fn started_at(&self, task: Option<&TaskEntry>) -> Option<DateTime<Utc>> {
    self.started_at.or_else(|| task.and_then(|task| task.started_at))
  }

  /// 最終ログ時刻を決める。
  ///
  /// 応答の `lastLogAt` を最優先し、無いときだけ、時刻を持つステップの記録の最新と現在ステップの
  /// 開始時刻の新しい方を使います。どちらも無ければ run の開始時刻です。
  pub fn resolve_last_log_at(&self, started_at: Option<DateTime<Utc>>) -> Option<DateTime<Utc>> {
    let latest_step = self
      .step_logs
      .iter()
      .map(|log| log.timestamp)
      .filter(|timestamp| *timestamp != DateTime::<Utc>::UNIX_EPOCH)
      .max();
    self.last_log_at.or(latest_step.into_iter().chain(self.current_step_started_at).max()).or(started_at)
  }

  /// ステップの記録から、本体ログの行と同じ形（「時刻 種類 ステップ」）の行を作る。
  ///
  /// ローカルの集計が本体ログの記録から作る行のうち、takt_get_run の stepLogs から再現できる
  /// のは step_complete の行だけです。時刻は RFC 3339（UTC は Z）で、時刻が無い記録は時刻を
  /// 省き、ステップ名が無い記録はステップ名を省きます（ローカルの行の作り方と同じです）。
  pub fn log_lines(&self) -> Vec<String> {
    self
      .step_logs
      .iter()
      .map(|log| {
        let timestamp = (log.timestamp != DateTime::<Utc>::UNIX_EPOCH)
          .then(|| log.timestamp.to_rfc3339_opts(SecondsFormat::AutoSi, true));
        let step = (!log.step_name.is_empty()).then(|| log.step_name.clone());
        [timestamp, Some("step_complete".to_owned()), step].into_iter().flatten().collect::<Vec<_>>().join(" ")
      })
      .collect()
  }

  /// ログの末尾の行。ローカルの集計と同じく、最後の20行です。
  pub fn log_tail_lines(&self) -> Vec<String> {
    let lines = self.log_lines();
    lines.iter().skip(lines.len().saturating_sub(LOG_TAIL_RECORDS)).cloned().collect()
  }

  /// ログの末尾。ローカルの集計（RunLogView::log_tail）と同じく、末尾の行を改行でつないだ
  /// ものです。
  pub fn log_tail(&self) -> String {
    self.log_tail_lines().join("\n")
  }
}

/// takt_get_run の応答（先頭ページ）と、全ページから集めたステップの記録から run を作る。
pub(super) fn parse_remote_run(
  value: &Value,
  steps: Vec<RunStepEntry>,
  step_logs: Vec<StepLogEntry>,
) -> Result<RemoteRun, ToolError> {
  let run_slug = domain_value("run slug", RunSlug::try_from(string_field(value, "runSlug")?.as_str()))?;
  let task_text = optional_string(value, "taskText").unwrap_or_default();
  let summary = optional_string(value, "summary").unwrap_or_else(|| task_summary(&task_text));
  let started_at = match parse_time(value.get("startedAt"))? {
    | Some(time) => Some(time),
    | None => parse_time(value.get("startTime"))?,
  };
  Ok(RemoteRun {
    run_slug,
    session_id: optional_string(value, "sessionId").unwrap_or_default(),
    summary,
    task_text,
    workflow: optional_string(value, "workflow").filter(|workflow| !workflow.is_empty()),
    status: parse_run_status(value.get("status"))?,
    current_step: optional_string(value, "currentStep"),
    current_iteration: optional_i32(value, "currentIteration"),
    phase: optional_i32(value, "phase"),
    started_at,
    current_step_started_at: parse_time(value.get("currentStepStartedAt"))?,
    current_step_execution_count: optional_i32(value, "currentStepExecutionCount"),
    steps,
    step_logs,
    reports: report_summaries(value),
    instructions: Vec::new(),
    last_log_at: parse_time(value.get("lastLogAt"))?,
  })
}

/// 任意の整数フィールドを読む。i32 に収まらない値は無いものとして扱います。
fn optional_i32(value: &Value, key: &str) -> Option<i32> {
  value.get(key).and_then(Value::as_i64).and_then(|value| i32::try_from(value).ok())
}

/// run の状態を既知の値だけから解析する。欠落と未知の値は解析エラーにします。
fn parse_run_status(value: Option<&Value>) -> Result<RunStatus, ToolError> {
  let text =
    value.and_then(Value::as_str).ok_or_else(|| ToolError::Parse("run の status が文字列ではありません".to_owned()))?;
  domain_value("run の status", RunStatus::try_from(text))
}

/// 応答の stepLogs 配列から、一覧のステップを出現順に取り出す。
///
/// 開始時刻は `startedAt` を優先し、無ければ `timestamp` で補い、どちらも無ければ `UNIX_EPOCH`
/// に落とします。takt の応答に `model` は無いため、あれば読み、無ければ空です。
pub(super) fn step_entries(value: &Value) -> Vec<RunStepEntry> {
  value
    .get("stepLogs")
    .and_then(Value::as_array)
    .map(|logs| {
      logs
        .iter()
        .map(|log| RunStepEntry {
          step_name:  optional_string(log, "step").unwrap_or_default(),
          model:      optional_string(log, "model").unwrap_or_default(),
          started_at: parse_time(log.get("startedAt"))
            .ok()
            .flatten()
            .or_else(|| parse_time(log.get("timestamp")).ok().flatten())
            .unwrap_or(DateTime::<Utc>::UNIX_EPOCH),
          ended_at:   parse_time(log.get("endedAt")).ok().flatten(),
        })
        .collect()
    })
    .unwrap_or_default()
}

/// 応答の stepLogs 配列から、run 詳細のステップの記録を出現順に取り出す。
pub(super) fn step_log_entries(value: &Value) -> Vec<StepLogEntry> {
  value
    .get("stepLogs")
    .and_then(Value::as_array)
    .map(|logs| {
      logs
        .iter()
        .map(|log| StepLogEntry {
          step_name: optional_string(log, "step").unwrap_or_default(),
          persona:   optional_string(log, "persona").unwrap_or_default(),
          status:    optional_string(log, "status").unwrap_or_default(),
          content:   optional_string(log, "content").unwrap_or_default(),
          timestamp: parse_time(log.get("timestamp")).ok().flatten().unwrap_or(DateTime::<Utc>::UNIX_EPOCH),
          iteration: optional_i32(log, "iteration").unwrap_or_default(),
        })
        .collect()
    })
    .unwrap_or_default()
}

/// 応答の reports 配列からレポートの一覧（名前と大きさ）を出現順に取り出す。
fn report_summaries(value: &Value) -> Vec<ReportEntry> {
  value
    .get("reports")
    .and_then(Value::as_array)
    .map(|reports| {
      reports
        .iter()
        .map(|report| ReportEntry {
          filename:   optional_string(report, "filename").unwrap_or_default(),
          size_bytes: report.get("sizeBytes").and_then(Value::as_i64).unwrap_or_default(),
        })
        .collect()
    })
    .unwrap_or_default()
}

/// stepLogPage.nextOffset を取り出す。null または欠落なら None（追従の終わり）です。
pub(super) fn step_log_next_offset(value: &Value) -> Option<usize> {
  value
    .get("stepLogPage")
    .and_then(|page| page.get("nextOffset"))
    .and_then(Value::as_u64)
    .and_then(|offset| usize::try_from(offset).ok())
}

/// takt_get_run の view report が1回で返す本文の上限（バイト）。takt-mcp の入力契約で
/// maxBytes は 4〜4096 です。
const REPORT_PAGE_MAX_BYTES: u64 = 4096;
/// takt_get_run の view report に渡せる maxBytes の下限（バイト）。
const REPORT_PAGE_MIN_BYTES: u64 = 4;

impl ToolSetCore<'_> {
  /// takt_get_run の view report から、run のレポート `filename` の本文を先頭から `max_bytes`
  /// まで読む。
  ///
  /// takt_get_run は1回の応答で最大 [`REPORT_PAGE_MAX_BYTES`] バイトの本文を
  /// `report.{content,totalBytes,nextOffset}` で返すため、nextOffset をたどって連結します。
  /// 対象のレポートが無いと takt-mcp は道具エラーを返します。応答に `report` が無い、型が違う、
  /// または nextOffset が進まない場合は解析エラーにします。
  pub async fn read_remote_report(
    &mut self,
    run_slug: &RunSlug,
    filename: &ReportFilename,
    max_bytes: u64,
  ) -> Result<RunContent, ToolError> {
    let mut content = String::new();
    let mut offset = 0_u64;
    loop {
      let remaining = max_bytes.saturating_sub(content.len() as u64);
      let value = self
        .call_json(
          "takt_get_run",
          json!({
            "runSlug": run_slug.as_str(),
            "view": "report",
            "report": filename.as_str(),
            "offset": offset,
            "maxBytes": remaining.clamp(REPORT_PAGE_MIN_BYTES, REPORT_PAGE_MAX_BYTES),
          }),
        )
        .await?;
      let page = parse_report_page(&value)?;
      content.push_str(&page.content);
      let next = match page.next_offset {
        | Some(next) if next <= offset || page.content.is_empty() => {
          return Err(ToolError::Parse("takt_get_run の report の nextOffset が進みません".to_owned()));
        },
        | next => next,
      };
      let collected = content.len() as u64;
      match next {
        | Some(next) if collected + REPORT_PAGE_MIN_BYTES <= max_bytes => offset = next,
        | _ => {
          truncate_at_char_boundary(&mut content, max_bytes);
          let truncated = (content.len() as u64) < page.total_bytes;
          return Ok(RunContent { content, size_bytes: page.total_bytes, truncated });
        },
      }
    }
  }
}

/// takt_get_run の view report が返す、レポート本文の1ページ。
struct ReportPage {
  content:     String,
  /// レポート全体の大きさ（バイト）。
  total_bytes: u64,
  /// 続きの読み位置。最後のページでは `None` です。
  next_offset: Option<u64>,
}

/// takt_get_run の view report の応答から `report.{content,totalBytes,nextOffset}` を読む。
///
/// `report` が無い、または各項目の型が違う場合は解析エラーにします。
fn parse_report_page(value: &Value) -> Result<ReportPage, ToolError> {
  let report = value
    .get("report")
    .filter(|report| report.is_object())
    .ok_or_else(|| ToolError::Parse("takt_get_run の応答に report がありません".to_owned()))?;
  let content = string_field(report, "content")?;
  let total_bytes = report
    .get("totalBytes")
    .and_then(Value::as_u64)
    .ok_or_else(|| ToolError::Parse("report の totalBytes が数値ではありません".to_owned()))?;
  let next_offset = match report.get("nextOffset") {
    | None | Some(Value::Null) => None,
    | Some(next) => {
      Some(next.as_u64().ok_or_else(|| ToolError::Parse("report の nextOffset が数値ではありません".to_owned()))?)
    },
  };
  Ok(ReportPage { content, total_bytes, next_offset })
}

/// 本文を `max_bytes` 以内の文字の境界で切り詰める。
fn truncate_at_char_boundary(content: &mut String, max_bytes: u64) {
  let mut end = usize::try_from(max_bytes).unwrap_or(usize::MAX).min(content.len());
  while !content.is_char_boundary(end) {
    end -= 1;
  }
  content.truncate(end);
}

/// takt_get_run の view instructions に渡す limit。takt の入力契約で limit は 1〜50 です。
const INSTRUCTIONS_PAGE_LIMIT: u64 = 50;
/// takt_get_run の view task に渡す maxBytes。view report と同じく、takt の入力契約の
/// 上限 4096 に合わせます。
const TASK_PAGE_MAX_BYTES: u64 = 4096;

impl ToolSetCore<'_> {
  /// takt_get_run の view instructions から、run の指示書の一覧（要約）を全ページ読む。
  ///
  /// 応答は `{ instructions: [{instructionId, issuedAt, state}], nextOffset }` で、nextOffset を
  /// たどって連結します。nextOffset が進まない場合と、追従がページ数の上限に達した場合は
  /// 解析エラーにします。本文は含みません。本文は GetRunContent が扱います。
  pub async fn read_remote_instructions(&mut self, run_slug: &RunSlug) -> Result<Vec<Instruction>, ToolError> {
    let mut instructions = Vec::new();
    let mut offset = 0_u64;
    let mut pages_taken = 0_usize;
    loop {
      let value = self
        .call_json(
          "takt_get_run",
          json!({
            "runSlug": run_slug.as_str(),
            "view": "instructions",
            "offset": offset,
            "limit": INSTRUCTIONS_PAGE_LIMIT,
          }),
        )
        .await?;
      instructions.extend(instruction_summaries(&value)?);
      match instructions_next_offset(&value)? {
        | Some(next) if next > offset => {
          pages_taken += 1;
          if pages_taken >= super::MAX_PAGES {
            return Err(ToolError::Parse("takt_get_run の instructions 追従が上限に達しました".to_owned()));
          }
          offset = next;
        },
        | Some(_) => {
          return Err(ToolError::Parse("takt_get_run の instructions の nextOffset が進みません".to_owned()));
        },
        | None => return Ok(instructions),
      }
    }
  }

  /// takt_get_run の view task から、run のタスク本文を全ページ読む。
  ///
  /// 要約の応答はタスク本文を含まない（taskOmission だけを返す）ため、詳細の読み取りは
  /// この view で本文を取り直します。応答は `{ task: {content, nextOffset} }` で、nextOffset を
  /// たどって連結します。nextOffset が進まない場合は解析エラーにし、追従がページ数の上限に
  /// 達したら、そこまでの本文を返します。
  pub async fn read_remote_task_text(&mut self, run_slug: &RunSlug) -> Result<String, ToolError> {
    let mut content = String::new();
    let mut offset = 0_u64;
    let mut pages_taken = 0_usize;
    loop {
      let value = self
        .call_json(
          "takt_get_run",
          json!({ "runSlug": run_slug.as_str(), "view": "task", "offset": offset, "maxBytes": TASK_PAGE_MAX_BYTES }),
        )
        .await?;
      let page = parse_task_page(&value)?;
      content.push_str(&page.content);
      match page.next_offset {
        | Some(next) if next > offset => {
          pages_taken += 1;
          if pages_taken >= super::MAX_PAGES {
            return Ok(content);
          }
          offset = next;
        },
        | Some(_) => {
          return Err(ToolError::Parse("takt_get_run の task の nextOffset が進みません".to_owned()));
        },
        | None => return Ok(content),
      }
    }
  }
}

/// takt_get_run の view instructions（instructionId 指定）が1回で返す本文の上限
/// （バイト）。view report と同じく、takt の入力契約で maxBytes は 4〜4096 です。
const INSTRUCTION_PAGE_MAX_BYTES: u64 = 4096;
/// takt_get_run の view instructions（instructionId 指定）に渡せる maxBytes の下限
/// （バイト）。
const INSTRUCTION_PAGE_MIN_BYTES: u64 = 4;

impl ToolSetCore<'_> {
  /// takt_get_run の view instructions から、run の指示書 `instruction_id` の本文を
  /// 先頭から `max_bytes` まで読む。
  ///
  /// 応答は `{ instruction: {content, totalBytes, nextOffset} }` で、view report と
  /// 同じく nextOffset をたどって連結します。対象の指示書が無いと takt-mcp は道具
  /// エラーを返します。応答に `instruction` が無い、型が違う、または nextOffset が
  /// 進まない場合は解析エラーにします。
  pub async fn read_remote_instruction(
    &mut self,
    run_slug: &RunSlug,
    instruction_id: InstructionId,
    max_bytes: u64,
  ) -> Result<RunContent, ToolError> {
    let mut content = String::new();
    let mut offset = 0_u64;
    loop {
      let remaining = max_bytes.saturating_sub(content.len() as u64);
      let value = self
        .call_json(
          "takt_get_run",
          json!({
            "runSlug": run_slug.as_str(),
            "view": "instructions",
            "instructionId": instruction_id.get(),
            "offset": offset,
            "maxBytes": remaining.clamp(INSTRUCTION_PAGE_MIN_BYTES, INSTRUCTION_PAGE_MAX_BYTES),
          }),
        )
        .await?;
      let page = parse_instruction_page(&value)?;
      content.push_str(&page.content);
      let next = match page.next_offset {
        | Some(next) if next <= offset || page.content.is_empty() => {
          return Err(ToolError::Parse("takt_get_run の instruction の nextOffset が進みません".to_owned()));
        },
        | next => next,
      };
      let collected = content.len() as u64;
      match next {
        | Some(next) if collected + INSTRUCTION_PAGE_MIN_BYTES <= max_bytes => offset = next,
        | _ => {
          truncate_at_char_boundary(&mut content, max_bytes);
          let truncated = (content.len() as u64) < page.total_bytes;
          return Ok(RunContent { content, size_bytes: page.total_bytes, truncated });
        },
      }
    }
  }
}

/// takt_get_run の view instructions（instructionId 指定）が返す、指示書本文の1ページ。
struct InstructionPage {
  content:     String,
  /// 指示書全体の大きさ（バイト）。
  total_bytes: u64,
  /// 続きの読み位置。最後のページでは `None` です。
  next_offset: Option<u64>,
}

/// takt_get_run の view instructions（instructionId 指定）の応答から
/// `instruction.{content,totalBytes,nextOffset}` を読む。
///
/// `instruction` が無い、または各項目の型が違う場合は解析エラーにします
/// （view report と同じ規則）。
fn parse_instruction_page(value: &Value) -> Result<InstructionPage, ToolError> {
  let instruction = value
    .get("instruction")
    .filter(|instruction| instruction.is_object())
    .ok_or_else(|| ToolError::Parse("takt_get_run の応答に instruction がありません".to_owned()))?;
  let content = string_field(instruction, "content")?;
  let total_bytes = instruction
    .get("totalBytes")
    .and_then(Value::as_u64)
    .ok_or_else(|| ToolError::Parse("instruction の totalBytes が数値ではありません".to_owned()))?;
  let next_offset = match instruction.get("nextOffset") {
    | None | Some(Value::Null) => None,
    | Some(next) => {
      Some(next.as_u64().ok_or_else(|| ToolError::Parse("instruction の nextOffset が数値ではありません".to_owned()))?)
    },
  };
  Ok(InstructionPage { content, total_bytes, next_offset })
}

/// 応答の instructions 配列から、指示書の要約を出現順に取り出す。応答に配列が無ければ
/// 空の一覧です。
fn instruction_summaries(value: &Value) -> Result<Vec<Instruction>, ToolError> {
  let Some(entries) = value.get("instructions").and_then(Value::as_array) else {
    return Ok(Vec::new());
  };
  entries.iter().map(parse_instruction_summary).collect()
}

/// 指示書の要約の1件を読む。ID は検証つきの値オブジェクトへ変換し、状態は既知の名前だけを
/// 受け付けます。発行時刻が無い・解釈できない場合はステップの記録と同じく UNIX_EPOCH に
/// 落とします。
fn parse_instruction_summary(entry: &Value) -> Result<Instruction, ToolError> {
  let instruction_id =
    domain_value("指示書の ID", InstructionId::try_from(checked_i32(entry.get("instructionId"), "instructionId")?))?;
  let issued_at = parse_time(entry.get("issuedAt"))?.unwrap_or(DateTime::<Utc>::UNIX_EPOCH);
  let state = parse_instruction_state(entry.get("state"))?;
  Ok(Instruction { instruction_id, issued_at, state, content: String::new() })
}

/// takt の live intervention の状態の名前を、指示書の状態へ変換する。欠落と未知の名前は
/// 解析エラーにします（run の status と同じ規則）。
fn parse_instruction_state(value: Option<&Value>) -> Result<InstructionState, ToolError> {
  let text = value
    .and_then(Value::as_str)
    .ok_or_else(|| ToolError::Parse("指示書の state が文字列ではありません".to_owned()))?;
  match text {
    | "pending" => Ok(InstructionState::Pending),
    | "deliveredSameSession" => Ok(InstructionState::DeliveredSameSession),
    | "deliveredNextStep" => Ok(InstructionState::DeliveredNextStep),
    | "unconsumedWarned" => Ok(InstructionState::UnconsumedWarned),
    | _ => Err(ToolError::Parse(format!("指示書の state が未知の値です: {text}"))),
  }
}

/// view instructions の応答の nextOffset を取り出す。null または欠落なら None（追従の
/// 終わり）です。
fn instructions_next_offset(value: &Value) -> Result<Option<u64>, ToolError> {
  match value.get("nextOffset") {
    | None | Some(Value::Null) => Ok(None),
    | Some(next) => Ok(Some(
      next.as_u64().ok_or_else(|| ToolError::Parse("instructions の nextOffset が数値ではありません".to_owned()))?,
    )),
  }
}

/// takt_get_run の view task が返す、タスク本文の1ページ。
struct TaskPage {
  content:     String,
  /// 続きの読み位置。最後のページでは `None` です。
  next_offset: Option<u64>,
}

/// takt_get_run の view task の応答から `task.{content,nextOffset}` を読む。
///
/// `task` が無い、または各項目の型が違う場合は解析エラーにします（view report と同じ規則）。
fn parse_task_page(value: &Value) -> Result<TaskPage, ToolError> {
  let task = value
    .get("task")
    .filter(|task| task.is_object())
    .ok_or_else(|| ToolError::Parse("takt_get_run の応答に task がありません".to_owned()))?;
  let content = string_field(task, "content")?;
  let next_offset = match task.get("nextOffset") {
    | None | Some(Value::Null) => None,
    | Some(next) => {
      Some(next.as_u64().ok_or_else(|| ToolError::Parse("task の nextOffset が数値ではありません".to_owned()))?)
    },
  };
  Ok(TaskPage { content, next_offset })
}

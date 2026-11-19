//! takt-mcp の読み取り道具を型付きで呼び出し、応答テキストを model 型へ解析する。
//!
//! 応答は `{<key>:[...], nextOffset, ...}` のページ形式です。nextOffset が null になるまで
//! 追従して全件を集めます。道具が isError を返した場合は [`ToolError::Tool`] として扱い、
//! 輸送エラーとは区別します。

use std::{
  collections::{BTreeMap, BTreeSet, HashMap},
  path::PathBuf,
  time::Duration,
};

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::{
  client::{CallError, McpClient, McpClientConfig, TransportError},
  metrics::{ToolCallMetrics, ToolCallOutcome},
};
use crate::{
  config::McpServer,
  domain::{GoalId, RunSlug},
  model::{ExecutionStatus, GoalEntry, GoalStatus, NotificationEntry, ProjectSnapshot, RunEntry, TerminalReads},
};

mod goal;
mod goal_read;
mod omissions;
mod parse;
mod run;
mod run_follow;
mod snapshot;

use self::{
  goal::{
    is_missing_goal_field, parse_goal, parse_goal_execution_status, parse_goal_question, parse_goal_status,
    parse_goal_work_log, parse_inquiry, parse_notification, parse_task, parse_task_detail, prev_inquiries,
    registration_summary_text, resolve_awaiting_merge_at, resolve_inquiry, ParsedInquiry, TaskLinkFields,
  },
  parse::{domain_value, optional_string, required_i32, required_string, string_field, Page},
  run::{parse_remote_run, step_entries, step_log_entries, step_log_next_offset},
};
pub use self::{
  goal_read::{ApproveGoalRegistrationResult, MergeGoalPrResult},
  run::RemoteRun,
};

/// 一覧読み取り（ゴール・タスク）の期限。応答が大きく、2026-11-17 の本番実測では
/// 高負荷のときに takt_list_goals の応答が最大で約15秒、takt_list_tasks が15秒の
/// 期限切れになり、読み取りサイクル全体が失敗しました。初期化の期限（60秒）と
/// 同じ予算まで広げ、遅れてもサイクルを失敗させないようにします。
const LIST_READ_TIMEOUT: Duration = Duration::from_secs(60);

/// takt-mcp 子プロセスの起動設定を作る。
///
/// poller の定期読み取りと、書き込み・詳細の RPC が要求時に使う子プロセスが、同じ起動条件
/// （タイムアウトを含む）を共有するために公開します。
pub fn client_config(server: &McpServer, cwd: &std::path::Path) -> McpClientConfig {
  McpClientConfig {
    command:         server.command.clone(),
    args:            server.args.clone(),
    env:             server.env.clone(),
    cwd:             cwd.to_path_buf(),
    request_timeout: Duration::from_secs(15),
    tool_timeouts:   BTreeMap::from([
      ("takt_list_goals".to_owned(), LIST_READ_TIMEOUT),
      ("takt_list_tasks".to_owned(), LIST_READ_TIMEOUT),
    ]),
  }
}

/// 読み取り道具の呼び出しで起きる失敗。
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
  /// 子プロセスの起動失敗・タイムアウト・不正な応答・プロセス死亡を表す輸送エラー。
  #[error(transparent)]
  Transport(#[from] super::client::TransportError),
  /// 道具が isError を返した失敗。子プロセスは生存して応答しているため接続断ではありません。
  #[error(transparent)]
  Tool(#[from] super::client::ToolCallError),
  #[error("takt-mcp の応答を解析できませんでした: {0}")]
  Parse(String),
}

impl From<CallError> for ToolError {
  fn from(error: CallError) -> Self {
    match error {
      | CallError::Transport(error) => ToolError::Transport(error),
      | CallError::Tool(error) => ToolError::Tool(error),
    }
  }
}

/// 輸送エラーのうち期限切れかどうか。
pub(super) fn is_timeout(error: &ToolError) -> bool {
  matches!(error, ToolError::Transport(TransportError::Timeout(_)))
}

/// 補助読み取りの失敗のうち、前回値へ縮退してよい失敗かどうか。
///
/// 道具エラーは従来どおり縮退します。輸送の期限切れも、高負荷で1件の補助読み取りが
/// 遅れただけでプロジェクト全体を失敗させないため、縮退の対象に加えます。それ以外の
/// 輸送エラー（プロセス終了・起動失敗など）は接続自体が壊れている兆候として、
/// 従来どおり全体の失敗にします。
pub(super) fn degrades_to_previous(error: &ToolError) -> bool {
  matches!(error, ToolError::Tool(_)) || is_timeout(error)
}

/// 1プロジェクト分の状態を読み取るための道具のまとまり。
///
/// takt-mcp の読み取り道具は入力引数に絶対パスの `cwd` を必須とするため、この道具のまとまりは
/// 対象プロジェクトの `cwd` を保持し、すべての tools/call の引数へ渡します。
pub struct ToolSet<'a> {
  client:    &'a mut McpClient,
  cwd:       PathBuf,
  metrics:   ToolCallMetrics,
  workflows: HashMap<String, Option<crate::workflow_progress::WorkflowDefinition>>,
}

/// ページ形式の一覧を全件集めるための上限。無限追従を避けます。
pub(super) const MAX_PAGES: usize = 100;
/// takt のゴール仕様の記法。takt_get_goal は省略した大きい値だけを omissions に載せるため、
/// 仕様の有無を omissions で判定せず、既知の記法を順に読みます。
pub(super) const SPECIFICATION_NOTATIONS: [&str; 3] = ["gherkin", "quint", "alloy"];

impl<'a> ToolSet<'a> {
  pub fn new(client: &'a mut McpClient, cwd: PathBuf) -> Self {
    ToolSet { client, cwd, metrics: ToolCallMetrics::default(), workflows: HashMap::new() }
  }

  /// この道具のまとまりが記録した呼び出しの集計を取り出す。
  pub(crate) fn take_metrics(&mut self) -> ToolCallMetrics {
    std::mem::take(&mut self.metrics)
  }

  /// `cwd` を含む tools/call の引数を組み立てる。
  pub(super) fn arguments(&self, extra: Value) -> Value {
    let mut object = serde_json::Map::new();
    object.insert("cwd".to_owned(), Value::String(self.cwd.display().to_string()));
    if let Value::Object(extra) = extra {
      object.extend(extra);
    }
    Value::Object(object)
  }
}

/// 道具呼び出しの結果を集計用の結果へ分類する。応答の解析エラーは道具呼び出し自体の結果では
/// ないため、ここでは扱いません。
pub(super) fn call_outcome(result: &Result<String, CallError>) -> ToolCallOutcome {
  match result {
    | Ok(_) => ToolCallOutcome::Succeeded,
    | Err(CallError::Tool(_)) => ToolCallOutcome::Failed,
    | Err(CallError::Transport(TransportError::Timeout(_))) => ToolCallOutcome::TimedOut,
    | Err(CallError::Transport(_)) => ToolCallOutcome::Failed,
  }
}

/// 終端（`status == Completed` または `execution_status == Aborted`）のゴールか。
pub(super) fn is_terminal(goal: &GoalEntry) -> bool {
  goal.status == GoalStatus::Completed || goal.execution_status == ExecutionStatus::Aborted
}

/// 補助読み取りの項目 `item` を省ける、前回と同じ状態の終端ゴールかを判定する。
///
/// 前回の同じゴールが同じ終端状態で、かつ前回その状態でこの項目を読めていた場合だけ省けます。
/// 初回観測、状態変化、`AwaitingMerge` などの非終端、前回の終端状態で読み取りに失敗した項目は
/// 読み取り対象のままです。問い合わせへの回答の後に前回の記録から外されたゴールも、
/// 質問を読み直します。
pub(super) fn is_unchanged_terminal_goal(
  goal: &GoalEntry,
  prev: Option<&ProjectSnapshot>,
  item: impl Fn(&TerminalReads) -> &BTreeSet<GoalId>,
) -> bool {
  let Some(prev) = prev else {
    return false;
  };
  let Some(previous) = prev.goals.iter().find(|entry| entry.goal_id == goal.goal_id) else {
    return false;
  };
  previous.status == goal.status
    && previous.execution_status == goal.execution_status
    && is_terminal(goal)
    && item(&prev.terminal_reads).contains(&goal.goal_id)
}

/// 前回 snapshot の同じ runSlug の run を返す。前回が無ければ `None` です。
pub(super) fn previous_run(prev: Option<&ProjectSnapshot>, run_slug: &RunSlug) -> Option<RunEntry> {
  prev.and_then(|snapshot| snapshot.runs.iter().find(|entry| entry.run_slug == run_slug.as_str())).cloned()
}

/// 前回 snapshot の同じゴールの通知と、マージ待ちの起点時刻を引き継ぐ。前回が無ければ何もしません。
pub(super) fn inherit_previous_notifications(
  notifications: &mut Vec<NotificationEntry>,
  awaiting_merge_at: &mut BTreeMap<GoalId, DateTime<Utc>>,
  goal: &GoalEntry,
  prev: Option<&ProjectSnapshot>,
) {
  let Some(prev) = prev else {
    return;
  };
  notifications.extend(prev.notifications.iter().filter(|entry| entry.goal_id == goal.goal_id).cloned());
  if let Some(recorded_at) =
    prev.goals.iter().find(|entry| entry.goal_id == goal.goal_id).and_then(|entry| entry.awaiting_merge_at)
  {
    awaiting_merge_at.insert(goal.goal_id.clone(), recorded_at);
  }
}

#[cfg(test)]
mod tests {
  //! 道具呼び出しの計時境界と、結果分類を固定する。
  //!
  //! 計時境界は mock MCP サーバの起動と initialize を遅らせ、読み取り全体の経過時間と
  //! 道具別の所要時間を同時に観測して確かめます。

  use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
  };

  use super::{call_outcome, ToolError, ToolSet};
  use crate::{
    mcp::{
      client::{CallError, McpClient, McpClientConfig, ToolCallError, TransportError},
      metrics::{ToolCallMetrics, ToolCallOutcome},
    },
    run_progress::run_log_handle,
  };

  fn mock_script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("mock_mcp_server.mjs")
  }

  fn write_state(directory: &Path) -> PathBuf {
    let path = directory.join("state.json");
    std::fs::write(&path, "{}").expect("状態ファイルを書けます");
    path
  }

  fn config(directory: &Path, command: &str, args: Vec<String>, timeout_secs: u64) -> McpClientConfig {
    McpClientConfig {
      command: command.to_owned(),
      args,
      env: Default::default(),
      cwd: directory.to_path_buf(),
      request_timeout: Duration::from_secs(timeout_secs),
      tool_timeouts: Default::default(),
    }
  }

  fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
  }

  /// mock サーバの起動と initialize を `delay` 秒遅らせる設定を作る。
  fn delayed_config(directory: &Path, delay: &str) -> McpClientConfig {
    let state = write_state(directory);
    let script = mock_script();
    let command = format!(
      "sleep {delay}; exec node {} --state {}",
      shell_quote(&script.display().to_string()),
      shell_quote(&state.display().to_string())
    );
    config(directory, "sh", vec!["-c".to_owned(), command], 5)
  }

  #[tokio::test]
  async fn initial_call_duration_excludes_process_start_and_initialize() {
    let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
    let mut client = McpClient::new(delayed_config(directory.path(), "1.5"));
    let mut tools = ToolSet::new(&mut client, directory.path().to_path_buf());

    let started = Instant::now();
    tools
      .read_snapshot(None, &run_log_handle(directory.path().to_path_buf()))
      .await
      .expect("起動の遅延後も読み取れます");
    let elapsed = started.elapsed();
    let metrics = tools.take_metrics();
    drop(tools);

    let goals = metrics.stats("takt_list_goals").expect("takt_list_goals の集計があります");
    assert_eq!(goals.calls, 1);
    assert!(elapsed >= Duration::from_millis(1500), "起動と initialize の遅延を含む経過時間です: {elapsed:?}");
    assert!(goals.max < Duration::from_millis(1000), "道具呼び出しの計時に起動と initialize を含めません: {goals:?}");
    client.shutdown().await;
  }

  #[tokio::test]
  async fn start_failure_is_not_recorded_as_a_tool_call() {
    let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
    let config = config(directory.path(), "this-command-does-not-exist", Vec::new(), 1);
    let mut client = McpClient::new(config);
    let mut tools = ToolSet::new(&mut client, directory.path().to_path_buf());

    let error = tools
      .read_snapshot(None, &run_log_handle(directory.path().to_path_buf()))
      .await
      .expect_err("起動できない client は読み取れません");
    let metrics = tools.take_metrics();
    drop(tools);

    assert!(
      matches!(error, ToolError::Transport(TransportError::Spawn(_) | TransportError::Io(_))),
      "起動失敗は輸送エラーです: {error}"
    );
    assert_eq!(metrics.total_calls(), 0, "起動失敗を道具呼び出しとして記録しません");
    assert_eq!(metrics.total_failures(), 0);
  }

  #[tokio::test]
  async fn exit_before_initialize_reply_is_not_recorded_as_a_tool_call() {
    let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
    // initialize の要求を読んだ後、応答せずに終了する子プロセスです。
    let config = config(directory.path(), "sh", vec!["-c".to_owned(), "read -r _line; exit 0".to_owned()], 1);
    let mut client = McpClient::new(config);
    let mut tools = ToolSet::new(&mut client, directory.path().to_path_buf());

    let error = tools
      .read_snapshot(None, &run_log_handle(directory.path().to_path_buf()))
      .await
      .expect_err("initialize に応答がなければ読み取れません");
    let metrics = tools.take_metrics();
    drop(tools);

    assert!(
      matches!(error, ToolError::Transport(TransportError::ProcessExited)),
      "initialize の前の終了は輸送エラーです: {error}"
    );
    assert_eq!(metrics.total_calls(), 0, "起動中の終了を道具呼び出しとして記録しません");
    assert_eq!(metrics.total_failures(), 0, "起動中の終了を道具の失敗として記録しません");
    assert_eq!(metrics.total_timeouts(), 0, "起動中の終了を道具のタイムアウトとして記録しません");
  }

  #[test]
  fn classifies_tool_call_results_into_success_failure_and_timeout() {
    assert_eq!(call_outcome(&Ok("{}".to_owned())), ToolCallOutcome::Succeeded);
    assert_eq!(
      call_outcome(&Err(CallError::Tool(ToolCallError {
        tool:    "takt_list_goals".to_owned(),
        message: "失敗しました".to_owned(),
      }))),
      ToolCallOutcome::Failed
    );
    assert_eq!(call_outcome(&Err(CallError::Transport(TransportError::Timeout(15)))), ToolCallOutcome::TimedOut);
    assert_eq!(
      call_outcome(&Err(CallError::Transport(TransportError::Io("切断".to_owned())))),
      ToolCallOutcome::Failed
    );

    let mut metrics = ToolCallMetrics::default();
    let timeout = Err(CallError::Transport(TransportError::Timeout(15)));
    metrics.record("takt_list_goals", Duration::from_millis(4), call_outcome(&timeout));
    assert_eq!(metrics.total_timeouts(), 1);
    assert_eq!(metrics.total_failures(), 0, "タイムアウトを失敗に混ぜません");
  }
}

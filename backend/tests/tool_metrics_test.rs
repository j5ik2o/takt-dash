//! takt-mcp の道具呼び出し計測の集計を、模した入力から検証する。

use std::time::Duration;

use takt_dash_backend::mcp::{ToolCallMetrics, ToolCallOutcome};

fn record(metrics: &mut ToolCallMetrics, tool: &str, millis: u64, outcome: ToolCallOutcome) {
  metrics.record(tool, Duration::from_millis(millis), outcome);
}

#[test]
fn aggregates_calls_total_max_and_p95_per_tool() {
  let mut metrics = ToolCallMetrics::default();
  record(&mut metrics, "takt_list_goals", 10, ToolCallOutcome::Succeeded);
  record(&mut metrics, "takt_list_goals", 30, ToolCallOutcome::Succeeded);
  record(&mut metrics, "takt_list_goals", 20, ToolCallOutcome::Succeeded);
  record(&mut metrics, "takt_list_tasks", 5, ToolCallOutcome::Succeeded);
  record(&mut metrics, "takt_list_tasks", 15, ToolCallOutcome::Succeeded);

  let goals = metrics.stats("takt_list_goals").expect("計測した道具の集計があります");
  assert_eq!(goals.tool, "takt_list_goals");
  assert_eq!(goals.calls, 3);
  assert_eq!(goals.failures, 0);
  assert_eq!(goals.timeouts, 0);
  assert_eq!(goals.total, Duration::from_millis(60));
  assert_eq!(goals.max, Duration::from_millis(30));
  assert_eq!(goals.p95, Duration::from_millis(30), "3回の p95 は昇順ソート後の最大値です");

  let tasks = metrics.stats("takt_list_tasks").expect("計測した道具の集計があります");
  assert_eq!(tasks.calls, 2);
  assert_eq!(tasks.total, Duration::from_millis(20));
  assert_eq!(tasks.max, Duration::from_millis(15));
  assert_eq!(tasks.p95, Duration::from_millis(15));

  assert_eq!(metrics.total_calls(), 5);
  assert!(metrics.stats("takt_get_run").is_none(), "計測していない道具は集計を持ちません");
}

#[test]
fn p95_of_a_single_call_is_its_duration() {
  let mut metrics = ToolCallMetrics::default();
  record(&mut metrics, "takt_get_goal", 7, ToolCallOutcome::Succeeded);

  let stat = metrics.stats("takt_get_goal").expect("集計があります");
  assert_eq!(stat.p95, Duration::from_millis(7));
}

#[test]
fn p95_of_three_calls_is_the_sorted_last_value() {
  let mut metrics = ToolCallMetrics::default();
  record(&mut metrics, "takt_get_goal", 7, ToolCallOutcome::Succeeded);
  record(&mut metrics, "takt_get_goal", 1, ToolCallOutcome::Succeeded);
  record(&mut metrics, "takt_get_goal", 4, ToolCallOutcome::Succeeded);

  let stat = metrics.stats("takt_get_goal").expect("集計があります");
  assert_eq!(stat.p95, Duration::from_millis(7), "入力順ではなく昇順ソート後の ceil(0.95*3)-1 番目です");
}

#[test]
fn p95_of_twenty_calls_is_the_nineteenth_sorted_value() {
  let mut metrics = ToolCallMetrics::default();
  for millis in (1..=20).rev() {
    record(&mut metrics, "takt_get_run", millis, ToolCallOutcome::Succeeded);
  }

  let stat = metrics.stats("takt_get_run").expect("集計があります");
  assert_eq!(stat.p95, Duration::from_millis(19), "昇順ソート後の ceil(0.95*20)-1 番目です");
  assert_eq!(stat.max, Duration::from_millis(20));
}

#[test]
fn merges_project_metrics_before_computing_p95() {
  let mut first = ToolCallMetrics::default();
  for millis in 1..=10 {
    record(&mut first, "takt_list_goals", millis, ToolCallOutcome::Succeeded);
  }
  let mut second = ToolCallMetrics::default();
  for millis in 11..=20 {
    record(&mut second, "takt_list_goals", millis, ToolCallOutcome::Succeeded);
  }

  first.merge(second);

  let stat = first.stats("takt_list_goals").expect("統合した集計があります");
  assert_eq!(stat.calls, 20);
  assert_eq!(stat.total, Duration::from_millis(210));
  assert_eq!(stat.max, Duration::from_millis(20));
  assert_eq!(
    stat.p95,
    Duration::from_millis(19),
    "プロジェクトごとの p95 の平均(15)や最大(20)ではなく、全件を統合した19番目の値です"
  );
}

#[test]
fn counts_timeouts_and_failures_separately() {
  let mut metrics = ToolCallMetrics::default();
  record(&mut metrics, "takt_get_goal", 4, ToolCallOutcome::Succeeded);
  record(&mut metrics, "takt_get_goal", 6, ToolCallOutcome::Failed);
  record(&mut metrics, "takt_get_goal", 10, ToolCallOutcome::TimedOut);
  record(&mut metrics, "takt_get_run", 5, ToolCallOutcome::TimedOut);

  let goal = metrics.stats("takt_get_goal").expect("集計があります");
  assert_eq!(goal.calls, 3);
  assert_eq!(goal.failures, 1);
  assert_eq!(goal.timeouts, 1);
  assert_eq!(goal.max, Duration::from_millis(10));
  assert_eq!(goal.p95, Duration::from_millis(10));

  let run = metrics.stats("takt_get_run").expect("集計があります");
  assert_eq!(run.calls, 1);
  assert_eq!(run.failures, 0, "タイムアウトを failures に含めません");
  assert_eq!(run.timeouts, 1);

  assert_eq!(metrics.total_calls(), 4);
  assert_eq!(metrics.total_failures(), 1, "タイムアウトを failures に含めません");
  assert_eq!(metrics.total_timeouts(), 2);
}

#[test]
fn displays_every_per_tool_field_in_tool_name_order() {
  let mut metrics = ToolCallMetrics::default();
  record(&mut metrics, "takt_list_tasks", 4, ToolCallOutcome::TimedOut);
  record(&mut metrics, "takt_list_goals", 10, ToolCallOutcome::Succeeded);
  record(&mut metrics, "takt_list_goals", 30, ToolCallOutcome::Failed);

  assert_eq!(
    metrics.to_string(),
    "takt_list_goals(calls=2, failures=1, timeouts=0, total_ms=40, max_ms=30, p95_ms=30), \
     takt_list_tasks(calls=1, failures=0, timeouts=1, total_ms=4, max_ms=4, p95_ms=4)",
    "道具エントリは道具名順に、回数・失敗・タイムアウト・合計・最大・p95 をすべて出力します"
  );
}

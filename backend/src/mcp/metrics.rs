//! takt-mcp の道具呼び出しを道具名ごとに集計する。
//!
//! 1回ごとの所要時間と結果を保持し、呼び出し回数・失敗・タイムアウト・合計・最大・p95 を返します。
//! p95 は昇順に並べた所要時間の `ceil(0.95 * n) - 1` 番目（0始まり）です。集計済みの値を
//! 平均するのではなく、1回ごとの所要時間を統合してから算出するため、複数プロジェクト分の
//! マージ後も周期全体の p95 になります。

use std::{collections::BTreeMap, fmt, time::Duration};

/// 道具呼び出し1回の結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCallOutcome {
  Succeeded,
  Failed,
  TimedOut,
}

#[derive(Debug, Clone, Copy)]
struct ToolCall {
  duration: Duration,
  outcome:  ToolCallOutcome,
}

/// 1つの道具の集計値。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallStat {
  pub tool:     String,
  pub calls:    usize,
  pub failures: usize,
  pub timeouts: usize,
  pub total:    Duration,
  pub max:      Duration,
  pub p95:      Duration,
}

/// 道具呼び出しの記録を道具名ごとに保持し、集計値を返す。
#[derive(Debug, Default)]
pub struct ToolCallMetrics {
  calls: BTreeMap<String, Vec<ToolCall>>,
}

impl ToolCallMetrics {
  /// 1回の呼び出しを記録する。
  pub fn record(&mut self, tool: &str, duration: Duration, outcome: ToolCallOutcome) {
    self.calls.entry(tool.to_owned()).or_default().push(ToolCall { duration, outcome });
  }

  /// 別の集計（別プロジェクト分など）を統合する。
  ///
  /// p95 を周期全体で算出できるよう、集計済みの値ではなく1回ごとの所要時間を統合します。
  pub fn merge(&mut self, other: ToolCallMetrics) {
    for (tool, calls) in other.calls {
      self.calls.entry(tool).or_default().extend(calls);
    }
  }

  /// 道具名ごとの集計を返す。記録が無い道具は `None`。
  pub fn stats(&self, tool: &str) -> Option<ToolCallStat> {
    self.calls.get(tool).map(|calls| summarize(tool, calls))
  }

  /// 記録した道具を名前順に集計して返す。
  pub fn summaries(&self) -> Vec<ToolCallStat> {
    self.calls.iter().map(|(tool, calls)| summarize(tool, calls)).collect()
  }

  pub fn total_calls(&self) -> usize {
    self.calls.values().map(Vec::len).sum()
  }

  pub fn total_failures(&self) -> usize {
    self.calls.values().flatten().filter(|call| call.outcome == ToolCallOutcome::Failed).count()
  }

  pub fn total_timeouts(&self) -> usize {
    self.calls.values().flatten().filter(|call| call.outcome == ToolCallOutcome::TimedOut).count()
  }
}

impl fmt::Display for ToolCallMetrics {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    for (index, stat) in self.summaries().iter().enumerate() {
      if index > 0 {
        f.write_str(", ")?;
      }
      let ToolCallStat { tool, calls, failures, timeouts, total, max, p95 } = stat;
      let total_ms = total.as_millis();
      let max_ms = max.as_millis();
      let p95_ms = p95.as_millis();
      write!(f, "{tool}(calls={calls}, failures={failures}, timeouts={timeouts}, ")?;
      write!(f, "total_ms={total_ms}, max_ms={max_ms}, p95_ms={p95_ms})")?;
    }
    Ok(())
  }
}

fn summarize(tool: &str, calls: &[ToolCall]) -> ToolCallStat {
  let mut durations: Vec<Duration> = calls.iter().map(|call| call.duration).collect();
  durations.sort_unstable();
  ToolCallStat {
    tool:     tool.to_owned(),
    calls:    calls.len(),
    failures: calls.iter().filter(|call| call.outcome == ToolCallOutcome::Failed).count(),
    timeouts: calls.iter().filter(|call| call.outcome == ToolCallOutcome::TimedOut).count(),
    total:    durations.iter().sum(),
    max:      durations.last().copied().unwrap_or_default(),
    p95:      percentile_95(&durations),
  }
}

/// 昇順に並んだ所要時間から p95 を選ぶ。`index = ceil(0.95 * n) - 1`（0始まり）。
fn percentile_95(sorted: &[Duration]) -> Duration {
  let index = (sorted.len() * 95).div_ceil(100).saturating_sub(1);
  sorted[index]
}

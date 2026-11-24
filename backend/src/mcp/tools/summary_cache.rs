//! PR を持つゴールの summary（path なしの takt_get_goal の応答）を、要求をまたいで短い間覚える。
//!
//! takt は path なしの takt_get_goal のたびに PR の状態を gh で読みます。画面の詳細は1秒ごとに
//! 読み直すため、そのまま呼ぶと開いている間ずっと毎秒 gh を呼び、GitHub の上限を使い切ります。
//! PR を持たないゴールは gh を呼ばないので覚えず、状態を毎回読みます。

use std::{
  collections::HashMap,
  path::PathBuf,
  sync::{LazyLock, Mutex, MutexGuard},
  time::{Duration, Instant},
};

use serde_json::Value;

use crate::domain::GoalId;

/// PR を持つゴールの summary を覚えておく期間。
pub(super) const GOAL_SUMMARY_TTL: Duration = Duration::from_secs(180);

/// 覚えた summary の鍵。接続先が変わったら、同じ cwd とゴールでも旧接続先の値を使いません。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct GoalSummaryKey {
  pub(super) connection: String,
  pub(super) cwd:        PathBuf,
  pub(super) goal_id:    GoalId,
}

/// 鍵ごとの、読んだ時刻と path なしの takt_get_goal の応答。
#[derive(Default)]
pub(super) struct GoalSummaryCache {
  entries: HashMap<GoalSummaryKey, (Instant, Value)>,
}

impl GoalSummaryCache {
  /// `now` の時点で期限内の summary を返す。
  pub(super) fn get(&self, key: &GoalSummaryKey, now: Instant) -> Option<Value> {
    self.entries.get(key).filter(|(read_at, _)| is_fresh(*read_at, now)).map(|(_, value)| value.clone())
  }

  /// 読んだ summary を覚える。PR
  /// を持たない応答は覚えず、前の値も捨てます。期限切れの値もここで捨てます。
  pub(super) fn record(&mut self, key: GoalSummaryKey, now: Instant, value: &Value) {
    self.entries.retain(|_, (read_at, _)| is_fresh(*read_at, now));
    if value.pointer("/goal/pullRequest").is_some() {
      self.entries.insert(key, (now, value.clone()));
    } else {
      self.entries.remove(&key);
    }
  }
}

fn is_fresh(read_at: Instant, now: Instant) -> bool {
  now.saturating_duration_since(read_at) < GOAL_SUMMARY_TTL
}

static GOAL_SUMMARIES: LazyLock<Mutex<GoalSummaryCache>> = LazyLock::new(Default::default);

/// 要求をまたいで使う summary の記憶。
pub(super) fn goal_summaries() -> MutexGuard<'static, GoalSummaryCache> {
  GOAL_SUMMARIES.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
  //! 期限と鍵の境界を、時刻を渡して確かめる。

  use std::{path::PathBuf, time::Duration};

  use serde_json::{json, Value};

  use super::{GoalSummaryCache, GoalSummaryKey, GOAL_SUMMARY_TTL};
  use crate::domain::GoalId;

  fn key(connection: &str) -> GoalSummaryKey {
    GoalSummaryKey {
      connection: connection.to_owned(),
      cwd:        PathBuf::from("/abs/takt-dash"),
      goal_id:    GoalId::try_from("g1").expect("有効なゴール ID です"),
    }
  }

  fn summary_with_pull_request(status: &str) -> Value {
    json!({ "goal": { "id": "g1", "status": status, "pullRequest": { "number": 42 } } })
  }

  #[test]
  fn returns_the_recorded_summary_until_the_ttl_and_a_new_one_after_it() {
    let mut cache = GoalSummaryCache::default();
    let start = std::time::Instant::now();
    cache.record(key("a"), start, &summary_with_pull_request("created"));

    let before_expiry = start + GOAL_SUMMARY_TTL - Duration::from_secs(1);
    assert_eq!(cache.get(&key("a"), before_expiry), Some(summary_with_pull_request("created")));

    let expired = start + GOAL_SUMMARY_TTL;
    assert_eq!(cache.get(&key("a"), expired), None, "期限が来たら読み直します");

    cache.record(key("a"), expired, &summary_with_pull_request("completed"));
    assert_eq!(cache.get(&key("a"), expired), Some(summary_with_pull_request("completed")), "読み直した値を返します");
  }

  #[test]
  fn does_not_reuse_a_summary_read_from_another_connection() {
    let mut cache = GoalSummaryCache::default();
    let now = std::time::Instant::now();
    cache.record(key("old"), now, &summary_with_pull_request("created"));

    assert_eq!(cache.get(&key("new"), now), None);
  }

  #[test]
  fn forgets_the_summary_once_the_goal_has_no_pull_request() {
    let mut cache = GoalSummaryCache::default();
    let now = std::time::Instant::now();
    cache.record(key("a"), now, &summary_with_pull_request("created"));
    cache.record(key("a"), now, &json!({ "goal": { "id": "g1", "status": "created" } }));

    assert_eq!(cache.get(&key("a"), now), None);
  }
}

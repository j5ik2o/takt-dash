//! ゴールごとの通知の読み取り。前回と同じ終端ゴールは読み取りを省いて前回の通知を引き継ぐ。

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde_json::json;

use super::{
  goal::parse_notification,
  parse::Page,
  previous::{degrades_to_previous, inherit_previous_notifications, is_terminal, is_unchanged_terminal_goal},
  ToolError, ToolSetCore, MAX_PAGES,
};
use crate::{
  domain::GoalId,
  model::{GoalEntry, NotificationEntry, ProjectSnapshot},
};

impl<'a> ToolSetCore<'a> {
  /// ゴールごとの通知と、マージ待ちの起点時刻を集める。あわせて、終端状態で通知を読めたゴール
  /// （読み取りを省いたゴールを含む）を返します。
  pub(super) async fn list_notifications(
    &mut self,
    goals: &[GoalEntry],
    prev: Option<&ProjectSnapshot>,
  ) -> Result<(Vec<NotificationEntry>, BTreeMap<GoalId, DateTime<Utc>>, BTreeSet<GoalId>), ToolError> {
    let mut notifications = Vec::new();
    let mut awaiting_merge_at = BTreeMap::new();
    let mut read_in_terminal = BTreeSet::new();
    for goal in goals {
      if is_unchanged_terminal_goal(goal, prev, |reads| &reads.notifications) {
        inherit_previous_notifications(&mut notifications, &mut awaiting_merge_at, goal, prev);
        read_in_terminal.insert(goal.goal_id.clone());
        continue;
      }
      match self.list_goal_notifications(&goal.goal_id).await {
        | Ok((goal_notifications, goal_awaiting_merge_at)) => {
          if let Some(recorded_at) = goal_awaiting_merge_at {
            awaiting_merge_at.insert(goal.goal_id.clone(), recorded_at);
          }
          notifications.extend(goal_notifications);
          if is_terminal(goal) {
            read_in_terminal.insert(goal.goal_id.clone());
          }
        },
        | Err(error) if degrades_to_previous(&error) => {
          inherit_previous_notifications(&mut notifications, &mut awaiting_merge_at, goal, prev);
        },
        | Err(error) => return Err(error),
      }
    }
    Ok((notifications, awaiting_merge_at, read_in_terminal))
  }

  /// 1 ゴールの通知を nextOffset が null になるまで追従して集める。あわせて、そのゴールの
  /// awaiting_merge 通知のうち最も新しい recordedAt を返す。
  async fn list_goal_notifications(
    &mut self,
    goal_id: &GoalId,
  ) -> Result<(Vec<NotificationEntry>, Option<DateTime<Utc>>), ToolError> {
    let mut notifications = Vec::new();
    let mut awaiting_merge_at: Option<DateTime<Utc>> = None;
    let mut offset = 0;
    for _ in 0..MAX_PAGES {
      let value = self
        .call_json("takt_get_goal", json!({ "goalId": goal_id.as_str(), "path": "notifications", "offset": offset }))
        .await?;
      let page = Page::from_value(&value, "items")?;
      for item in &page.items {
        let notification = parse_notification(goal_id, item, &page.shortened)?;
        if notification.kind == "awaiting_merge"
          && awaiting_merge_at.is_none_or(|current| notification.recorded_at > current)
        {
          awaiting_merge_at = Some(notification.recorded_at);
        }
        notifications.push(notification);
      }
      match page.next_offset {
        | Some(next) => offset = next,
        | None => return Ok((notifications, awaiting_merge_at)),
      }
    }
    Err(ToolError::Parse("takt_get_goal の notices 追従が上限に達しました".to_owned()))
  }
}

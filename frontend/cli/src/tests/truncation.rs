//! 切り詰めの印（`name_truncated`・`body_truncated`）のある名前と本文に、省略記号を 1 つ付ける
//! 表示（issue #230）。印の無い名前と本文は、文の途中で終わっていても変えない。

use super::fixtures::{sample_goals, sample_overview};
use crate::{
  domain::{ProjectName, ServerId},
  proto::v1::GetGoalsResponse,
  render::{render_decisions, render_goals},
  render_overview,
};

const QA_CUT: &str = "土台と完成申請の宛先は、プルリク";

#[test]
fn overview_marks_truncated_goal_names_and_decision_bodies_once() {
  let mut state = sample_overview();
  state.goals[0].name = QA_CUT.to_owned();
  state.goals[0].name_truncated = true;
  state.pending_decisions[0].body = QA_CUT.to_owned();
  state.pending_decisions[0].body_truncated = true;

  let rendered = render_overview(&state).expect("有効です");

  assert_eq!(rendered.matches(&format!("{QA_CUT}…")).count(), rendered.matches(QA_CUT).count(), "{rendered}");
  assert_eq!(rendered.matches(QA_CUT).count(), 2, "ゴールの行と判断待ちの行に出ます: {rendered}");
  assert!(!rendered.contains("……"), "{rendered}");
}

#[test]
fn overview_leaves_names_and_bodies_without_the_mark_unchanged() {
  let mut state = sample_overview();
  state.goals[0].name = QA_CUT.to_owned();
  state.pending_decisions[0].body = QA_CUT.to_owned();

  let rendered = render_overview(&state).expect("有効です");

  assert!(rendered.contains(QA_CUT), "{rendered}");
  assert!(!rendered.contains(&format!("{QA_CUT}…")), "印が無ければ省略記号を付けません: {rendered}");
}

#[test]
fn goals_and_decisions_commands_mark_truncated_text() {
  let mut goals = sample_goals();
  goals[0].name = QA_CUT.to_owned();
  goals[0].name_truncated = true;
  let rendered = render_goals(&GetGoalsResponse { goals });
  assert!(rendered.contains(&format!("「{QA_CUT}…」")), "{rendered}");

  let mut state = sample_overview();
  let decision = &mut state.pending_decisions[0];
  decision.body = QA_CUT.to_owned();
  decision.body_truncated = true;
  let server_id = ServerId::try_from(decision.server_id.as_str()).expect("有効です");
  let project = ProjectName::try_from(decision.project.as_str()).expect("有効です");
  let rendered = render_decisions(&state, &server_id, &project).expect("有効です");
  assert!(rendered.contains(&format!("{QA_CUT}…")), "{rendered}");
}

//! 進行図を proto の公開表現へ写す。

use super::mapping::optional_timestamp;
use crate::{
  takt_dash::v1,
  workflow_progress::{ExecutionCount, NodeState, WorkflowProgress},
};

fn count(value: Option<ExecutionCount>) -> Option<v1::StepExecutionCount> {
  value.map(|value| v1::StepExecutionCount { value: value.into() })
}

pub(super) fn progress(value: &WorkflowProgress) -> v1::WorkflowProgress {
  v1::WorkflowProgress {
    initial_step:     value.definition.initial_step.clone(),
    definition:       value
      .definition
      .nodes
      .iter()
      .map(|node| v1::WorkflowDefinitionNode {
        name:  node.name.clone(),
        kind:  node.kind.clone(),
        rules: node.next.iter().map(|next| v1::WorkflowRule { next: next.clone() }).collect(),
      })
      .collect(),
    nodes:            value
      .nodes
      .iter()
      .map(|node| v1::WorkflowProgressNode {
        name:            node.name.clone(),
        state:           match node.state {
          | NodeState::Unknown => v1::WorkflowNodeState::Unknown,
          | NodeState::Unvisited => v1::WorkflowNodeState::Unvisited,
          | NodeState::Running => v1::WorkflowNodeState::Running,
          | NodeState::Completed => v1::WorkflowNodeState::Completed,
          | NodeState::Skipped => v1::WorkflowNodeState::Skipped,
          | NodeState::Failed => v1::WorkflowNodeState::Failed,
          | NodeState::Aborted => v1::WorkflowNodeState::Aborted,
        } as i32,
        execution_count: count(node.count),
        started_at:      optional_timestamp(node.started_at),
        active_children: node.children.clone(),
        log_index:       node.log_index.and_then(|index| u32::try_from(index).ok()),
      })
      .collect(),
    edges:            value
      .edges
      .iter()
      .map(|edge| v1::WorkflowProgressEdge {
        source:     edge.source.clone(),
        target:     edge.target.clone(),
        rule_index: u32::try_from(edge.rule_index).expect("規則番号はページ上限以内です"),
        traversals: count(edge.traversals),
        backward:   edge.backward,
      })
      .collect(),
    connections:      value
      .connections
      .iter()
      .map(|connection| v1::WorkflowProgressConnection {
        source:    connection.source.clone(),
        target:    connection.target.clone(),
        traversed: connection.traversed,
      })
      .collect(),
    backbone:         value.backbone.clone(),
    loops:            value
      .loops
      .iter()
      .map(|repair| v1::WorkflowRepairLoop {
        source:          repair.source.clone(),
        target:          repair.target.clone(),
        steps:           repair.steps.clone(),
        execution_count: count(repair.count),
      })
      .collect(),
    other_steps:      value.others.clone(),
    current_step:     value.current_step.clone(),
    terminal:         value.terminal.clone(),
    current_position: value
      .position()
      .and_then(|index| u32::try_from(index).ok())
      .map(|value| v1::WorkflowPosition { value }),
  }
}

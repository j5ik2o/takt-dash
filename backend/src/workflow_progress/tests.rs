use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::{
  model::RunStatus,
  workflow_progress::{
    build_progress, DefinitionNode, ExecutionCount, ExecutionEvent, ExecutionEvidence, NodeState, StepExecution,
    WorkflowDefinition,
  },
};

fn definition(nodes: &[(&str, &[&str])]) -> WorkflowDefinition {
  WorkflowDefinition {
    initial_step: nodes.first().map(|node| node.0.to_owned()),
    nodes:        nodes
      .iter()
      .map(|(name, next)| DefinitionNode {
        name:       (*name).to_owned(),
        kind:       "agent".to_owned(),
        conditions: vec![None; next.len()],
        next:       next.iter().map(|next| Some((*next).to_owned())).collect(),
      })
      .collect(),
  }
}

fn evidence(counts: &[(&str, u32)], records: &[Value]) -> ExecutionEvidence {
  ExecutionEvidence {
    counts:           Some(
      counts
        .iter()
        .map(|(step, count)| StepExecution {
          identity: format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&(Vec::<String>::new(), "outer-ref", *step)).unwrap())
          ),
          workflow: "outer".to_owned(),
          step:     (*step).to_owned(),
          count:    ExecutionCount::try_from(*count).unwrap(),
        })
        .collect(),
    ),
    events:           records.iter().filter_map(|record| ExecutionEvent::from_record(record, Some(0))).collect(),
    history_complete: true,
  }
}

fn record(kind: &str, step: &str, rule: Option<usize>) -> Value {
  json!({ "type": kind, "step": step, "workflow": "outer", "matchedRuleIndex": rule,
    "stack": [{ "step": step, "workflow": "outer", "workflow_ref": "outer-ref" }] })
}

#[test]
fn call_return_and_rule_evidence_are_compared_independently() {
  let mut graph = definition(&[("develop", &["gate", "replan"]), ("gate", &["COMPLETE"]), ("replan", &["develop"])]);
  graph.nodes[0].conditions = vec![Some("COMPLETE".into()), Some("need_replan".into())];
  for (returned, rule, expected) in
    [(None, None, 0), (Some("need_replan"), None, 1), (None, Some(0), 0), (Some("need_replan"), Some(1), 1)]
  {
    let mut call = record("workflow_call_complete", "develop", rule);
    call["workflow"] = json!("outer-ref");
    call["status"] = json!("completed");
    call["returnValue"] = json!(returned);
    let result = build_progress(graph.clone(), &evidence(&[], &[call]), "outer", Some("gate"), RunStatus::Running);
    assert_eq!(result.nodes[0].state, NodeState::Completed);
    assert_eq!(result.edges[expected].traversals, Some(ExecutionCount::try_from(1).unwrap()));
    assert_eq!(result.edges[1 - expected].traversals, Some(ExecutionCount::try_from(0).unwrap()));
  }
}

#[test]
fn shorter_success_path_wins_over_an_earlier_success_path() {
  let graph = definition(&[
    ("start", &["long", "short"]),
    ("long", &["extra"]),
    ("extra", &["COMPLETE"]),
    ("short", &["COMPLETE"]),
  ]);
  let result = build_progress(graph.clone(), &evidence(&[], &[]), "outer", None, RunStatus::Running);
  assert_eq!(result.backbone, vec!["start", "short"]);
  let mut same_length = graph;
  same_length.nodes[1].next = vec![Some("COMPLETE".into())];
  assert_eq!(build_progress(same_length, &evidence(&[], &[]), "outer", None, RunStatus::Running).backbone, vec![
    "start", "long"
  ]);
}

#[test]
fn shortest_success_path_ignores_an_earlier_longer_repair_route() {
  let graph = definition(&[
    ("plan", &["fix", "coding"]),
    ("fix", &["plan"]),
    ("coding", &["review"]),
    ("review", &["supervise"]),
    ("supervise", &["COMPLETE"]),
  ]);
  let result = build_progress(graph, &evidence(&[], &[]), "outer", None, RunStatus::Running);
  assert_eq!(result.backbone, ["plan", "coding", "review", "supervise"]);
  assert_eq!(result.loops[0].source, "plan");
  assert_eq!(result.loops[0].target, "plan");
}

#[test]
fn equal_length_success_paths_follow_rule_order() {
  let graph = definition(&[("plan", &["second", "first"]), ("first", &["COMPLETE"]), ("second", &["COMPLETE"])]);
  assert_eq!(build_progress(graph, &evidence(&[], &[]), "outer", None, RunStatus::Running).backbone, [
    "plan", "second"
  ]);
}

#[test]
fn six_repairs_share_one_loop_and_a_different_return_target_stays_separate() {
  let graph = definition(&[
    ("start", &["develop"]),
    ("develop", &["gate"]),
    ("gate", &["fmt", "clippy", "test", "types", "lint", "proto", "audit"]),
    ("audit", &["audit_fix", "COMPLETE"]),
    ("fmt", &["gate"]),
    ("clippy", &["gate"]),
    ("test", &["gate"]),
    ("types", &["gate"]),
    ("lint", &["gate"]),
    ("proto", &["gate"]),
    ("audit_fix", &["gate"]),
  ]);
  let result = build_progress(
    graph,
    &evidence(&[("fmt", 1), ("test", 1)], &[record("step_start", "fmt", None)]),
    "outer",
    Some("fmt"),
    RunStatus::Running,
  );
  assert_eq!(result.backbone, ["start", "develop", "gate", "audit"]);
  assert_eq!(result.loops.len(), 2);
  let repair = result.loops.iter().find(|repair| repair.source == "gate").unwrap();
  assert_eq!(repair.steps.len(), 6);
  assert_eq!(repair.count.map(u32::from), Some(2));
  assert_eq!(result.loops.iter().find(|repair| repair.source == "audit").unwrap().target, "gate");
  assert_eq!(result.position(), Some(3));
  assert_eq!(result.nodes.iter().find(|node| node.name == "gate").unwrap().state, NodeState::Running);
}

#[test]
fn node_counts_use_meta_even_when_the_log_contains_only_one_start() {
  let result = build_progress(
    definition(&[("gate", &["COMPLETE"])]),
    &evidence(&[("gate", 3)], &[record("step_start", "gate", None)]),
    "outer",
    Some("gate"),
    RunStatus::Running,
  );
  assert_eq!(result.nodes[0].count.map(u32::from), Some(3));
  assert_eq!(result.nodes[0].state, NodeState::Running);
}

#[test]
fn terminal_states_and_unvisited_nodes_are_distinct() {
  for (status, expected, terminal) in [
    (RunStatus::Failed, NodeState::Failed, "FAILED"),
    (RunStatus::Aborted, NodeState::Aborted, "ABORT"),
    (RunStatus::Completed, NodeState::Completed, "COMPLETE"),
  ] {
    let result = build_progress(
      definition(&[("gate", &["COMPLETE", "ABORT"]), ("unused", &[])]),
      &evidence(&[("gate", 1)], &[]),
      "outer",
      Some("gate"),
      status,
    );
    assert_eq!(result.nodes[0].state, expected);
    assert_eq!(result.nodes[1].state, NodeState::Skipped);
    assert_eq!(result.terminal.as_deref(), Some(terminal));
    assert!(result.edges.iter().all(|edge| edge.target != "ABORT"));
  }
  let result =
    build_progress(definition(&[("gate", &["COMPLETE"])]), &evidence(&[], &[]), "outer", None, RunStatus::Running);
  assert_eq!(result.nodes[0].state, NodeState::Unvisited);
}

#[test]
fn top_level_matched_rule_marks_only_its_edge_and_counts_actual_returns() {
  let graph = definition(&[("gate", &["fix", "COMPLETE"]), ("fix", &["gate"])]);
  let records = [
    record("step_complete", "gate", Some(0)),
    record("step_complete", "fix", Some(0)),
    record("step_complete", "fix", Some(0)),
  ];
  let result = build_progress(graph, &evidence(&[], &records), "outer", None, RunStatus::Running);
  assert_eq!(
    result.edges.iter().find(|edge| edge.source == "gate" && edge.target == "fix").unwrap().traversals.map(u32::from),
    Some(1)
  );
  assert_eq!(result.edges.iter().find(|edge| edge.source == "fix").unwrap().traversals.map(u32::from), Some(2));
  assert_eq!(result.edges.iter().find(|edge| edge.target == "COMPLETE").unwrap().traversals.map(u32::from), Some(0));
}

#[test]
fn a_child_with_the_same_gate_name_does_not_mark_the_outer_edge() {
  let mut child = record("step_complete", "gate", Some(0));
  child["stack"] = json!([{ "step": "develop", "workflow": "outer", "workflow_ref": "outer-ref" }, { "step": "gate", "workflow": "child" }]);
  child["workflow"] = json!("child");
  let result = build_progress(
    definition(&[("gate", &["fix", "COMPLETE"]), ("fix", &["gate"])]),
    &evidence(&[], &[child]),
    "outer",
    None,
    RunStatus::Running,
  );
  assert_eq!(result.edges.iter().find(|edge| edge.target == "fix").unwrap().traversals.map(u32::from), Some(0));
}

#[test]
fn active_child_is_attached_to_its_parent_and_completed_children_are_removed() {
  let mut plan = record("step_start", "plan", None);
  plan["workflow"] = json!("child");
  plan["stack"] = json!([{ "step": "develop", "workflow": "outer", "workflow_ref": "outer-ref" }, { "step": "plan", "workflow": "child" }]);
  let mut done = plan.clone();
  done["type"] = json!("step_complete");
  let mut coding = plan.clone();
  coding["step"] = json!("coding");
  coding["stack"][1]["step"] = json!("coding");
  let graph = definition(&[("develop", &["COMPLETE"])]);
  let first = build_progress(
    graph.clone(),
    &evidence(&[("develop", 1)], &[plan.clone()]),
    "outer",
    Some("develop"),
    RunStatus::Running,
  );
  assert_eq!(first.nodes[0].children, ["plan"]);
  let next = build_progress(
    graph,
    &evidence(&[("develop", 1)], &[plan, done, coding]),
    "outer",
    Some("develop"),
    RunStatus::Running,
  );
  assert_eq!(next.nodes[0].children, ["coding"]);
  assert_eq!(next.nodes[0].count.map(u32::from), Some(1));
}

#[test]
fn parallel_participants_never_complete_an_unvisited_top_level_node() {
  for role in ["child", "direct_participant", "workflow_call_participant"] {
    let mut child = record("step_complete", "review", Some(0));
    child["parallel"] = json!({ "role": role });
    child["stack"][0]["step"] = json!("parallel");
    let result = build_progress(
      definition(&[("parallel", &["review"]), ("review", &["COMPLETE"])]),
      &evidence(&[("parallel", 1), ("review", 1)], &[child]),
      "outer",
      Some("parallel"),
      RunStatus::Running,
    );
    assert_eq!(result.nodes[1].state, NodeState::Unvisited);
    assert_eq!(result.nodes[1].count.map(u32::from), Some(0));
  }
}

#[test]
fn unknown_rules_keep_their_indices_and_missing_evidence_stays_unknown() {
  let node =
    DefinitionNode::from_value(&json!({ "name": "gate", "kind": "system", "rules": [{}, { "next": "COMPLETE" }] }))
      .unwrap();
  let graph = WorkflowDefinition { initial_step: Some("gate".to_owned()), nodes: vec![node] };
  let result = build_progress(graph, &ExecutionEvidence::default(), "outer", None, RunStatus::Running);
  assert_eq!(result.edges[0].rule_index, 1);
  assert_eq!(result.edges[0].traversals, None);
  assert_eq!(result.nodes[0].count, None);
  assert_eq!(result.nodes[0].state, NodeState::Unknown);
}

#[test]
fn unclassified_nodes_remain_visible_and_names_without_rules_have_no_edges() {
  let result = build_progress(
    definition(&[("plan", &["COMPLETE"]), ("orphan", &["missing"])]),
    &evidence(&[], &[]),
    "outer",
    None,
    RunStatus::Running,
  );
  assert_eq!(result.backbone, ["plan"]);
  assert_eq!(result.others, ["orphan"]);
  let result = build_progress(
    definition(&[("plan", &[]), ("review", &[])]),
    &ExecutionEvidence::default(),
    "outer",
    None,
    RunStatus::Running,
  );
  assert!(result.backbone.is_empty());
  assert!(result.edges.is_empty());
  assert_eq!(result.nodes.iter().map(|node| node.name.as_str()).collect::<Vec<_>>(), ["plan", "review"]);
}

#[test]
fn execution_count_rejects_values_outside_the_api_range() {
  assert_eq!(u32::from(ExecutionCount::try_from(0).unwrap()), 0);
  assert!(ExecutionCount::try_from(u32::MAX).is_err());
}

#[test]
fn actual_backbone_returns_have_counts_while_unused_returns_stay_zero() {
  let graph = definition(&[("plan", &["gate"]), ("gate", &["plan", "COMPLETE"])]);
  let result = build_progress(
    graph,
    &evidence(&[], &[record("step_complete", "gate", Some(0)), record("step_complete", "gate", Some(0))]),
    "outer",
    Some("plan"),
    RunStatus::Running,
  );
  assert_eq!(result.backbone, ["plan", "gate"]);
  let backward = result.edges.iter().find(|edge| edge.backward).unwrap();
  assert_eq!((&*backward.source, &*backward.target), ("gate", "plan"));
  assert_eq!(backward.traversals.map(u32::from), Some(2));
  assert_eq!(result.edges.iter().find(|edge| edge.target == "COMPLETE").unwrap().traversals.map(u32::from), Some(0));
}

#[test]
fn a_single_nested_count_without_root_identity_is_not_reused_as_an_outer_count() {
  let mut data = evidence(&[("gate", 3)], &[]);
  data.counts.as_mut().unwrap()[0].identity = "nested-identity".to_owned();
  let result = build_progress(definition(&[("gate", &["COMPLETE"])]), &data, "outer", None, RunStatus::Running);
  assert_eq!(result.nodes[0].count, None);
}

#[test]
fn workflow_call_lifecycle_records_do_not_replace_the_step_rule_result() {
  let result = build_progress(
    definition(&[("develop", &["fix", "COMPLETE"]), ("fix", &["develop"])]),
    &evidence(&[], &[
      record("workflow_call_start", "develop", None),
      record("workflow_call_complete", "develop", None),
      record("step_complete", "develop", Some(1)),
    ]),
    "outer",
    None,
    RunStatus::Completed,
  );
  assert_eq!(result.edges.iter().find(|edge| edge.target == "fix").unwrap().traversals.map(u32::from), Some(0));
  assert_eq!(result.edges.iter().find(|edge| edge.target == "COMPLETE").unwrap().traversals.map(u32::from), Some(1));
}

#[test]
fn connections_keep_rule_counts_and_aggregate_known_and_unknown_traversals() {
  use super::{aggregate_connections, ProgressEdge};
  for (values, expected) in [
    (vec![Some(0), Some(0)], Some(false)),
    (vec![Some(0), Some(1)], Some(true)),
    (vec![Some(0), None], None),
    (vec![Some(1), None], Some(true)),
  ] {
    let edges: Vec<_> = values
      .iter()
      .enumerate()
      .map(|(rule_index, count)| ProgressEdge {
        source: "gate".into(),
        target: "wait_ci".into(),
        rule_index,
        traversals: count.map(|n| ExecutionCount::try_from(n).unwrap()),
        backward: false,
      })
      .collect();
    let connections = aggregate_connections(&edges);
    assert_eq!(connections.len(), 1);
    assert_eq!(connections[0].traversed, expected);
    assert_eq!(edges.iter().map(|edge| edge.traversals.map(u32::from)).collect::<Vec<_>>(), values);
  }
}

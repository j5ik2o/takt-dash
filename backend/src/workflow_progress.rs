//! workflow の定義と実行証拠を結合する。表示順から遷移や到達回数を推定しない。

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::model::RunStatus;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct ExecutionCount(u32);

impl TryFrom<u32> for ExecutionCount {
  type Error = &'static str;

  fn try_from(value: u32) -> Result<Self, Self::Error> {
    if value > i32::MAX as u32 {
      return Err("実行回数が上限を超えています");
    }
    Ok(Self(value))
  }
}

impl From<ExecutionCount> for u32 {
  fn from(value: ExecutionCount) -> Self {
    value.0
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowDefinition {
  pub initial_step: Option<String>,
  pub nodes:        Vec<DefinitionNode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefinitionNode {
  pub name:       String,
  pub kind:       String,
  // 解釈できない規則も位置を保持し、matchedRuleIndex の意味を変えない。
  pub next:       Vec<Option<String>>,
  pub conditions: Vec<Option<String>>,
}

impl DefinitionNode {
  pub(crate) fn from_value(value: &Value) -> Option<Self> {
    Some(Self {
      name:       value.get("name")?.as_str()?.to_owned(),
      kind:       value.get("kind").and_then(Value::as_str).unwrap_or("agent").to_owned(),
      next:       value
        .get("rules")
        .and_then(Value::as_array)
        .map(|rules| rules.iter().map(|rule| rule.get("next").and_then(Value::as_str).map(str::to_owned)).collect())
        .unwrap_or_default(),
      conditions: value
        .get("rules")
        .and_then(Value::as_array)
        .map(|rules| {
          rules
            .iter()
            .map(|rule| {
              let condition = rule.get("condition")?;
              (condition.get("kind").and_then(Value::as_str) == Some("semantic"))
                .then(|| condition.get("label").and_then(Value::as_str).map(str::to_owned))
                .flatten()
            })
            .collect()
        })
        .unwrap_or_default(),
    })
  }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct StepExecution {
  pub identity: String,
  pub workflow: String,
  pub step:     String,
  pub count:    ExecutionCount,
}

pub(crate) fn execution_counts(raw: Option<Value>) -> Option<Vec<StepExecution>> {
  raw.and_then(|value| match serde_json::from_value(value) {
    | Ok(counts) => Some(counts),
    | Err(error) => {
      tracing::warn!(error = %error, "stepExecutions を解釈できませんでした。回数を不明として扱います。");
      None
    },
  })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionEvent {
  pub step:               String,
  pub workflow:           Option<String>,
  pub root_step:          Option<String>,
  pub root_workflow:      Option<String>,
  pub root_reference:     Option<String>,
  pub top_level:          bool,
  pub direct_participant: bool,
  pub complete:           bool,
  pub rule_evaluated:     bool,
  pub failed:             bool,
  pub aborted:            bool,
  pub rule_index:         Option<usize>,
  pub timestamp:          Option<DateTime<Utc>>,
  pub iteration:          Option<i64>,
  pub log_index:          Option<usize>,
  pub call_result:        Option<String>,
  pub invocation:         Option<String>,
  pub root_invocation:    Option<String>,
  pub participant:        Option<String>,
  pub participant_parent: Option<String>,
  pub parallel_parent:    Option<String>,
  pub call:               bool,
}

impl ExecutionEvent {
  pub(crate) fn from_record(record: &Value, log_index: Option<usize>) -> Option<Self> {
    let kind = record.get("type")?.as_str()?;
    let phase = matches!(kind, "phase_start" | "phase_complete");
    let parallel = record.get("parallel");
    if phase && parallel.and_then(|p| p.get("role")).and_then(Value::as_str) != Some("direct_participant") {
      return None;
    }
    if !phase && !matches!(kind, "step_start" | "step_complete" | "workflow_call_start" | "workflow_call_complete") {
      return None;
    }
    let stack = record.get("stack").and_then(Value::as_array);
    let root = stack.and_then(|stack| stack.first());
    let child = record
      .get("parallel")
      .filter(|value| !value.is_null())
      .is_some_and(|parallel| parallel.get("role").and_then(Value::as_str) != Some("parent"));
    Some(Self {
      step: record.get("step")?.as_str()?.to_owned(),
      // 呼出し記録の workflow は表示名でなく参照IDの場合がある。同じフレームの
      // step と参照を照合してから、そのフレームの表示名へ正規化する。
      workflow: stack
        .and_then(|frames| frames.last())
        .filter(|frame| {
          frame.get("step") == record.get("step")
            && (frame.get("workflow_ref") == record.get("workflow") || frame.get("workflow") == record.get("workflow"))
        })
        .and_then(|frame| frame.get("workflow"))
        .or_else(|| record.get("workflow"))
        .and_then(Value::as_str)
        .map(str::to_owned),
      root_step: root.and_then(|root| root.get("step")).and_then(Value::as_str).map(str::to_owned),
      root_workflow: root.and_then(|root| root.get("workflow")).and_then(Value::as_str).map(str::to_owned),
      root_reference: root.and_then(|root| root.get("workflow_ref")).and_then(Value::as_str).map(str::to_owned),
      top_level: !child && stack.is_none_or(|stack| stack.len() <= 1),
      direct_participant: !phase && child && stack.is_some_and(|stack| stack.len() == 1),
      complete: matches!(kind, "step_complete" | "workflow_call_complete" | "phase_complete"),
      rule_evaluated: matches!(kind, "step_complete" | "workflow_call_complete"),
      failed: record.get("error").is_some_and(|error| !error.is_null())
        || matches!(record.get("status").and_then(Value::as_str), Some("failed" | "error")),
      aborted: record.get("status").and_then(Value::as_str) == Some("aborted"),
      rule_index: record.get("matchedRuleIndex").and_then(Value::as_u64).and_then(|index| usize::try_from(index).ok()),
      timestamp: record
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
        .map(|time| time.with_timezone(&Utc)),
      iteration: record.get("iteration").and_then(Value::as_i64),
      log_index,
      call_result: (kind == "workflow_call_complete")
        .then(|| {
          record.get("returnValue").and_then(Value::as_str).map(str::to_owned).or_else(|| {
            match record.get("status").and_then(Value::as_str) {
              | Some("completed") => Some("COMPLETE".to_owned()),
              | Some("aborted") => Some("ABORT".to_owned()),
              | _ => None,
            }
          })
        })
        .flatten(),
      invocation: stack
        .filter(|frames| frames.last().is_some_and(|frame| frame.get("occurrence").is_some()))
        .map(|frames| Value::Array(frames.clone()).to_string()),
      root_invocation: root.filter(|frame| frame.get("occurrence").is_some()).map(Value::to_string),
      participant: if phase {
        parallel.and_then(|p| p.get("participationId")).and_then(Value::as_str).map(|id| {
          serde_json::json!([parallel.and_then(|p| p.get("parentParticipationId")), id, record.get("phase")])
            .to_string()
        })
      } else {
        None
      },
      participant_parent: parallel
        .and_then(|p| p.get("parentParticipationId"))
        .and_then(Value::as_str)
        .map(str::to_owned),
      parallel_parent: parallel
        .filter(|p| p.get("role").and_then(Value::as_str) == Some("parent"))
        .and_then(|p| p.get("participationId"))
        .and_then(Value::as_str)
        .map(str::to_owned),
      call: matches!(kind, "workflow_call_start" | "workflow_call_complete"),
    })
  }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecutionEvidence {
  pub counts:           Option<Vec<StepExecution>>,
  pub events:           Vec<ExecutionEvent>,
  pub history_complete: bool,
}

impl ExecutionEvidence {
  pub(crate) fn bind_logs(&mut self, logs: &[crate::model::StepLogEntry]) {
    for event in &mut self.events {
      event.log_index = if event.complete && !event.call && event.rule_evaluated {
        logs.iter().position(|log| {
          event.step == log.step_name
            && event.iteration == Some(i64::from(log.iteration))
            && event.timestamp == Some(log.timestamp)
        })
      } else {
        None
      };
    }
  }

  pub(crate) fn ingest(&mut self, record: &Value, log_index: Option<usize>) {
    if record.get("type").and_then(Value::as_str) == Some("workflow_start") {
      self.events.clear();
      self.history_complete = true;
    } else if let Some(event) = ExecutionEvent::from_record(record, log_index) {
      self.events.push(event);
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeState {
  Unknown,
  Unvisited,
  Running,
  Completed,
  Skipped,
  Failed,
  Aborted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgressNode {
  pub name:       String,
  pub state:      NodeState,
  pub count:      Option<ExecutionCount>,
  pub started_at: Option<DateTime<Utc>>,
  pub children:   Vec<String>,
  pub log_index:  Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgressEdge {
  pub source:     String,
  pub target:     String,
  pub rule_index: usize,
  pub traversals: Option<ExecutionCount>,
  pub backward:   bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgressConnection {
  pub source:    String,
  pub target:    String,
  pub traversed: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepairLoop {
  pub source: String,
  pub target: String,
  pub steps:  Vec<String>,
  pub count:  Option<ExecutionCount>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowProgress {
  pub definition:   WorkflowDefinition,
  pub nodes:        Vec<ProgressNode>,
  pub edges:        Vec<ProgressEdge>,
  pub connections:  Vec<ProgressConnection>,
  pub backbone:     Vec<String>,
  pub loops:        Vec<RepairLoop>,
  pub others:       Vec<String>,
  pub current_step: Option<String>,
  pub terminal:     Option<String>,
}

impl WorkflowProgress {
  pub fn position(&self) -> Option<usize> {
    let current = self.current_step.as_deref()?;
    self
      .backbone
      .iter()
      .position(|step| step == current)
      .or_else(|| {
        let repair = self.loops.iter().find(|repair| repair.steps.iter().any(|step| step == current))?;
        self.backbone.iter().position(|step| step == &repair.source)
      })
      .map(|index| index + 1)
  }
}

pub fn build_progress(
  definition: WorkflowDefinition,
  evidence: &ExecutionEvidence,
  workflow: &str,
  current_step: Option<&str>,
  status: RunStatus,
) -> WorkflowProgress {
  let backbone =
    shortest_path(&definition, definition.initial_step.as_deref(), |next| next == "COMPLETE", &BTreeSet::new())
      .unwrap_or_default();
  let spine: BTreeSet<_> = backbone.iter().cloned().collect();
  let mut groups: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
  let mut assigned = BTreeSet::new();
  for source in &backbone {
    let Some(node) = definition.nodes.iter().find(|node| &node.name == source) else { continue };
    for next in node.next.iter().flatten().filter(|next| !spine.contains(*next)) {
      if let Some(path) = shortest_path(&definition, Some(next), |next| spine.contains(next), &spine) {
        let Some(last) = path.last() else { continue };
        let Some(last_node) = definition.nodes.iter().find(|node| &node.name == last) else { continue };
        let Some(target) = last_node.next.iter().flatten().find(|next| spine.contains(*next)) else { continue };
        let steps = groups.entry((source.clone(), target.clone())).or_default();
        for step in path {
          if assigned.insert(step.clone()) {
            steps.push(step);
          }
        }
      }
    }
  }
  let top_events: Vec<_> = evidence
    .events
    .iter()
    .filter(|event| event.top_level && event.workflow.as_deref().is_none_or(|name| name == workflow))
    .collect();
  let root_reference = evidence
    .events
    .iter()
    .find(|event| event.root_workflow.as_deref() == Some(workflow))
    .and_then(|event| event.root_reference.as_deref());
  let mut nodes: Vec<_> = definition
    .nodes
    .iter()
    .map(|node| {
      let count = execution_count(evidence, workflow, &node.name, root_reference);
      let latest = top_events.iter().rev().find(|event| event.step == node.name);
      let is_current = current_step == Some(node.name.as_str());
      let state = if is_current {
        match status {
          | RunStatus::Running => NodeState::Running,
          | RunStatus::Failed => NodeState::Failed,
          | RunStatus::Aborted => NodeState::Aborted,
          | RunStatus::Completed => NodeState::Completed,
        }
      } else if latest.is_some_and(|event| event.failed) {
        NodeState::Failed
      } else if latest.is_some_and(|event| event.aborted) {
        NodeState::Aborted
      } else if latest.is_some_and(|event| event.complete) {
        NodeState::Completed
      } else if count.is_some_and(|count| count.0 == 0) {
        if status == RunStatus::Running {
          NodeState::Unvisited
        } else {
          NodeState::Skipped
        }
      } else {
        NodeState::Unknown
      };
      let mut active_children = BTreeMap::new();
      if is_current && status == RunStatus::Running {
        let mut parent_active = true;
        let mut parent_invocation = None;
        let mut closed_parallel = BTreeSet::new();
        for event in &evidence.events {
          if event.top_level && event.step == node.name {
            active_children.clear();
            closed_parallel.clear();
            parent_active = !event.complete;
            parent_invocation = event.root_invocation.as_ref();
          }
          if parent_active
            && !event.top_level
            && event.root_step.as_deref() == Some(node.name.as_str())
            && event.root_workflow.as_deref() == Some(workflow)
            && parent_invocation.is_none_or(|parent| event.root_invocation.as_ref() == Some(parent))
          {
            if let Some(parent) = &event.parallel_parent {
              active_children.retain(|_, (_, owner): &mut (String, Option<String>)| owner.as_ref() != Some(parent));
              if event.complete {
                closed_parallel.insert(parent.clone());
              } else {
                closed_parallel.remove(parent);
              }
            }
            if event.participant_parent.as_ref().is_some_and(|parent| closed_parallel.contains(parent)) {
              continue;
            }
            let key = (event.participant.clone().or_else(|| event.workflow.clone()), event.step.clone());
            if event.complete {
              active_children.remove(&key);
            } else {
              active_children.insert(key, (event.step.clone(), event.participant_parent.clone()));
            }
          }
        }
      }
      ProgressNode {
        name: node.name.clone(),
        state,
        count,
        started_at: top_events
          .iter()
          .rev()
          .find(|event| event.step == node.name && !event.complete)
          .and_then(|event| event.timestamp),
        children: active_children.into_values().map(|(step, _)| step).collect(),
        log_index: top_events
          .iter()
          .rev()
          .find(|event| event.step == node.name && event.log_index.is_some())
          .and_then(|event| event.log_index),
      }
    })
    .collect();
  let loops: Vec<_> = groups
    .into_iter()
    .map(|((source, target), steps)| {
      let count = steps
        .iter()
        .try_fold(0_u32, |total, name| total.checked_add(nodes.iter().find(|node| &node.name == name)?.count?.0))
        .and_then(|value| ExecutionCount::try_from(value).ok());
      RepairLoop { source, target, steps, count }
    })
    .collect();
  if status == RunStatus::Running {
    for repair in &loops {
      if current_step.is_some_and(|current| repair.steps.iter().any(|step| step == current)) {
        if let Some(source) = nodes.iter_mut().find(|node| node.name == repair.source) {
          source.state = NodeState::Running;
        }
      }
    }
  }
  let mut edges = Vec::new();
  for node in &definition.nodes {
    for (index, target) in node.next.iter().enumerate() {
      let Some(target) = target.as_ref().filter(|target| target.as_str() != "ABORT") else { continue };
      let completions: Vec<_> =
        top_events.iter().enumerate().filter(|(_, event)| event.rule_evaluated && event.step == node.name).collect();
      let mut decisions = Vec::new();
      let mut paired = BTreeSet::new();
      for (position, event) in completions {
        if paired.contains(&position) {
          continue;
        }
        // step_complete と workflow_call_complete が同じフレームの完了を表す場合、
        // 明示規則番号を優先して一度だけ投影する。
        let adjacent = top_events.get(position + 1).filter(|other| {
          event.invocation == other.invocation
            && other.rule_evaluated
            && other.step == event.step
            && other.call != event.call
        });
        if adjacent.is_some() {
          paired.insert(position + 1);
        }
        let explicit = event.rule_index.or_else(|| adjacent.and_then(|other| other.rule_index));
        let inferred =
          event.call_result.as_deref().or_else(|| adjacent.and_then(|other| other.call_result.as_deref())).and_then(
            |result| {
              let candidates: Vec<_> = node
                .conditions
                .iter()
                .enumerate()
                .filter(|(_, condition)| condition.as_deref() == Some(result))
                .map(|(index, _)| index)
                .collect();
              if candidates.len() == 1 {
                return Some(candidates[0]);
              }
              // 条件式を評価せず、同じ最上位フレームの後続開始で一意になる候補だけを採用する。
              let next = top_events.iter().skip(position + 1).find(|event| !event.complete)?;
              let matching: Vec<_> = candidates
                .into_iter()
                .filter(|index| node.next.get(*index).and_then(Option::as_deref) == Some(next.step.as_str()))
                .collect();
              (matching.len() == 1).then(|| matching[0])
            },
          );
        decisions.push(explicit.or(inferred).filter(|index| node.next.get(*index).is_some_and(Option::is_some)));
      }
      let traversals = decisions.iter().filter(|decision| **decision == Some(index)).count();
      let source_history_complete = evidence.history_complete && decisions.iter().all(Option::is_some);
      let traversals = if traversals > 0 || source_history_complete {
        u32::try_from(traversals).ok().and_then(|value| ExecutionCount::try_from(value).ok())
      } else {
        None
      };
      let backward = backbone
        .iter()
        .position(|name| name == &node.name)
        .zip(backbone.iter().position(|name| name == target))
        .is_some_and(|(from, to)| to <= from);
      edges.push(ProgressEdge {
        source: node.name.clone(),
        target: target.clone(),
        rule_index: index,
        traversals,
        backward,
      });
    }
  }
  let connections = aggregate_connections(&edges);
  let others = definition
    .nodes
    .iter()
    .filter(|node| !spine.contains(&node.name) && !assigned.contains(&node.name))
    .map(|node| node.name.clone())
    .collect();
  WorkflowProgress {
    definition,
    nodes,
    edges,
    connections,
    backbone,
    loops,
    others,
    current_step: current_step.map(str::to_owned),
    terminal: match status {
      | RunStatus::Completed => Some("COMPLETE".to_owned()),
      | RunStatus::Aborted => Some("ABORT".to_owned()),
      | RunStatus::Failed => Some("FAILED".to_owned()),
      | RunStatus::Running => None,
    },
  }
}

fn aggregate_connections(edges: &[ProgressEdge]) -> Vec<ProgressConnection> {
  let mut connections = Vec::new();
  for edge in edges {
    if connections
      .iter()
      .any(|connection: &ProgressConnection| connection.source == edge.source && connection.target == edge.target)
    {
      continue;
    }
    let matching: Vec<_> =
      edges.iter().filter(|other| other.source == edge.source && other.target == edge.target).collect();
    let traversed = if matching.iter().any(|edge| edge.traversals.is_some_and(|count| count.0 > 0)) {
      Some(true)
    } else if matching.iter().all(|edge| edge.traversals.is_some()) {
      Some(false)
    } else {
      None
    };
    connections.push(ProgressConnection { source: edge.source.clone(), target: edge.target.clone(), traversed });
  }
  connections
}

fn execution_count(
  evidence: &ExecutionEvidence,
  workflow: &str,
  step: &str,
  root_reference: Option<&str>,
) -> Option<ExecutionCount> {
  let counts = evidence.counts.as_ref()?;
  // 直接の並列参加者には独自の stack frame が無く、meta の識別子が同名の最上位 step と
  // 重なり得る。所属を確認できた子の回数を最上位へ転用しない。
  if evidence
    .events
    .iter()
    .any(|event| event.direct_participant && event.step == step && event.workflow.as_deref() == Some(workflow))
  {
    let has_outer = evidence
      .events
      .iter()
      .any(|event| event.top_level && event.step == step && event.workflow.as_deref() == Some(workflow));
    return if evidence.history_complete && !has_outer { Some(ExecutionCount(0)) } else { None };
  }
  let matching: Vec<_> = counts.iter().filter(|entry| entry.workflow == workflow && entry.step == step).collect();
  if matching.is_empty() {
    return Some(ExecutionCount(0));
  }
  if let Some(reference) = root_reference {
    let encoded = serde_json::to_vec(&(Vec::<String>::new(), reference, step)).ok()?;
    let identity = format!("{:x}", Sha256::digest(encoded));
    return Some(
      matching.iter().find(|entry| entry.identity == identity).map(|entry| entry.count).unwrap_or(ExecutionCount(0)),
    );
  }
  // 履歴の末尾しか読めないとき、同名の呼出箇所を一つに合算しない。
  let encoded = serde_json::to_vec(&(Vec::<String>::new(), workflow, step)).ok()?;
  let identity = format!("{:x}", Sha256::digest(encoded));
  matching.iter().find(|entry| entry.identity == identity).map(|entry| entry.count)
}

fn shortest_path(
  definition: &WorkflowDefinition,
  initial: Option<&str>,
  terminal: impl Fn(&str) -> bool,
  stop: &BTreeSet<String>,
) -> Option<Vec<String>> {
  let initial = initial?;
  let mut queue = VecDeque::from([vec![initial.to_owned()]]);
  let mut seen = BTreeSet::new();
  while let Some(path) = queue.pop_front() {
    let name = path.last()?;
    if !seen.insert(name.clone()) || stop.contains(name) {
      continue;
    }
    let Some(node) = definition.nodes.iter().find(|node| &node.name == name) else { continue };
    for next in node.next.iter().flatten() {
      if terminal(next) {
        return Some(path);
      }
      let mut extended = path.clone();
      extended.push(next.clone());
      queue.push_back(extended);
    }
  }
  None
}

#[cfg(test)]
mod tests;

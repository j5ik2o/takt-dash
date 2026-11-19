import type { WorkflowProgress, StepExecutionCount } from "../proto/taktdash";
import { toStepExecutionCount, toWorkflowPosition } from "./domain-primitives";
import { enumFromWire } from "./proto-values";
import { toDate, recordOf, asOptionalNumber, asOptionalString, arrayOf, toStringArray } from "./wire-values";

function toExecutionCount(raw: unknown): StepExecutionCount | undefined {
  if (raw === undefined || raw === null) return undefined;
  return { value: toStepExecutionCount(recordOf(raw).value ?? 0) };
}

export function restoreWorkflowProgress(raw: unknown): WorkflowProgress | undefined {
  if (raw === undefined || raw === null) return undefined;
  const value = recordOf(raw);
  return {
    initialStep: asOptionalString(value.initialStep),
    definition: arrayOf(value.definition).map((raw) => {
      const node = recordOf(raw);
      return { name: String(node.name), kind: String(node.kind), rules: arrayOf(node.rules).map((raw) => ({ next: asOptionalString(recordOf(raw).next) })) };
    }),
    nodes: arrayOf(value.nodes).map((raw) => {
      const node = recordOf(raw);
      return { name: String(node.name), state: enumFromWire("WorkflowNodeState", node.state), executionCount: toExecutionCount(node.executionCount), startedAt: toDate(node.startedAt), activeChildren: toStringArray(node.activeChildren), logIndex: asOptionalNumber(node.logIndex) };
    }),
    edges: arrayOf(value.edges).map((raw) => {
      const edge = recordOf(raw);
      return { source: String(edge.source), target: String(edge.target), ruleIndex: Number(edge.ruleIndex ?? 0), traversals: toExecutionCount(edge.traversals), backward: edge.backward === true };
    }),
    connections: arrayOf(value.connections).map((raw) => {
      const connection = recordOf(raw);
      return { source: String(connection.source), target: String(connection.target), traversed: typeof connection.traversed === "boolean" ? connection.traversed : undefined };
    }),
    backbone: toStringArray(value.backbone),
    loops: arrayOf(value.loops).map((raw) => {
      const repair = recordOf(raw);
      return { source: String(repair.source), target: String(repair.target), steps: toStringArray(repair.steps), executionCount: toExecutionCount(repair.executionCount) };
    }),
    otherSteps: toStringArray(value.otherSteps),
    currentStep: asOptionalString(value.currentStep), terminal: asOptionalString(value.terminal),
    currentPosition: value.currentPosition === undefined || value.currentPosition === null ? undefined : { value: toWorkflowPosition(recordOf(value.currentPosition).value) },
  };
}

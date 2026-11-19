import test from "node:test";
import assert from "node:assert/strict";
import { WorkflowProgress, WorkflowNodeState } from "../../src/proto/taktdash.ts";
import { restoreWorkflowProgress } from "../../src/state/workflow-progress-wire.ts";
import { DomainPrimitiveError } from "../../src/state/domain-primitives.ts";

test("進行図は proto の JSON から状態・時刻・規則順・ゼロ回・不明を復元する", () => {
  const progress = WorkflowProgress.create({
    initialStep: "gate", currentStep: "gate", currentPosition: { value: 1 }, backbone: ["gate"],
    definition: [{ name: "gate", kind: "workflow_call", rules: [{}, { next: "COMPLETE" }] }],
    nodes: [
      { name: "gate", state: WorkflowNodeState.WORKFLOW_NODE_STATE_RUNNING, executionCount: { value: 3 }, startedAt: new Date("2026-11-19T00:00:00Z"), activeChildren: ["plan"], logIndex: 0 },
      { name: "fix", state: WorkflowNodeState.WORKFLOW_NODE_STATE_UNVISITED, executionCount: { value: 0 } },
      { name: "unknown", state: WorkflowNodeState.WORKFLOW_NODE_STATE_UNKNOWN },
    ],
    connections: [{source:"gate",target:"COMPLETE",traversed:false},{source:"gate",target:"fix",traversed:true},{source:"fix",target:"gate"}],
    edges: [{ source: "gate", target: "COMPLETE", ruleIndex: 1, traversals: { value: 0 } }],
    loops: [{ source: "gate", target: "gate", steps: ["fix"], executionCount: { value: 0 } }], otherSteps: ["unknown"],
  });
  assert.deepEqual(restoreWorkflowProgress(WorkflowProgress.toJSON(progress)), progress);
  assert.deepEqual(restoreWorkflowProgress(progress), progress);
});

test("図を持たない旧応答は未指定のままにし、不正な回数・位置・状態を拒否する", () => {
  assert.equal(restoreWorkflowProgress(undefined), undefined);
  assert.equal(restoreWorkflowProgress(null), undefined);
  assert.throws(() => restoreWorkflowProgress({ nodes: [{ name: "gate", executionCount: { value: -1 } }] }), DomainPrimitiveError);
  assert.throws(() => restoreWorkflowProgress({ currentPosition: { value: 0 } }), DomainPrimitiveError);
  assert.throws(() => restoreWorkflowProgress({ nodes: [{ name: "gate", state: "UNRECOGNIZED" }] }), DomainPrimitiveError);
});

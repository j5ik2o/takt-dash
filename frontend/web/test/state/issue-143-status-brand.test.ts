// 状態種別が proto の number のまま流通しないことを、型検査で固定する（issue #143）。
//
// 復元の境界の enumFromWire は既知の値かどうかを実行時に確かめるが、返す型は number の
// ままなので、復元後の状態には未検証の数値をそのまま代入できてしまう。状態種別を
// domain-primitives.ts の RunLimit と同じ branded type（number に readonly __brand を
// 足した形）にし、その保証をここで検査する。
//
// いまの本番は number を返すため、下の 5 つの Expect は失敗する。本番を brand に変える
// まで緑にしない（キャストや本番の変更でこの検査を消さない）。
import test from "node:test";
import assert from "node:assert/strict";
import {
  ExecutionStatus as WireExecutionStatus,
  GoalStatus as WireGoalStatus,
  PendingDecisionType as WirePendingDecisionType,
  RunStatus as WireRunStatus,
  TaskStatus as WireTaskStatus,
} from "../../src/proto/taktdash.ts";
import {
  restoreDashboardState,
  type DashboardGoal,
  type DashboardPendingDecision,
  type DashboardRun,
} from "../../src/state/restore-state.ts";

// 二つの型が互いに代入可能であること（同じ型であること）を検査する。
type Equal<Left, Right> =
  (<Value>() => Value extends Left ? 1 : 2) extends <Value>() => Value extends Right ? 1 : 2
    ? true
    : false;

// 条件が true でなければ型検査で失敗させる。
type Expect<Condition extends true> = Condition;

// 期待する brand。domain-primitives.ts の RunLimit と同じ形で定義する。
type GoalStatus = number & { readonly __brand: "GoalStatus" };
type ExecutionStatus = number & { readonly __brand: "ExecutionStatus" };
type TaskStatus = number & { readonly __brand: "TaskStatus" };
type RunStatus = number & { readonly __brand: "RunStatus" };
type PendingDecisionType = number & { readonly __brand: "PendingDecisionType" };

type GoalStatusIsBrand = Expect<Equal<DashboardGoal["status"], GoalStatus>>;
type GoalExecutionStatusIsBrand = Expect<Equal<DashboardGoal["executionStatus"], ExecutionStatus>>;
type GoalMergeTaskLastIsBrand = Expect<Equal<DashboardGoal["mergeTaskLast"], TaskStatus | undefined>>;
type RunStatusIsBrand = Expect<Equal<DashboardRun["status"], RunStatus>>;
type PendingDecisionTypeIsBrand = Expect<
  Equal<DashboardPendingDecision["type"], PendingDecisionType>
>;

test("UNSPECIFIED を含む状態の種別は、いまの復元でも通る", () => {
  const restored = restoreDashboardState({
    goals: [
      {
        goalId: "goal-1",
        serverId: "mini",
        project: "esa",
        status: WireGoalStatus.GOAL_STATUS_UNSPECIFIED,
        executionStatus: WireExecutionStatus.EXECUTION_STATUS_UNSPECIFIED,
        mergeTaskLast: WireTaskStatus.TASK_STATUS_UNSPECIFIED,
      },
      // wire は 0 を省略するため、欠落も UNSPECIFIED として復元する。
      { goalId: "goal-2", serverId: "mini", project: "esa" },
    ],
    runs: [
      {
        runSlug: "run-1",
        serverId: "mini",
        project: "esa",
        taskName: "task",
        status: WireRunStatus.RUN_STATUS_UNSPECIFIED,
      },
      { runSlug: "run-2", serverId: "mini", project: "esa", taskName: "task" },
    ],
    pendingDecisions: [
      {
        serverId: "mini",
        project: "esa",
        type: WirePendingDecisionType.PENDING_DECISION_TYPE_UNSPECIFIED,
      },
      { serverId: "mini", project: "esa" },
    ],
  });

  for (const goal of restored.goals) {
    assert.equal(goal.status, WireGoalStatus.GOAL_STATUS_UNSPECIFIED);
    assert.equal(goal.executionStatus, WireExecutionStatus.EXECUTION_STATUS_UNSPECIFIED);
  }
  assert.equal(restored.goals[0]?.mergeTaskLast, WireTaskStatus.TASK_STATUS_UNSPECIFIED);
  assert.equal(restored.goals[1]?.mergeTaskLast, undefined);

  for (const run of restored.runs) {
    assert.equal(run.status, WireRunStatus.RUN_STATUS_UNSPECIFIED);
  }

  for (const decision of restored.pendingDecisions) {
    assert.equal(decision.type, WirePendingDecisionType.PENDING_DECISION_TYPE_UNSPECIFIED);
  }
});

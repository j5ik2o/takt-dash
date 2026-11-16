// ゴールの状態バッジの対応表（issue #78）。
//
// ゴールの状態は Goal.status と execution_status の組み合わせで決まる。実行状態を
// 先に評価すると、完了していても実行状態が ACTIVE のゴールに「稼働中」と出る。
// 終端（中止・完了）を先に確定し、残りを一時停止・マージ待ち・進行中へ分ける。
// この対応表を1か所に閉じ、画面の各所で組み合わせを分岐させない。
import type { Goal } from "../proto/taktdash";
import { EXECUTION_STATUS, GOAL_STATUS } from "./proto-values";

export type GoalBadgeKind = "active" | "awaiting_merge" | "paused" | "completed" | "aborted";
export type GoalBadgeTone = "live" | "warn" | "idle";

export type GoalBadge = {
  kind: GoalBadgeKind;
  label: string;
  tone: GoalBadgeTone;
};

export function goalBadge(goal: Pick<Goal, "status" | "executionStatus">): GoalBadge {
  if (goal.executionStatus === EXECUTION_STATUS.ABORTED) {
    return { kind: "aborted", label: "中止", tone: "warn" };
  }
  if (goal.status === GOAL_STATUS.COMPLETED) {
    return { kind: "completed", label: "完了", tone: "idle" };
  }
  if (goal.executionStatus === EXECUTION_STATUS.PAUSED) {
    return { kind: "paused", label: "一時停止", tone: "idle" };
  }
  if (goal.status === GOAL_STATUS.AWAITING_MERGE) {
    return { kind: "awaiting_merge", label: "マージ待ち", tone: "warn" };
  }
  return { kind: "active", label: "進行中", tone: "live" };
}

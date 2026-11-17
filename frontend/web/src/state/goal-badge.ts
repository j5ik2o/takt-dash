// ゴールの状態バッジの対応表（issue #78、issue #189）。
//
// ゴールの状態は Goal.status と execution_status の組み合わせで決まる。実行状態を
// 先に評価すると、完了していても実行状態が ACTIVE のゴールに「稼働中」と出る。
// 終端（中止・完了）を先に確定し、残りを一時停止・マージ待ちへ分ける。最後に残る
// 作成済みのゴールは、タスクが登録されていなければ「未着手」、実行中のタスクが
// あれば「実行中」、どちらでもなければ「進行中」とし、タスクが未登録なのか、
// 実際に run が動いているのかを文言で区別する（issue #189）。
// この対応表を1か所に閉じ、画面の各所で組み合わせを分岐させない。
import type { Goal } from "../proto/taktdash";
import { goalTaskProgress } from "./goal-summary";
import { EXECUTION_STATUS, GOAL_STATUS } from "./proto-values";

export type GoalBadgeKind =
  | "active"
  | "running"
  | "untouched"
  | "awaiting_merge"
  | "paused"
  | "completed"
  | "aborted";
export type GoalBadgeTone = "live" | "warn" | "idle";

export type GoalBadge = {
  kind: GoalBadgeKind;
  label: string;
  tone: GoalBadgeTone;
};

export function goalBadge(
  goal: Pick<Goal, "status" | "executionStatus" | "taskCounts">,
): GoalBadge {
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
  const progress = goalTaskProgress(goal);
  if (progress.total === 0) {
    return { kind: "untouched", label: "未着手", tone: "idle" };
  }
  if (progress.running > 0) {
    return { kind: "running", label: "実行中", tone: "live" };
  }
  return { kind: "active", label: "進行中", tone: "live" };
}

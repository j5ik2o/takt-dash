// ゴールの状態バッジの対応表（issue #78、issue #189）。
//
// ゴールの状態は Goal.status と execution_status の組み合わせで決まる。実行状態を
// 先に評価すると、完了していても実行状態が ACTIVE のゴールに「稼働中」と出る。
// 終端（中止・完了）を先に確定し、残りを一時停止・マージ待ちへ分ける。最後に残る
// 作成済みのゴールは、タスクが登録されていなければ「未着手」、実行中のタスクが
// あれば「実行中」、どちらでもなければ「進行中」とし、タスクが未登録なのか、
// 実際に run が動いているのかを文言で区別する（issue #189）。
// この対応表を1か所に閉じ、画面の各所で組み合わせを分岐させない。
// カードと詳細の文面（goalStateSentence）も同じ対応表から作る。状態と実行状態の
// ラベルをそのまま並べると「ゴールは完了で、実行状態は稼働中です」のように矛盾
// して読めるため、文面でも組み合わせを先に確定する（issue #198）。
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

export type GoalSection = "inProgress" | "untouched" | "completed" | "aborted";

// ゴールの区画分け（issue #242、issue #243）。
//
// 画面の各所で使う「進行中・未着手・完了・中止」の区画は、バッジの対応表から
// 一意に決める。タスクが登録されていないゴールは、進行中の区画へ入れず未着手の
// 区画へ分ける（issue #242）。中止したゴールは完了の区画へ含めず、中止の区画で
// 別に数える（issue #243）。区画分けをこの対応表からだけ作り、画面ごとに
// status と execution_status の組み合わせを分岐させない。
export function goalSection(
  goal: Pick<Goal, "status" | "executionStatus" | "taskCounts">,
): GoalSection {
  switch (goalBadge(goal).kind) {
    case "completed":
      return "completed";
    case "aborted":
      return "aborted";
    case "untouched":
      return "untouched";
    default:
      return "inProgress";
  }
}

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

// ゴールの状態を1文で表す文面（issue #198）。
//
// バッジと同じ優先順位で組み合わせを確定する。中止はゴール自体の中止として述べ、
// 完了したゴールは完了としてだけ述べる。完了後に実行中のタスクが残っている場合
// だけ、その件数を補う。作成済みのゴールは、実行状態の生の値ではなくタスクの
// 実態（未登録・実行中・進行中）で述べ、稼働中とは出さない。
export function goalStateSentence(
  goal: Pick<Goal, "status" | "executionStatus" | "taskCounts">,
): string {
  if (goal.executionStatus === EXECUTION_STATUS.ABORTED) {
    return "ゴールは中止されました。";
  }
  const progress = goalTaskProgress(goal);
  if (goal.status === GOAL_STATUS.COMPLETED) {
    if (progress.running > 0) {
      return `ゴールは完了しています。実行中のタスクが ${progress.running} 件残っています。`;
    }
    return "ゴールは完了しています。";
  }
  if (goal.executionStatus === EXECUTION_STATUS.PAUSED) {
    if (goal.status === GOAL_STATUS.AWAITING_MERGE) {
      return "ゴールは人のマージ待ちで、実行は一時停止しています。";
    }
    return "ゴールは作成済みで、実行は一時停止しています。";
  }
  if (goal.status === GOAL_STATUS.AWAITING_MERGE) {
    return "ゴールは人のマージ待ちです。";
  }
  if (progress.total === 0) {
    return "ゴールは作成済みで、タスクはまだ登録されていません。";
  }
  if (progress.running > 0) {
    return `ゴールは作成済みで、実行中のタスクが ${progress.running} 件あります。`;
  }
  return "ゴールは作成済みで、タスクは進行中です。";
}

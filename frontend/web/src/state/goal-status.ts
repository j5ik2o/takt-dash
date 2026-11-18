// ゴールとタスクの状態を画面の文言へ翻訳し、人のマージ待ちかどうかを判定する。
//
// 状態の意味づけは proto の列挙値と目標のライフサイクルの組み合わせで決まり、
// 概要の集計や並び替えとは独立して同じ値をゴール一覧・ゴール詳細・タスク詳細・
// 判断待ちが使う。表示文言を変えるときの更新をここへ閉じ込める。
import type { ExecutionStatus, Goal, GoalStatus, PendingDecision, TaskStatus } from "../proto/taktdash";
import { EXECUTION_STATUS, GOAL_STATUS, PENDING_DECISION_TYPE, TASK_STATUS } from "./proto-values";

export const goalStatusLabel = (status: GoalStatus): string => {
  switch (status) {
    case GOAL_STATUS.CREATED:
      return "作成済み";
    case GOAL_STATUS.AWAITING_MERGE:
      return "人のマージ待ち";
    case GOAL_STATUS.COMPLETED:
      return "完了";
    default:
      return "不明";
  }
};

export const executionStatusLabel = (status: ExecutionStatus): string => {
  switch (status) {
    case EXECUTION_STATUS.ACTIVE:
      return "稼働中";
    case EXECUTION_STATUS.PAUSED:
      return "一時停止中";
    case EXECUTION_STATUS.ABORTED:
      return "中止";
    default:
      return "不明";
  }
};

export const taskStatusLabel = (status: TaskStatus): string => {
  switch (status) {
    case TASK_STATUS.PENDING:
      return "未着手";
    case TASK_STATUS.RUNNING:
      return "実行中";
    case TASK_STATUS.COMPLETED:
      return "完了";
    case TASK_STATUS.FAILED:
      return "失敗";
    case TASK_STATUS.EXCEEDED:
      return "上限超過";
    case TASK_STATUS.PR_FAILED:
      return "PR 失敗";
    default:
      return "不明";
  }
};

// 仕上げタスクの末尾状態だけで人のマージ待ちかどうかを判定する。通常タスクの pending/running は
// 判定に影響しない。awaiting_merge で、末尾が未着手・実行中でなければ人のマージ待ちになる。
// 失敗・上限超過・PR 作成失敗は、仕上げが止まって人の判断が要る状態として含める（issue #231）。
export function isHumanMergeWaiting(goal: Goal): boolean {
  if (goal.status !== GOAL_STATUS.AWAITING_MERGE) {
    return false;
  }
  return goal.mergeTaskLast !== TASK_STATUS.PENDING && goal.mergeTaskLast !== TASK_STATUS.RUNNING;
}

// 仕上げタスクの末尾状態が失敗系のときに、仕上げが止まった理由を示す文面を返す。
// ゴール一覧のカードと判断待ちのカードで同じ文面を使うため、末尾状態だけから決める。
const mergeTaskFailureNoteForLast = (mergeTaskLast: TaskStatus | undefined): string | undefined => {
  switch (mergeTaskLast) {
    case TASK_STATUS.FAILED:
      return "仕上げタスクが失敗しました。";
    case TASK_STATUS.EXCEEDED:
      return "仕上げタスクが上限超過で止まりました。";
    case TASK_STATUS.PR_FAILED:
      return "仕上げタスクのプルリクエスト作成が失敗しました。";
    default:
      return undefined;
  }
};

// 仕上げタスクが失敗系の状態で終わったことを示す文面（issue #231）。人のマージ待ちとして
// 出すときに、仕上げが止まった理由が分かるようにする。awaiting_merge でないゴールと、
// 失敗系でない末尾状態では undefined を返す。
export function mergeTaskFailureNote(goal: Goal): string | undefined {
  if (goal.status !== GOAL_STATUS.AWAITING_MERGE) {
    return undefined;
  }
  return mergeTaskFailureNoteForLast(goal.mergeTaskLast);
}

// 判断待ちのカードに出す、仕上げタスクの失敗を示す文面（issue #231）。マージ待ちの項目で、
// 仕上げタスクが失敗系の状態で終わったときだけ返す。それ以外の種別と末尾状態では
// undefined を返す。
export function pendingDecisionMergeFailureNote(decision: PendingDecision): string | undefined {
  if (decision.type !== PENDING_DECISION_TYPE.AWAITING_MERGE) {
    return undefined;
  }
  return mergeTaskFailureNoteForLast(decision.mergeTaskLast);
}

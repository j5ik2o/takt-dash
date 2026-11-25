// 判断待ち1件の操作の形と、送信するアクションの組み立て。
//
// 一覧の表示と送信はこの結果をそのまま使い、同じ可否判定を各所で作り直さない。
// 必要な値が欠けた項目は操作させず、理由を表示する。
import type { PendingDecision } from "../proto/taktdash";
import { INQUIRY_KIND, OPERATION_ANSWER_REPORT, PENDING_DECISION_TYPE } from "./proto-values";

export const isPositiveInteger = (value: unknown): value is number =>
  typeof value === "number" && Number.isInteger(value) && value > 0;

// プルリクエストの URL を、リンクとして開ける形（http・https の絶対 URL）に限って返す。
// 確認ダイアログと一覧は、この値があるときだけプルリクエストへのリンクを出す。
export function pullRequestLinkUrl(url: unknown): string | undefined {
  if (typeof url !== "string" || !URL.canParse(url)) {
    return undefined;
  }
  const protocol = new URL(url).protocol;
  return protocol === "https:" || protocol === "http:" ? url : undefined;
}

// 一覧に出す操作の形。必要な値が欠けた項目は操作させず、理由を表示する。
export type DecisionActionState =
  | { kind: "question-options"; options: string[] }
  | { kind: "question-text" }
  | { kind: "operation" }
  | { kind: "merge" }
  | { kind: "goal-registration" }
  | { kind: "unavailable"; reason: string };

const unavailable = (reason: string): DecisionActionState => ({ kind: "unavailable", reason });

const awaitingMergeActionState = (decision: PendingDecision): DecisionActionState => {
  if (!decision.goalId) {
    return unavailable("ゴール ID が無いため、マージを承認できません。");
  }
  if (!isPositiveInteger(decision.pullRequest?.number)) {
    return unavailable("プルリクエストの番号が無いため、マージを承認できません。");
  }
  // 確認ダイアログでプルリクエストの中身を開いて確かめられない項目は、取り違えを防ぐため
  // マージを承認させない（issue #304）。
  if (pullRequestLinkUrl(decision.pullRequest?.url) === undefined) {
    return unavailable("プルリクエストの URL が無いため、マージを承認できません。");
  }
  return { kind: "merge" };
};

const goalRegistrationActionState = (decision: PendingDecision): DecisionActionState => {
  if (!isPositiveInteger(decision.revision)) {
    return unavailable("revision が無いため、承認・却下できません。");
  }
  return { kind: "goal-registration" };
};

const inquiryActionState = (decision: PendingDecision): DecisionActionState => {
  if (!decision.inquiryId) {
    return unavailable("問い合わせの識別子が無いため、回答できません。");
  }
  if (!decision.goalId) {
    return unavailable("ゴールに属さない問い合わせには回答できません。");
  }
  if (decision.inquiryKind === INQUIRY_KIND.OPERATION_REQUEST) {
    return { kind: "operation" };
  }
  if (decision.options.length > 0) {
    return { kind: "question-options", options: decision.options };
  }
  return { kind: "question-text" };
};

const DECISION_ACTION_STATE_BY_TYPE: Partial<Record<number, (decision: PendingDecision) => DecisionActionState>> = {
  [PENDING_DECISION_TYPE.AWAITING_MERGE]: awaitingMergeActionState,
  [PENDING_DECISION_TYPE.GOAL_REGISTRATION]: goalRegistrationActionState,
  [PENDING_DECISION_TYPE.INQUIRY]: inquiryActionState,
};

export function decisionActionState(decision: PendingDecision): DecisionActionState {
  const resolve = DECISION_ACTION_STATE_BY_TYPE[decision.type];
  return resolve === undefined
    ? unavailable("この種別の判断待ちには対応していません。")
    : resolve(decision);
}

type AnswerPayload = {
  serverId: string;
  project: string;
  goalId?: string;
  inquiryId: string;
  text?: string;
  report?: number;
};

type MergePayload = {
  serverId: string;
  project: string;
  goalId: string;
  prNumber: number;
};

type GoalRegistrationPayload = {
  serverId: string;
  project: string;
  revision: number;
};

export type DecisionAction =
  | { kind: "answer"; payload: AnswerPayload }
  | { kind: "merge"; payload: MergePayload }
  | { kind: "approve-registration"; payload: GoalRegistrationPayload }
  | { kind: "reject-registration"; payload: GoalRegistrationPayload };

const isAnswerable = (state: DecisionActionState): boolean =>
  state.kind === "question-options" || state.kind === "question-text" || state.kind === "operation";

export function buildAnswerAction(
  decision: PendingDecision,
  input: { text?: string; report?: number },
): DecisionAction | undefined {
  const state = decisionActionState(decision);
  if (!isAnswerable(state)) {
    return undefined;
  }

  const { text, report } = input;
  if (text !== undefined && report !== undefined) {
    return undefined;
  }
  if (text !== undefined && state.kind === "operation") {
    return undefined;
  }

  if (report !== undefined) {
    if (state.kind !== "operation") {
      return undefined;
    }
    if (report !== OPERATION_ANSWER_REPORT.COMPLETED && report !== OPERATION_ANSWER_REPORT.UNABLE) {
      return undefined;
    }
    return {
      kind: "answer",
      payload: {
        serverId: decision.serverId,
        project: decision.project,
        goalId: decision.goalId,
        inquiryId: decision.inquiryId,
        report,
      },
    };
  }

  if (text === undefined || text.trim() === "") {
    return undefined;
  }

  return {
    kind: "answer",
    payload: {
      serverId: decision.serverId,
      project: decision.project,
      goalId: decision.goalId,
      inquiryId: decision.inquiryId,
      text,
    },
  };
}

export function buildMergeAction(decision: PendingDecision): DecisionAction | undefined {
  const state = decisionActionState(decision);
  const goalId = decision.goalId;
  const prNumber = decision.pullRequest?.number;
  if (state.kind !== "merge" || goalId === undefined || prNumber === undefined) {
    return undefined;
  }
  return {
    kind: "merge",
    payload: { serverId: decision.serverId, project: decision.project, goalId, prNumber },
  };
}

export function buildGoalRegistrationAction(
  decision: PendingDecision,
  operation: "approve" | "reject",
): DecisionAction | undefined {
  const state = decisionActionState(decision);
  const revision = decision.revision;
  if (state.kind !== "goal-registration" || revision === undefined) {
    return undefined;
  }
  return {
    kind: operation === "approve" ? "approve-registration" : "reject-registration",
    payload: { serverId: decision.serverId, project: decision.project, revision },
  };
}

// 判断待ちの操作を組み立てる層。
//
// 一覧の項目から、送信先の種類・対象の識別・回答の内容を決める。画面（View と Route
// Handler）はこの層が作った値をそのまま使い、同じ判定を各所で作り直さない。
import type { PendingDecision } from "../proto/taktdash";
import { INQUIRY_KIND, OPERATION_ANSWER_REPORT, PENDING_DECISION_TYPE } from "./proto-values";

const inquiryKindLabel = (kind: number): string =>
  kind === INQUIRY_KIND.OPERATION_REQUEST ? "作業の依頼" : "質問";

const isPositiveInteger = (value: unknown): value is number =>
  typeof value === "number" && Number.isInteger(value) && value > 0;

// 一覧の React key と操作結果の key に使う。SSE の再描画で並び順が変わっても、
// 同じ判断待ちに同じ結果を紐づけられるよう、種別と識別子だけから作る。
export function decisionKey(decision: PendingDecision): string {
  switch (decision.type) {
    case PENDING_DECISION_TYPE.AWAITING_MERGE:
      return `merge:${decision.serverId}:${decision.project}:${decision.goalId ?? ""}`;
    case PENDING_DECISION_TYPE.GOAL_REGISTRATION:
      return `goal-registration:${decision.serverId}:${decision.project}:${decision.revision ?? ""}`;
    case PENDING_DECISION_TYPE.INQUIRY:
      return `inquiry:${decision.serverId}:${decision.project}:${decision.inquiryId}`;
    default:
      return `pending:${decision.serverId}:${decision.project}:${decision.type}`;
  }
}

export function decisionTypeLabel(decision: PendingDecision): string {
  switch (decision.type) {
    case PENDING_DECISION_TYPE.AWAITING_MERGE:
      return "人のマージ待ち";
    case PENDING_DECISION_TYPE.GOAL_REGISTRATION:
      return "ゴール登録の承認待ち";
    case PENDING_DECISION_TYPE.INQUIRY:
      return `問い合わせ（${inquiryKindLabel(decision.inquiryKind)}）`;
    default:
      return "判断待ち";
  }
}

// 操作結果に出す項目名。どのプロジェクトの何に対する結果かが分かるようにする。
export function decisionLabel(decision: PendingDecision): string {
  return `${decision.project} の${decisionTypeLabel(decision)}`;
}

// 操作要素の accessible name と入力欄の label に使う。一覧は全サーバー・全プロジェクトの
// 判断待ちの和集合なので、同じ固定名の操作要素が複数並ぶ。種別と行固有の識別子を含めて
// 対象を一意に識別できるようにする。
export function decisionTargetLabel(decision: PendingDecision): string {
  const base = decisionLabel(decision);
  switch (decision.type) {
    case PENDING_DECISION_TYPE.AWAITING_MERGE:
      return `${base} PR #${decision.pullRequest?.number ?? "不明"}`;
    case PENDING_DECISION_TYPE.GOAL_REGISTRATION:
      return `${base} revision ${decision.revision ?? "不明"}`;
    case PENDING_DECISION_TYPE.INQUIRY:
      return `${base} ${decision.inquiryId}（${decision.body}）`;
    default:
      return base;
  }
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

export function decisionActionState(decision: PendingDecision): DecisionActionState {
  switch (decision.type) {
    case PENDING_DECISION_TYPE.AWAITING_MERGE:
      if (!decision.goalId) {
        return unavailable("ゴール ID が無いため、マージを承認できません。");
      }
      if (!isPositiveInteger(decision.pullRequest?.number)) {
        return unavailable("プルリクエストの番号が無いため、マージを承認できません。");
      }
      return { kind: "merge" };
    case PENDING_DECISION_TYPE.GOAL_REGISTRATION:
      if (!isPositiveInteger(decision.revision)) {
        return unavailable("revision が無いため、承認・却下できません。");
      }
      return { kind: "goal-registration" };
    case PENDING_DECISION_TYPE.INQUIRY:
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
    default:
      return unavailable("この種別の判断待ちには対応していません。");
  }
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

export type DecisionOutcome = {
  ok: boolean;
  message: string;
};

export type DecisionResult = DecisionOutcome & {
  key: string;
  label: string;
};

// 送信結果を、一覧の表示領域へ渡す結果値へ変換する。key は一覧の React key と同じ
// decisionKey を使い、SSE の再描画で並び替わっても同じ項目の結果として対応付ける。
export function toDecisionResult(decision: PendingDecision, outcome: DecisionOutcome): DecisionResult {
  return {
    key: decisionKey(decision),
    label: decisionLabel(decision),
    ok: outcome.ok,
    message: outcome.message,
  };
}

// 結果一覧へ1件を追加する。同じ key の結果は置換し、別の key の結果は保持する。
export function upsertDecisionResult(results: DecisionResult[], next: DecisionResult): DecisionResult[] {
  return [next, ...results.filter((result) => result.key !== next.key)];
}

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

// 画面から届く操作の意図。押下されたボタンと、入力中の本文だけを持ち、状態は持たない。
export type DecisionDialogKind = "merge" | "approve-registration" | "reject-registration";

export type DecisionIntent =
  | { type: "answer-text" }
  | { type: "answer-option"; option: string }
  | { type: "answer-report"; report: number }
  | { type: "open"; dialog: DecisionDialogKind }
  | { type: "cancel" }
  | { type: "confirm"; dialog: DecisionDialogKind };

// 意図を、ダイアログを開く・閉じる・送信するのいずれかへ解決する。DOM に依存しないため、
// どの意図がどのダイアログ・どの送信操作になるかを単体テストで確認できる。
export type DecisionIntentResolution =
  | { kind: "open"; dialog: DecisionDialogKind }
  | { kind: "close" }
  | { kind: "submit"; action: DecisionAction | undefined };

export function resolveDecisionIntent(
  decision: PendingDecision,
  intent: DecisionIntent,
  input: { text: string },
): DecisionIntentResolution {
  switch (intent.type) {
    case "answer-text":
      return { kind: "submit", action: buildAnswerAction(decision, { text: input.text }) };
    case "answer-option":
      return { kind: "submit", action: buildAnswerAction(decision, { text: intent.option }) };
    case "answer-report":
      return { kind: "submit", action: buildAnswerAction(decision, { report: intent.report }) };
    case "open":
      return { kind: "open", dialog: intent.dialog };
    case "cancel":
      return { kind: "close" };
    case "confirm":
      return {
        kind: "submit",
        action:
          intent.dialog === "merge"
            ? buildMergeAction(decision)
            : buildGoalRegistrationAction(decision, intent.dialog === "approve-registration" ? "approve" : "reject"),
      };
  }
}

const ENDPOINTS: Record<DecisionAction["kind"], string> = {
  answer: "/api/decisions/answer",
  merge: "/api/decisions/merge",
  "approve-registration": "/api/decisions/goal-registration/approve",
  "reject-registration": "/api/decisions/goal-registration/reject",
};

const SUCCESS_MESSAGES: Record<DecisionAction["kind"], string> = {
  answer: "回答を送信しました。",
  merge: "マージを承認しました。",
  "approve-registration": "ゴール登録を承認しました。",
  "reject-registration": "ゴール登録を却下しました。",
};

const DEFAULT_FAILURE_MESSAGE = "操作に失敗しました。";
const COMMUNICATION_FAILURE_MESSAGE = "操作を送信できませんでした。通信を確認してください。";

type WriteResponseBody = { ok?: boolean; error?: string };

export async function submitDecisionAction(
  action: DecisionAction,
  fetchImpl: typeof fetch = fetch,
): Promise<DecisionOutcome> {
  let response: Response;
  try {
    response = await fetchImpl(ENDPOINTS[action.kind], {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(action.payload),
    });
  } catch {
    return { ok: false, message: COMMUNICATION_FAILURE_MESSAGE };
  }

  const body: WriteResponseBody | undefined = await response.json().catch(() => undefined);
  if (response.ok && body?.ok === true) {
    return { ok: true, message: SUCCESS_MESSAGES[action.kind] };
  }
  if (typeof body?.error === "string" && body.error !== "") {
    return { ok: false, message: body.error };
  }
  return { ok: false, message: DEFAULT_FAILURE_MESSAGE };
}

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

// 種別ごとの識別子。SSE の再描画で並び順が変わっても、同じ判断待ちに同じ結果を
// 紐づけられるよう、種別と識別子だけから作る。
type DecisionKeyBuilder = (decision: PendingDecision) => string;

const DECISION_KEY_BY_TYPE: Partial<Record<number, DecisionKeyBuilder>> = {
  [PENDING_DECISION_TYPE.AWAITING_MERGE]: (decision) =>
    `merge:${decision.serverId}:${decision.project}:${decision.goalId ?? ""}`,
  [PENDING_DECISION_TYPE.GOAL_REGISTRATION]: (decision) =>
    `goal-registration:${decision.serverId}:${decision.project}:${decision.revision ?? ""}`,
  [PENDING_DECISION_TYPE.INQUIRY]: (decision) =>
    `inquiry:${decision.serverId}:${decision.project}:${decision.inquiryId}`,
};

export function decisionKey(decision: PendingDecision): string {
  const build = DECISION_KEY_BY_TYPE[decision.type];
  return build === undefined
    ? `pending:${decision.serverId}:${decision.project}:${decision.type}`
    : build(decision);
}

type DecisionTypeLabelResolver = (decision: PendingDecision) => string;

const DECISION_TYPE_LABEL_BY_TYPE: Partial<Record<number, DecisionTypeLabelResolver>> = {
  [PENDING_DECISION_TYPE.AWAITING_MERGE]: () => "人のマージ待ち",
  [PENDING_DECISION_TYPE.GOAL_REGISTRATION]: () => "ゴール登録の承認待ち",
  [PENDING_DECISION_TYPE.INQUIRY]: (decision) => `問い合わせ（${inquiryKindLabel(decision.inquiryKind)}）`,
};

export function decisionTypeLabel(decision: PendingDecision): string {
  const resolve = DECISION_TYPE_LABEL_BY_TYPE[decision.type];
  return resolve === undefined ? "判断待ち" : resolve(decision);
}

// 操作結果に出す項目名と、操作要素の accessible name・入力欄の label の共通部分。
// 基本ラベル（サーバー・プロジェクト・種別）と、種別ごとの識別子を一箇所で作る。
// 種別を増やすときの更新をここだけに閉じ込め、2つの表示がずれないようにする。
type DecisionLabelParts = {
  base: string;
  identifier: string;
};

const decisionLabelIdentifier = (decision: PendingDecision): string => {
  switch (decision.type) {
    case PENDING_DECISION_TYPE.AWAITING_MERGE:
      return `PR #${decision.pullRequest?.number ?? "不明"}`;
    case PENDING_DECISION_TYPE.GOAL_REGISTRATION:
      return `revision ${decision.revision ?? "不明"}`;
    case PENDING_DECISION_TYPE.INQUIRY:
      return decision.inquiryId;
    default:
      return "";
  }
};

const decisionLabelParts = (decision: PendingDecision): DecisionLabelParts => ({
  base: `${decision.serverId} / ${decision.project} の${decisionTypeLabel(decision)}`,
  identifier: decisionLabelIdentifier(decision),
});

const composeDecisionLabel = ({ base, identifier }: DecisionLabelParts): string =>
  identifier === "" ? base : `${base} ${identifier}`;

export function decisionLabel(decision: PendingDecision): string {
  return composeDecisionLabel(decisionLabelParts(decision));
}

// 操作要素の accessible name と入力欄の label に使う。一覧は全サーバー・全プロジェクトの
// 判断待ちの和集合なので、同じ固定名の操作要素が複数並ぶ。種別と行固有の識別子を含めて
// 対象を一意に識別できるようにする。問い合わせだけは本文も添えて区別する。
export function decisionTargetLabel(decision: PendingDecision): string {
  const parts = decisionLabelParts(decision);
  if (decision.type === PENDING_DECISION_TYPE.INQUIRY) {
    return `${parts.base} ${parts.identifier}（${decision.body}）`;
  }
  return composeDecisionLabel(parts);
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

// 確認ダイアログを開いた時点の対象。開いた後に一覧が SSE で更新されても、確認時は
// この値で送信し、表示もこの値を保つ。対象が変わっていれば送信しない。
export type DecisionDialogTarget =
  | { kind: "merge"; prNumber: number; url: string }
  | { kind: "approve-registration"; revision: number }
  | { kind: "reject-registration"; revision: number };

export function openDecisionDialog(
  decision: PendingDecision,
  dialog: DecisionDialogKind,
): DecisionDialogTarget | undefined {
  switch (dialog) {
    case "merge": {
      const prNumber = decision.pullRequest?.number;
      const url = decision.pullRequest?.url;
      if (!isPositiveInteger(prNumber) || typeof url !== "string" || url === "") {
        return undefined;
      }
      return { kind: "merge", prNumber, url };
    }
    case "approve-registration":
    case "reject-registration": {
      const revision = decision.revision;
      if (!isPositiveInteger(revision)) {
        return undefined;
      }
      return { kind: dialog, revision };
    }
  }
}

// 確認ダイアログの開閉と、開いた時点の対象をまとめて持つ状態。対象はダイアログを開く
// 操作の結果としてこの状態へ入り、確認の送信可否と表示はこの値を参照して決める。
export type DecisionDialogState =
  | { kind: "closed" }
  | { kind: "open"; dialog: DecisionDialogKind; target: DecisionDialogTarget | null };

export const INITIAL_DECISION_DIALOG_STATE: DecisionDialogState = { kind: "closed" };

// ダイアログを開き、開いた時点の対象を保持する。対象の値が欠けていてもダイアログは
// 開くが、target は null になり、確認時は送信せず stale として扱う。
export function openDecisionDialogState(
  decision: PendingDecision,
  dialog: DecisionDialogKind,
): DecisionDialogState {
  return { kind: "open", dialog, target: openDecisionDialog(decision, dialog) ?? null };
}

const staleTarget = (): DecisionIntentResolution => ({
  kind: "stale",
  message: "確認した時点の対象が変わったため送信しませんでした。もう一度操作してください。",
});

// 意図を、ダイアログを開く・閉じる・送信する・対象が変わったので送らない、のいずれかへ
// 解決する。DOM に依存しないため、どの意図がどのダイアログ・どの送信操作になるかを
// 単体テストで確認できる。
export type DecisionIntentResolution =
  | { kind: "open"; dialog: DecisionDialogKind }
  | { kind: "close" }
  | { kind: "stale"; message: string }
  | { kind: "submit"; action: DecisionAction | undefined };

export function resolveDecisionIntent(
  decision: PendingDecision,
  intent: DecisionIntent,
  input: { text: string },
  dialogTarget?: DecisionDialogTarget,
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
    case "confirm": {
      if (dialogTarget === undefined) {
        return staleTarget();
      }
      if (intent.dialog === "merge") {
        if (dialogTarget.kind !== "merge") {
          return staleTarget();
        }
        const current = decision.pullRequest;
        if (
          current === undefined ||
          current.number !== dialogTarget.prNumber ||
          current.url !== dialogTarget.url
        ) {
          return staleTarget();
        }
        return { kind: "submit", action: buildMergeAction(decision) };
      }
      const targetRevision = dialogTarget.kind === "merge" ? undefined : dialogTarget.revision;
      if (
        targetRevision === undefined ||
        dialogTarget.kind !== intent.dialog ||
        targetRevision !== decision.revision
      ) {
        return staleTarget();
      }
      return {
        kind: "submit",
        action: buildGoalRegistrationAction(
          decision,
          intent.dialog === "approve-registration" ? "approve" : "reject",
        ),
      };
    }
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

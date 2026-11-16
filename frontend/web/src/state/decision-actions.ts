// 判断待ちの操作結果の保持、確認ダイアログの意図解決、送信、および分割したモジュールの公開面。
//
// 識別子と表示ラベルは decision-labels、操作の形とアクション組み立ては
// decision-action-builders が持つ。既存の利用元が同じ import 元を使えるよう、
// ここから名前付きで re-export する。
import type { PendingDecision } from "../proto/taktdash";
import type { DecisionAction, DecisionActionState } from "./decision-action-builders";
import {
  buildAnswerAction,
  buildGoalRegistrationAction,
  buildMergeAction,
  decisionActionState,
  isPositiveInteger,
} from "./decision-action-builders";
import { decisionKey, decisionLabel } from "./decision-labels";

export type { DecisionAction, DecisionActionState };
export { decisionActionState, buildAnswerAction, buildGoalRegistrationAction, buildMergeAction };
export { decisionKey, decisionLabel };
export { decisionTargetLabel, decisionTypeLabel } from "./decision-labels";

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

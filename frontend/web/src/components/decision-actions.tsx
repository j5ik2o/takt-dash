"use client";

// 判断待ち1件分の操作の状態を持つ。確認ダイアログの開閉、入力中の本文、送信中を所有し、
// 送信結果は項目ごとに一意な key を添えて親へ渡す。
import { useRef, useState, type ReactElement } from "react";
import type { PendingDecision } from "../proto/taktdash";
import {
  INITIAL_DECISION_DIALOG_STATE,
  decisionAttempt,
  submitDecisionAction,
  toDecisionResult,
  type DecisionAction,
  type DecisionAttempt,
  type DecisionDialogState,
  type DecisionResult,
} from "../state/decision-actions";
import {
  DecisionActionsView,
  decideDecisionTransition,
  type DecisionActionsIntent,
} from "./decision-actions-view";

type DecisionActionsProps = {
  decision: PendingDecision;
  onResult: (result: DecisionResult) => void;
};

export function DecisionActions({ decision, onResult }: DecisionActionsProps): ReactElement {
  const [text, setText] = useState("");
  const [dialogState, setDialogState] = useState<DecisionDialogState>(INITIAL_DECISION_DIALOG_STATE);
  const [busy, setBusy] = useState(false);
  // 成功していない直前の送信。同じ操作のやり直しを、同じ鍵で backend へ送るために持つ。
  const unsettledAttempt = useRef<DecisionAttempt | undefined>(undefined);

  const run = async (action: DecisionAction | undefined): Promise<void> => {
    if (action === undefined) {
      return;
    }
    setBusy(true);
    const attempt = decisionAttempt(action, unsettledAttempt.current);
    unsettledAttempt.current = attempt;
    const outcome = await submitDecisionAction(action, fetch, attempt.idempotencyKey);
    if (outcome.ok) {
      unsettledAttempt.current = undefined;
    }
    setBusy(false);
    setDialogState(INITIAL_DECISION_DIALOG_STATE);
    onResult(toDecisionResult(decision, outcome));
    if (outcome.ok && action.kind === "answer") {
      setText("");
    }
  };

  const handleAction = (intent: DecisionActionsIntent): void => {
    const transition = decideDecisionTransition({ decision, intent, text, state: dialogState });
    switch (transition.kind) {
      case "state":
        setDialogState(transition.state);
        return;
      case "result":
        setDialogState(transition.state);
        onResult(transition.result);
        return;
      case "submit":
        void run(transition.action);
        return;
    }
  };

  return (
    <DecisionActionsView
      decision={decision}
      text={text}
      dialog={dialogState.kind === "open" ? dialogState.dialog : null}
      dialogTarget={dialogState.kind === "open" ? dialogState.target : null}
      busy={busy}
      onTextChange={setText}
      onAction={handleAction}
    />
  );
}

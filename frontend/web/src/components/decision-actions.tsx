"use client";

// 判断待ち1件分の操作の状態を持つ。確認ダイアログの開閉、入力中の本文、送信中を所有し、
// 送信結果は項目ごとに一意な key を添えて親へ渡す。
import { useLayoutEffect, useRef, useState, type ReactElement } from "react";
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
  acceptsDecisionIntent,
  decideDecisionTransition,
  type DecisionActionsIntent,
} from "./decision-actions-view";

type DecisionActionsProps = {
  decision: PendingDecision;
  disconnected: boolean;
  onResult: (result: DecisionResult) => void;
};

export function DecisionActions({ decision, disconnected, onResult }: DecisionActionsProps): ReactElement {
  const [text, setText] = useState("");
  const [dialogState, setDialogState] = useState<DecisionDialogState>(INITIAL_DECISION_DIALOG_STATE);
  const [busy, setBusy] = useState(false);
  // 送信中かどうかを、描き直しを待たずに見るための守り。busy の state は描画に使い、
  // この ref は同じ同期の区間での二重の送信と、送信中の意図の受け付けを止める。
  const submitting = useRef(false);
  // 成功していない直前の送信。同じ操作のやり直しを、同じ鍵で backend へ送るために持つ。
  const unsettledAttempt = useRef<DecisionAttempt | undefined>(undefined);
  // 切断中は送らない。ボタンの disabled だけに頼らず、操作の入口でも最新の接続状態を見る。
  const disconnectedRef = useRef(disconnected);
  useLayoutEffect(() => {
    disconnectedRef.current = disconnected;
  }, [disconnected]);

  const run = async (action: DecisionAction | undefined): Promise<void> => {
    if (action === undefined || disconnectedRef.current) {
      return;
    }
    if (submitting.current) {
      return;
    }
    submitting.current = true;
    setBusy(true);
    try {
      const attempt = decisionAttempt(action, unsettledAttempt.current);
      unsettledAttempt.current = attempt;
      const outcome = await submitDecisionAction(action, fetch, attempt.idempotencyKey);
      if (outcome.ok) {
        unsettledAttempt.current = undefined;
      }
      setDialogState(INITIAL_DECISION_DIALOG_STATE);
      onResult(toDecisionResult(decision, outcome));
      if (outcome.ok && action.kind === "answer") {
        setText("");
      }
    } finally {
      submitting.current = false;
      setBusy(false);
    }
  };

  const handleAction = (intent: DecisionActionsIntent): boolean => {
    if (!acceptsDecisionIntent(intent, disconnectedRef.current)) {
      return false;
    }
    if (submitting.current) {
      return false;
    }
    const transition = decideDecisionTransition({ decision, intent, text, state: dialogState });
    switch (transition.kind) {
      case "state":
        setDialogState(transition.state);
        return true;
      case "result":
        setDialogState(transition.state);
        onResult(transition.result);
        return true;
      case "submit":
        void run(transition.action);
        return true;
    }
  };

  return (
    <DecisionActionsView
      decision={decision}
      text={text}
      dialog={dialogState.kind === "open" ? dialogState.dialog : null}
      dialogTarget={dialogState.kind === "open" ? dialogState.target : null}
      busy={busy}
      disconnected={disconnected}
      onTextChange={setText}
      onAction={handleAction}
    />
  );
}

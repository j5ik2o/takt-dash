"use client";

// 判断待ち1件分の操作の状態を持つ。確認ダイアログの開閉、入力中の本文、送信中を所有し、
// 送信結果は項目ごとに一意な key を添えて親へ渡す。
import { useState, type ReactElement } from "react";
import type { PendingDecision } from "../proto/taktdash";
import {
  resolveDecisionIntent,
  submitDecisionAction,
  toDecisionResult,
  type DecisionAction,
  type DecisionResult,
} from "../state/decision-actions";
import { DecisionActionsView, type DecisionActionsIntent, type DialogKind } from "./decision-actions-view";

type DecisionActionsProps = {
  decision: PendingDecision;
  onResult: (result: DecisionResult) => void;
};

export function DecisionActions({ decision, onResult }: DecisionActionsProps): ReactElement {
  const [text, setText] = useState("");
  const [dialog, setDialog] = useState<DialogKind | null>(null);
  const [busy, setBusy] = useState(false);

  const run = async (action: DecisionAction | undefined): Promise<void> => {
    if (action === undefined) {
      return;
    }
    setBusy(true);
    const outcome = await submitDecisionAction(action);
    setBusy(false);
    setDialog(null);
    onResult(toDecisionResult(decision, outcome));
    if (outcome.ok && action.kind === "answer") {
      setText("");
    }
  };

  const handleAction = (intent: DecisionActionsIntent): void => {
    const resolution = resolveDecisionIntent(decision, intent, { text });
    switch (resolution.kind) {
      case "open":
        setDialog(resolution.dialog);
        return;
      case "close":
        setDialog(null);
        return;
      case "submit":
        void run(resolution.action);
        return;
    }
  };

  return (
    <DecisionActionsView
      decision={decision}
      text={text}
      dialog={dialog}
      busy={busy}
      onTextChange={setText}
      onAction={handleAction}
    />
  );
}

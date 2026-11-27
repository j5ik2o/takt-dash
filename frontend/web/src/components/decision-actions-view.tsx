"use client";

// 判断待ち1件分の操作 UI。送信内容の組み立てと送信そのものは DecisionActions が担う。
// この分離により、DOM を持たないテスト環境でもサーバー描画した HTML として操作要素を
// 検証でき、フォーカス管理はブラウザでのマウント後にだけ働く。
// 確認ダイアログ本体は confirm-dialog.tsx に分け、このファイルはテストと他の部品が
// これまでどおり import できるよう、名前をそのまま再公開する。
import { useEffect, useId, useRef, type ReactElement } from "react";
import type { PendingDecision } from "../proto/taktdash";
import {
  INITIAL_DECISION_DIALOG_STATE,
  decisionActionState,
  decisionTargetDescription,
  decisionTargetLabel,
  openDecisionDialogState,
  resolveDecisionIntent,
  toDecisionResult,
  type DecisionAction,
  type DecisionDialogState,
  type DecisionDialogTarget,
  type DecisionIntent,
  type DecisionResult,
} from "../state/decision-actions";
import {
  DIALOG_TRIGGERS,
  DecisionTargetDescription,
  DialogActionSection,
  OfflineNote,
  buildConfirmDialogs,
  type DecisionActionsIntent,
  type DialogKind,
} from "./confirm-dialog";

export { decideDialogKey } from "./confirm-dialog";
export type { DecisionActionsIntent, DialogKeyDecision, DialogKind } from "./confirm-dialog";

type DecisionActionsViewProps = {
  decision: PendingDecision;
  text: string;
  dialog: DialogKind | null;
  dialogTarget: DecisionDialogTarget | null;
  busy: boolean;
  /** backend と切れている間は、回答を送れないので操作を止める（issue #307）。 */
  disconnected: boolean;
  onTextChange: (text: string) => void;
  onAction: (intent: DecisionActionsIntent) => boolean;
};

export type DialogRestoreDecision =
  | { kind: "clear" }
  | { kind: "none" }
  | { kind: "wait"; target: DialogKind }
  | { kind: "restore"; target: DialogKind }
  | { kind: "drop" };

// ダイアログの開閉とトリガーの有効性から、閉じた後のフォーカス復帰を決める。
// DOM に依存しないため、非 busy の即時復帰・busy 中の待機・busy 解除後の復帰を
// 単体テストで確認できる。
export function decideDialogRestore(input: {
  dialog: DialogKind | null;
  previousDialog: DialogKind | null;
  pendingRestore: DialogKind | null;
  triggers: Partial<Record<DialogKind, { disabled: boolean } | null>>;
  /** 復帰を待つ間に、利用者が別の要素へフォーカスを移していたら true。上書きせず、待ちをやめる。 */
  focusElsewhere: boolean;
}): DialogRestoreDecision {
  if (input.dialog !== null) {
    return { kind: "clear" };
  }
  const target = input.previousDialog ?? input.pendingRestore;
  if (target === null) {
    return { kind: "none" };
  }
  const trigger = input.triggers[target];
  if (trigger === null || trigger === undefined || trigger.disabled) {
    return { kind: "wait", target };
  }
  if (input.focusElsewhere) {
    return { kind: "drop" };
  }
  return { kind: "restore", target };
}

// 切断中に受け付ける操作。閉じる操作だけは通し、開く・確定・回答は送信につながるので止める。
// ボタンの disabled だけに頼らず、操作の入口でもこの判定を通す（issue #307）。
export function acceptsDecisionIntent(intent: DecisionIntent, disconnected: boolean): boolean {
  return !disconnected || intent.type === "cancel";
}

export type DecisionTransition =
  | { kind: "state"; state: DecisionDialogState }
  | { kind: "result"; state: DecisionDialogState; result: DecisionResult }
  | { kind: "submit"; action: DecisionAction | undefined };

// 操作の意図を、次のダイアログ状態・結果通知・送信のいずれかへ写す。開いた時点の対象は
// ダイアログ状態が保持し、確認はその値で解決する。DOM に依存しないため、開いた後に一覧が
// 変わった確認が送信されず stale の結果になることを単体テストで確認できる。
export function decideDecisionTransition(input: {
  decision: PendingDecision;
  intent: DecisionIntent;
  text: string;
  state: DecisionDialogState;
}): DecisionTransition {
  const target = input.state.kind === "open" ? input.state.target : null;
  const resolution = resolveDecisionIntent(
    input.decision,
    input.intent,
    { text: input.text },
    target === null ? undefined : target,
  );
  switch (resolution.kind) {
    case "open":
      return { kind: "state", state: openDecisionDialogState(input.decision, resolution.dialog) };
    case "close":
      return { kind: "state", state: INITIAL_DECISION_DIALOG_STATE };
    case "stale":
      return {
        kind: "result",
        state: INITIAL_DECISION_DIALOG_STATE,
        result: toDecisionResult(input.decision, { ok: false, message: resolution.message }),
      };
    case "submit":
      return { kind: "submit", action: resolution.action };
  }
}

export function DecisionActionsView({
  decision,
  text,
  dialog,
  dialogTarget,
  busy,
  disconnected,
  onTextChange,
  onAction,
}: DecisionActionsViewProps): ReactElement {
  const state = decisionActionState(decision);
  const targetLabel = decisionTargetLabel(decision);
  // 操作ボタンの説明要素に出す文面。項目内にサーバーの ID も表示名も出さない既存契約に合わせ、
  // 対象の識別に必要な情報だけを含める。
  const descriptionLabel = decisionTargetDescription(decision);
  // この項目の操作ボタンが参照する宛先の識別子。項目ごとに一意になり、同名ボタンが
  // 複数並んでも各ボタンが自分の項目の対象説明だけを参照する。
  const descriptionId = useId();

  // ダイアログを開いたトリガーを覚えておき、閉じたときにフォーカスを戻す。
  // 送信中・切断中はトリガーが無効なので、復帰はその解除まで待つ。
  const triggerRefs = useRef<Record<DialogKind, HTMLButtonElement | null>>({
    merge: null,
    "approve-registration": null,
    "reject-registration": null,
    "answer-report-completed": null,
    "answer-report-unable": null,
  });
  const previousDialog = useRef<DialogKind | null>(null);
  const pendingRestore = useRef<DialogKind | null>(null);
  // ダイアログの外で、フォーカスのあるボタンや入力欄が切断で無効になったら、フォーカスを外す。
  // フォーカスを残すブラウザでも、落とすブラウザと同じく body にそろえる（仕様の disconnect）。
  useEffect(() => {
    const active = document.activeElement;
    if (dialog === null && (active instanceof HTMLButtonElement || active instanceof HTMLInputElement) && active.disabled) {
      active.blur();
    }
  }, [dialog, busy, disconnected]);
  useEffect(() => {
    const previous = previousDialog.current;
    previousDialog.current = dialog;
    const decision = decideDialogRestore({
      dialog,
      previousDialog: previous,
      pendingRestore: pendingRestore.current,
      triggers: triggerRefs.current,
      focusElsewhere: document.activeElement !== null && document.activeElement !== document.body,
    });
    switch (decision.kind) {
      case "clear":
        pendingRestore.current = null;
        return;
      case "none":
        return;
      case "drop":
        pendingRestore.current = null;
        return;
      case "wait":
        pendingRestore.current = decision.target;
        return;
      case "restore":
        triggerRefs.current[decision.target]?.focus();
        pendingRestore.current = null;
        return;
    }
  }, [dialog, busy, disconnected]);

  switch (state.kind) {
    case "unavailable":
      return <p className="note">{state.reason}</p>;

    case "question-options":
      return (
        <div className="actions">
          <DecisionTargetDescription id={descriptionId} targetLabel={descriptionLabel} />
          {state.options.map((option) => (
            <button
              key={option}
              type="button"
              disabled={busy || disconnected}
              aria-label={option}
              aria-describedby={descriptionId}
              onClick={() => onAction({ type: "answer-option", option })}
            >
              {option}
            </button>
          ))}
          <OfflineNote disconnected={disconnected} />
        </div>
      );

    case "question-text":
      return (
        <form
          className="actions"
          onSubmit={(event) => {
            event.preventDefault();
            onAction({ type: "answer-text" });
          }}
        >
          <DecisionTargetDescription id={descriptionId} targetLabel={descriptionLabel} />
          <input
            type="text"
            value={text}
            disabled={busy || disconnected}
            aria-label={`回答（${targetLabel}）`}
            onChange={(event) => onTextChange(event.target.value)}
          />
          <button
            type="submit"
            disabled={busy || disconnected || text.trim() === ""}
            aria-label="回答する"
            aria-describedby={descriptionId}
          >
            回答する
          </button>
          <OfflineNote disconnected={disconnected} />
        </form>
      );

    case "operation":
    case "merge":
    case "goal-registration": {
      const group = state.kind === "operation" ? "operation" : state.kind;
      return (
        <DialogActionSection
          triggers={
            group === "merge"
              ? DIALOG_TRIGGERS.merge
              : group === "operation"
                ? DIALOG_TRIGGERS.operation
                : DIALOG_TRIGGERS.goalRegistration
          }
          configs={buildConfirmDialogs(group, decision, dialogTarget)}
          active={dialog}
          descriptionId={descriptionId}
          targetLabel={descriptionLabel}
          busy={busy}
          disconnected={disconnected}
          triggerRefs={triggerRefs}
          onAction={onAction}
        />
      );
    }
  }
}

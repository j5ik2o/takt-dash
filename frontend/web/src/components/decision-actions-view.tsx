"use client";

// 判断待ち1件分の操作 UI。送信内容の組み立てと送信そのものは DecisionActions が担う。
// この分離により、DOM を持たないテスト環境でもサーバー描画した HTML として操作要素を
// 検証でき、フォーカス管理はブラウザでのマウント後にだけ働く。
import { useCallback, useEffect, useId, useRef, type ReactElement, type RefObject } from "react";
import type { PendingDecision } from "../proto/taktdash";
import { OPERATION_ANSWER_REPORT } from "../state/proto-values";
import {
  INITIAL_DECISION_DIALOG_STATE,
  decisionActionState,
  decisionTargetLabel,
  openDecisionDialogState,
  resolveDecisionIntent,
  toDecisionResult,
  type DecisionAction,
  type DecisionDialogKind,
  type DecisionDialogState,
  type DecisionDialogTarget,
  type DecisionIntent,
  type DecisionResult,
} from "../state/decision-actions";

export type DialogKind = DecisionDialogKind;

export type DecisionActionsIntent = DecisionIntent;

type DecisionActionsViewProps = {
  decision: PendingDecision;
  text: string;
  dialog: DialogKind | null;
  dialogTarget: DecisionDialogTarget | null;
  busy: boolean;
  onTextChange: (text: string) => void;
  onAction: (intent: DecisionActionsIntent) => void;
};

type ConfirmDialogProps = {
  dialog: DialogKind;
  title: string;
  message: string;
  confirmLabel: string;
  busy: boolean;
  onAction: (intent: DecisionActionsIntent) => void;
};

export type DialogKeyDecision =
  | { kind: "cancel" }
  | { kind: "none" }
  | { kind: "focus"; target: "first" | "last" | "heading" };

// ダイアログ表示中の Tab / Shift+Tab / Escape を、キーとフォーカスの現在位置だけから決める。
// DOM に依存しないため、ダイアログ外フォーカス時の Tab 先を単体テストで確認できる。
export function decideDialogKey(input: {
  key: string;
  shiftKey: boolean;
  tabbableCount: number;
  activeIndex: number;
}): DialogKeyDecision {
  if (input.key === "Escape") {
    return { kind: "cancel" };
  }
  if (input.key !== "Tab") {
    return { kind: "none" };
  }
  if (input.tabbableCount === 0) {
    return { kind: "focus", target: "heading" };
  }
  if (input.activeIndex < 0) {
    return { kind: "focus", target: input.shiftKey ? "last" : "first" };
  }
  if (input.shiftKey && input.activeIndex === 0) {
    return { kind: "focus", target: "last" };
  }
  if (!input.shiftKey && input.activeIndex === input.tabbableCount - 1) {
    return { kind: "focus", target: "first" };
  }
  return { kind: "none" };
}

export type DialogRestoreDecision =
  | { kind: "clear" }
  | { kind: "none" }
  | { kind: "wait"; target: DialogKind }
  | { kind: "restore"; target: DialogKind };

// ダイアログの開閉とトリガーの有効性から、閉じた後のフォーカス復帰を決める。
// DOM に依存しないため、非 busy の即時復帰・busy 中の待機・busy 解除後の復帰を
// 単体テストで確認できる。
export function decideDialogRestore(input: {
  dialog: DialogKind | null;
  previousDialog: DialogKind | null;
  pendingRestore: DialogKind | null;
  triggers: Record<DialogKind, { disabled: boolean } | null>;
}): DialogRestoreDecision {
  if (input.dialog !== null) {
    return { kind: "clear" };
  }
  const target = input.previousDialog ?? input.pendingRestore;
  if (target === null) {
    return { kind: "none" };
  }
  const trigger = input.triggers[target];
  if (trigger === null || trigger.disabled) {
    return { kind: "wait", target };
  }
  return { kind: "restore", target };
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

function ConfirmDialog({ dialog, title, message, confirmLabel, busy, onAction }: ConfirmDialogProps): ReactElement {
  const headingId = useId();
  const headingRef = useRef<HTMLHeadingElement>(null);
  const dialogRef = useRef<HTMLDivElement>(null);
  // close を送出したら封じ込めを止める。閉じる途中の focusout で消えゆくダイアログへ
  // フォーカスを戻し、トリガーへの復帰と競合させない。
  const released = useRef(false);
  const cancel = useCallback((): void => {
    released.current = true;
    onAction({ type: "cancel" });
  }, [onAction]);

  // 開いた直後にダイアログの見出しへフォーカスを移し、読み上げとキーボード操作の起点にする。
  useEffect(() => {
    headingRef.current?.focus();
  }, []);

  // 送信中はボタンが無効になり、押したボタンからフォーカスが外れる。見出しへ戻して
  // ダイアログ内にフォーカスを保つ。
  useEffect(() => {
    if (busy) {
      headingRef.current?.focus();
    }
  }, [busy]);

  // キー処理と逸脱検知はダイアログの子孫ではなく document に置く。本文や背景のクリックで
  // フォーカスが body へ移っても Tab/Escape を捕まえ、背景操作へ抜けさせない。
  useEffect(() => {
    const dialogElement = dialogRef.current;
    if (dialogElement === null) {
      return;
    }
    const tabbableElements = (): HTMLElement[] =>
      Array.from(dialogElement.querySelectorAll<HTMLElement>("button:not([disabled])"));
    const inside = (target: EventTarget | null): boolean =>
      target instanceof Node && dialogElement.contains(target);

    const focusInside = (target: "first" | "last" | "heading"): void => {
      if (target !== "heading") {
        const elements = tabbableElements();
        const element = target === "first" ? elements[0] : elements[elements.length - 1];
        if (element !== undefined) {
          element.focus();
          return;
        }
      }
      headingRef.current?.focus();
    };

    const handleKeyDown = (event: globalThis.KeyboardEvent): void => {
      if (released.current) {
        return;
      }
      const active = document.activeElement as HTMLElement | null;
      const elements = tabbableElements();
      const decision = decideDialogKey({
        key: event.key,
        shiftKey: event.shiftKey,
        tabbableCount: elements.length,
        activeIndex: active === null ? -1 : elements.indexOf(active),
      });
      switch (decision.kind) {
        case "cancel":
          event.preventDefault();
          cancel();
          return;
        case "focus":
          event.preventDefault();
          focusInside(decision.target);
          return;
        case "none":
          return;
      }
    };

    const handleFocusOut = (event: FocusEvent): void => {
      if (released.current || inside(event.relatedTarget)) {
        return;
      }
      focusInside("heading");
    };

    const handleFocusIn = (event: FocusEvent): void => {
      if (released.current || inside(event.target)) {
        return;
      }
      focusInside("heading");
    };

    document.addEventListener("keydown", handleKeyDown);
    document.addEventListener("focusout", handleFocusOut);
    document.addEventListener("focusin", handleFocusIn);
    return () => {
      document.removeEventListener("keydown", handleKeyDown);
      document.removeEventListener("focusout", handleFocusOut);
      document.removeEventListener("focusin", handleFocusIn);
    };
  }, [cancel]);

  return (
    <div className="dialog-backdrop">
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={headingId}
        className="dialog"
      >
        <h2 id={headingId} ref={headingRef} tabIndex={-1}>
          {title}
        </h2>
        <p className="dialog-message">{message}</p>
        <div className="actions">
          <button type="button" disabled={busy} onClick={cancel}>
            やめる
          </button>
          <button type="button" disabled={busy} onClick={() => onAction({ type: "confirm", dialog })}>
            {confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}

type ConfirmDialogConfig = {
  dialog: DialogKind;
  title: string;
  message: string;
  confirmLabel: string;
};

type DialogTrigger = {
  dialog: DialogKind;
  label: string;
};

const DIALOG_TRIGGERS = {
  merge: [{ dialog: "merge", label: "マージを承認" }],
  goalRegistration: [
    { dialog: "approve-registration", label: "承認" },
    { dialog: "reject-registration", label: "却下" },
  ],
} as const satisfies Record<string, DialogTrigger[]>;

type DialogGroup = "merge" | "goal-registration";

// 種別ごとの見出し・本文・確定ボタン名。本文は対象の値（PR 番号・revision）を差し込む。
// 確認ダイアログを開いた時点の値を dialogTarget に保っている場合、開いた後に SSE で
// 一覧が更新されても、確認する対象は開いた時点の値を表示する。
function buildConfirmDialogs(
  group: DialogGroup,
  decision: PendingDecision,
  dialogTarget: DecisionDialogTarget | null,
): ConfirmDialogConfig[] {
  if (group === "merge") {
    const prNumber =
      dialogTarget?.kind === "merge" ? dialogTarget.prNumber : decision.pullRequest?.number;
    return [
      {
        dialog: "merge",
        title: "マージの承認",
        message: `プルリクエスト #${prNumber ?? ""} のマージを承認します。よろしいですか。`,
        confirmLabel: "マージする",
      },
    ];
  }
  const revision =
    dialogTarget?.kind === "approve-registration" || dialogTarget?.kind === "reject-registration"
      ? dialogTarget.revision
      : decision.revision;
  return [
    {
      dialog: "approve-registration",
      title: "ゴール登録の承認",
      message: `revision ${revision ?? ""} のゴール登録を承認します。よろしいですか。`,
      confirmLabel: "承認",
    },
    {
      dialog: "reject-registration",
      title: "ゴール登録の却下",
      message: `revision ${revision ?? ""} のゴール登録を却下します。よろしいですか。`,
      confirmLabel: "却下",
    },
  ];
}

type DialogActionSectionProps = {
  triggers: DialogTrigger[];
  configs: ConfirmDialogConfig[];
  active: DialogKind | null;
  targetLabel: string;
  busy: boolean;
  triggerRefs: RefObject<Record<DialogKind, HTMLButtonElement | null>>;
  onAction: (intent: DecisionActionsIntent) => void;
};

// 確認ダイアログを開くトリガー群と、いま開いている種別のダイアログをまとめて描画する。
// 対象の識別子を含む aria-label と、種別ごとの見出し・本文・確定ボタン名が同じ形で並ぶ。
function DialogActionSection({
  triggers,
  configs,
  active,
  targetLabel,
  busy,
  triggerRefs,
  onAction,
}: DialogActionSectionProps): ReactElement {
  const current = configs.find((config) => config.dialog === active);
  return (
    <div className="actions">
      {triggers.map(({ dialog, label }) => (
        <button
          key={dialog}
          type="button"
          disabled={busy}
          ref={(element) => {
            triggerRefs.current[dialog] = element;
          }}
          aria-label={`${label}（${targetLabel}）`}
          onClick={() => onAction({ type: "open", dialog })}
        >
          {label}
        </button>
      ))}
      {current === undefined ? null : (
        <ConfirmDialog
          dialog={current.dialog}
          title={current.title}
          message={current.message}
          confirmLabel={current.confirmLabel}
          busy={busy}
          onAction={onAction}
        />
      )}
    </div>
  );
}

export function DecisionActionsView({
  decision,
  text,
  dialog,
  dialogTarget,
  busy,
  onTextChange,
  onAction,
}: DecisionActionsViewProps): ReactElement {
  const state = decisionActionState(decision);
  const targetLabel = decisionTargetLabel(decision);

  // ダイアログを開いたトリガーを覚えておき、閉じたときにフォーカスを戻す。
  // 送信中はトリガーが無効なので、復帰は busy の解除まで待つ。
  const triggerRefs = useRef<Record<DialogKind, HTMLButtonElement | null>>({
    merge: null,
    "approve-registration": null,
    "reject-registration": null,
  });
  const previousDialog = useRef<DialogKind | null>(null);
  const pendingRestore = useRef<DialogKind | null>(null);
  useEffect(() => {
    const previous = previousDialog.current;
    previousDialog.current = dialog;
    const decision = decideDialogRestore({
      dialog,
      previousDialog: previous,
      pendingRestore: pendingRestore.current,
      triggers: triggerRefs.current,
    });
    switch (decision.kind) {
      case "clear":
        pendingRestore.current = null;
        return;
      case "none":
        return;
      case "wait":
        pendingRestore.current = decision.target;
        return;
      case "restore":
        triggerRefs.current[decision.target]?.focus();
        pendingRestore.current = null;
        return;
    }
  }, [dialog, busy]);

  switch (state.kind) {
    case "unavailable":
      return <p className="note">{state.reason}</p>;

    case "question-options":
      return (
        <div className="actions">
          {state.options.map((option) => (
            <button
              key={option}
              type="button"
              disabled={busy}
              aria-label={`${option}（${targetLabel}）`}
              onClick={() => onAction({ type: "answer-option", option })}
            >
              {option}
            </button>
          ))}
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
          <input
            type="text"
            value={text}
            disabled={busy}
            aria-label={`回答（${targetLabel}）`}
            onChange={(event) => onTextChange(event.target.value)}
          />
          <button
            type="submit"
            disabled={busy || text.trim() === ""}
            aria-label={`回答する（${targetLabel}）`}
          >
            回答する
          </button>
        </form>
      );

    case "operation":
      return (
        <div className="actions">
          <button
            type="button"
            disabled={busy}
            aria-label={`実施した（${targetLabel}）`}
            onClick={() => onAction({ type: "answer-report", report: OPERATION_ANSWER_REPORT.COMPLETED })}
          >
            実施した
          </button>
          <button
            type="button"
            disabled={busy}
            aria-label={`対応できない（${targetLabel}）`}
            onClick={() => onAction({ type: "answer-report", report: OPERATION_ANSWER_REPORT.UNABLE })}
          >
            対応できない
          </button>
        </div>
      );

    case "merge":
      return (
        <DialogActionSection
          triggers={DIALOG_TRIGGERS.merge}
          configs={buildConfirmDialogs("merge", decision, dialogTarget)}
          active={dialog}
          targetLabel={targetLabel}
          busy={busy}
          triggerRefs={triggerRefs}
          onAction={onAction}
        />
      );

    case "goal-registration":
      return (
        <DialogActionSection
          triggers={DIALOG_TRIGGERS.goalRegistration}
          configs={buildConfirmDialogs("goal-registration", decision, dialogTarget)}
          active={dialog}
          targetLabel={targetLabel}
          busy={busy}
          triggerRefs={triggerRefs}
          onAction={onAction}
        />
      );
  }
}

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
  decisionTargetDescription,
  decisionTargetLabel,
  openDecisionDialogState,
  pullRequestLinkUrl,
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
  /** backend と切れている間は、回答を送れないので操作を止める（issue #307）。 */
  disconnected: boolean;
  onTextChange: (text: string) => void;
  onAction: (intent: DecisionActionsIntent) => void;
};

// 切断中に操作要素へ添える理由。再接続すると disconnected が戻り、操作も戻る。
function OfflineNote({ disconnected }: { disconnected: boolean }): ReactElement | null {
  return disconnected ? <p className="note">再接続するまで操作できません。</p> : null;
}

// 確認ダイアログに出す対象の1行。href があれば、値を新しいタブで開くリンクにする。
type ConfirmDialogDetail = {
  label: string;
  value: string;
  href?: string;
};

type ConfirmDialogProps = {
  dialog: DialogKind;
  title: string;
  message: string;
  details: ConfirmDialogDetail[];
  confirmLabel: string;
  busy: boolean;
  disconnected: boolean;
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

function ConfirmDialog({ dialog, title, message, details, confirmLabel, busy, disconnected, onAction }: ConfirmDialogProps): ReactElement {
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
      Array.from(dialogElement.querySelectorAll<HTMLElement>("a[href], button:not([disabled])"));
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
        {details.length === 0 ? null : (
          <dl className="dialog-target">
            {details.map((detail) => (
              <div key={detail.label}>
                <dt>{detail.label}</dt>
                <dd>
                  {detail.href === undefined ? (
                    detail.value
                  ) : (
                    <a href={detail.href} target="_blank" rel="noopener noreferrer">
                      {detail.value}
                    </a>
                  )}
                </dd>
              </div>
            ))}
          </dl>
        )}
        <p className="dialog-message">{message}</p>
        <OfflineNote disconnected={disconnected} />
        <div className="actions">
          <button type="button" disabled={busy} onClick={cancel}>
            やめる
          </button>
          <button type="button" disabled={busy || disconnected} onClick={() => onAction({ type: "confirm", dialog })}>
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
  details: ConfirmDialogDetail[];
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

// 種別ごとの見出し・対象・本文・確定ボタン名。本文は対象の値（PR 番号・revision）を差し込む。
// マージは、別のプロジェクトのプルリクエストと取り違えないよう、プロジェクトと、
// プルリクエストを新しいタブで開くリンクを対象として出す（issue #304）。
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
    const prUrl =
      dialogTarget?.kind === "merge" ? dialogTarget.url : pullRequestLinkUrl(decision.pullRequest?.url);
    return [
      {
        dialog: "merge",
        title: "マージの承認",
        details: [
          { label: "プロジェクト", value: decision.project },
          { label: "プルリクエスト", value: `#${prNumber ?? ""} を開く（新しいタブ）`, href: prUrl },
        ],
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
      details: [],
      message: `revision ${revision ?? ""} のゴール登録を承認します。よろしいですか。`,
      confirmLabel: "承認",
    },
    {
      dialog: "reject-registration",
      title: "ゴール登録の却下",
      details: [],
      message: `revision ${revision ?? ""} のゴール登録を却下します。よろしいですか。`,
      confirmLabel: "却下",
    },
  ];
}

// 一覧に同じ操作名のボタンが複数並んでも、読み上げだけで操作と対象の対応が取れるよう、
// 対象の説明をボタン外の要素に置き、各ボタンから aria-describedby で参照する。
// ボタンの読み上げ名は操作の文言だけにする（issue #324、候補1を採用）。
function DecisionTargetDescription({ id, targetLabel }: { id: string; targetLabel: string }): ReactElement {
  return (
    <span id={id} className="visually-hidden">
      対象: {targetLabel}
    </span>
  );
}

type DialogActionSectionProps = {
  triggers: DialogTrigger[];
  configs: ConfirmDialogConfig[];
  active: DialogKind | null;
  descriptionId: string;
  targetLabel: string;
  busy: boolean;
  disconnected: boolean;
  triggerRefs: RefObject<Record<DialogKind, HTMLButtonElement | null>>;
  onAction: (intent: DecisionActionsIntent) => void;
};

// 確認ダイアログを開くトリガー群と、いま開いている種別のダイアログをまとめて描画する。
// ボタン名は操作の文言だけにし、対象の識別子は aria-describedby で補う。
function DialogActionSection({
  triggers,
  configs,
  active,
  descriptionId,
  targetLabel,
  busy,
  disconnected,
  triggerRefs,
  onAction,
}: DialogActionSectionProps): ReactElement {
  const current = configs.find((config) => config.dialog === active);
  return (
    <div className="actions">
      <DecisionTargetDescription id={descriptionId} targetLabel={targetLabel} />
      {triggers.map(({ dialog, label }) => (
        <button
          key={dialog}
          type="button"
          disabled={busy || disconnected}
          ref={(element) => {
            triggerRefs.current[dialog] = element;
          }}
          aria-label={label}
          aria-describedby={descriptionId}
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
          details={current.details}
          confirmLabel={current.confirmLabel}
          busy={busy}
          disconnected={disconnected}
          onAction={onAction}
        />
      )}
      <OfflineNote disconnected={disconnected} />
    </div>
  );
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
      return (
        <div className="actions">
          <DecisionTargetDescription id={descriptionId} targetLabel={descriptionLabel} />
          <button
            type="button"
            disabled={busy || disconnected}
            aria-label="実施した"
            aria-describedby={descriptionId}
            onClick={() => onAction({ type: "answer-report", report: OPERATION_ANSWER_REPORT.COMPLETED })}
          >
            実施した
          </button>
          <button
            type="button"
            disabled={busy || disconnected}
            aria-label="対応できない"
            aria-describedby={descriptionId}
            onClick={() => onAction({ type: "answer-report", report: OPERATION_ANSWER_REPORT.UNABLE })}
          >
            対応できない
          </button>
          <OfflineNote disconnected={disconnected} />
        </div>
      );

    case "merge":
    case "goal-registration": {
      const group = state.kind;
      return (
        <DialogActionSection
          triggers={group === "merge" ? DIALOG_TRIGGERS.merge : DIALOG_TRIGGERS.goalRegistration}
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

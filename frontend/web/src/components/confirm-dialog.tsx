"use client";

// 確認ダイアログ本体と、そのトリガー群・種別ごとの文面の組み立て。
// decision-actions-view.tsx から分けたファイルで、振る舞いは分け方前と同じである。
// フォーカスの封じ込めと復帰先の決定（decideDialogKey）、開いた直後の確定の守りをここが担う。
import { useCallback, useEffect, useId, useRef, type ReactElement, type RefObject } from "react";
import type { PendingDecision } from "../proto/taktdash";
import {
  pullRequestLinkUrl,
  type DecisionDialogKind,
  type DecisionDialogTarget,
  type DecisionIntent,
} from "../state/decision-actions";
import { goalHeading } from "../state/goal-summary";

export type DialogKind = DecisionDialogKind;

export type DecisionActionsIntent = DecisionIntent;

// 切断中に操作要素へ添える理由。再接続すると disconnected が戻り、操作も戻る。
export function OfflineNote({ disconnected }: { disconnected: boolean }): ReactElement | null {
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
  onAction: (intent: DecisionActionsIntent) => boolean;
};

export type DialogKeyDecision =
  | { kind: "cancel" }
  | { kind: "none" }
  | { kind: "focus"; target: "first" | "last" | "heading" };

// ダイアログ表示中の Tab / Shift+Tab / Escape を、キーとフォーカスの現在位置だけから決める。
// DOM に依存しないため、ダイアログ外フォーカス時の Tab 先を単体テストで確認できる。
export function decideDialogKey(input: {
  key: string;
  /** 送信中は「やめる」が無効なので、Escape でも取り消さない（不変条件2）。 */
  busy: boolean;
  shiftKey: boolean;
  tabbableCount: number;
  activeIndex: number;
}): DialogKeyDecision {
  if (input.key === "Escape") {
    return input.busy ? { kind: "none" } : { kind: "cancel" };
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

// ダイアログが出てから確定を受け付けるまでの時間（ミリ秒）。一覧のボタンを2回すばやく押したとき、
// 2回目が同じ座標に出た確定に当たっても送らないため、すべての確認に共通で置く（issue #308）。
const CONFIRM_ACCEPT_DELAY_MS = 500;

// 確定の受付を始めたかどうかを持つ。初めは受け付けず、setTimeout で 500ms 後に
// 受け付けを始める。壁時計には頼らない。閉じるときにタイマーを解くため、前に開いた
// ダイアログのタイマーが、開き直したダイアログを早く受け付けにしない。開くたびに
// ConfirmDialog が作り直される（DialogActionSection が種別を key にする）ため数え直す。
function useConfirmAcceptance(): () => boolean {
  const acceptingConfirm = useRef(false);
  useEffect(() => {
    acceptingConfirm.current = false;
    const timer = setTimeout(() => {
      acceptingConfirm.current = true;
    }, CONFIRM_ACCEPT_DELAY_MS);
    return () => {
      clearTimeout(timer);
    };
  }, []);
  return useCallback((): boolean => acceptingConfirm.current, []);
}

function ConfirmDialog({ dialog, title, message, details, confirmLabel, busy, disconnected, onAction }: ConfirmDialogProps): ReactElement {
  const headingId = useId();
  const headingRef = useRef<HTMLHeadingElement>(null);
  const dialogRef = useRef<HTMLDivElement>(null);
  const isAcceptingConfirm = useConfirmAcceptance();
  // close を送出したら封じ込めを止める。閉じる途中の focusout で消えゆくダイアログへ
  // フォーカスを戻し、トリガーへの復帰と競合させない。
  const released = useRef(false);
  // キー処理は document に一度だけ登録するため、送信中かどうかは ref で読む。
  const busyRef = useRef(busy);
  useEffect(() => {
    busyRef.current = busy;
  }, [busy]);
  const cancel = useCallback((): void => {
    if (onAction({ type: "cancel" })) {
      released.current = true;
    }
  }, [onAction]);
  // 開いた直後の確定は受け付けない。送らず、ダイアログも閉じない。ボタンは disabled に
  // しない（無効化は送信中か切断中だけで決める）。やめると Escape はこの間も受け付ける。
  const confirm = useCallback((): void => {
    if (released.current) {
      return;
    }
    if (!isAcceptingConfirm()) {
      return;
    }
    onAction({ type: "confirm", dialog });
  }, [isAcceptingConfirm, dialog, onAction]);

  // 開いた直後にダイアログの見出しへフォーカスを移し、読み上げとキーボード操作の起点にする。
  useEffect(() => {
    headingRef.current?.focus();
  }, []);

  // 送信中はボタンが無効になり、押したボタンからフォーカスが外れる。見出しへ戻して
  // ダイアログ内にフォーカスを保つ。切断で確定ボタンが無効になったときも、そこに
  // フォーカスがあれば見出しへ移す。押せる「やめる」やリンクにあるフォーカスは動かさない。
  useEffect(() => {
    const active = document.activeElement;
    const lostFocus =
      active === null ||
      !(dialogRef.current?.contains(active) ?? false) ||
      (active instanceof HTMLButtonElement && active.disabled);
    if (busy || (disconnected && lostFocus)) {
      headingRef.current?.focus();
    }
  }, [busy, disconnected]);

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
        busy: busyRef.current,
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
          <button type="button" className="danger" disabled={busy || disconnected} onClick={confirm}>
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

export const DIALOG_TRIGGERS = {
  merge: [{ dialog: "merge", label: "マージする" }],
  goalRegistration: [
    { dialog: "approve-registration", label: "ゴール登録を承認する" },
    { dialog: "reject-registration", label: "ゴール登録を却下する" },
  ],
  operation: [
    { dialog: "answer-report-completed", label: "実施した" },
    { dialog: "answer-report-unable", label: "対応できない" },
  ],
} as const satisfies Record<string, DialogTrigger[]>;

type DialogGroup = "merge" | "goal-registration" | "operation";

// ゴール登録の対象。登録案の名前は本文の最初の1文（goalHeading）で、本文が空なら行を出さない。
function registrationDetails(decision: PendingDecision): ConfirmDialogDetail[] {
  const details: ConfirmDialogDetail[] = [{ label: "プロジェクト", value: decision.project }];
  const body = typeof decision.body === "string" ? decision.body : "";
  if (body.trim() !== "") {
    details.push({ label: "登録案", value: goalHeading(body) });
  }
  return details;
}

// 種別ごとの見出し・対象・本文・確定ボタン名。本文は対象の値（PR 番号・revision）を差し込む。
// マージは、別のプロジェクトのプルリクエストと取り違えないよう、プロジェクトと、
// プルリクエストを新しいタブで開くリンクを対象として出す（issue #304）。
// 確認ダイアログを開いた時点の値を dialogTarget に保っている場合、開いた後に SSE で
// 一覧が更新されても、確認する対象は開いた時点の値を表示する。
export function buildConfirmDialogs(
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
        title: "プルリクエストのマージ",
        details: [
          { label: "プロジェクト", value: decision.project },
          { label: "プルリクエスト", value: `#${prNumber ?? ""} を開く（新しいタブ）`, href: prUrl },
        ],
        message: `プルリクエスト #${prNumber ?? ""} をマージします。よろしいですか。`,
        confirmLabel: "マージする",
      },
    ];
  }
  if (group === "operation") {
    return [
      {
        dialog: "answer-report-completed",
        title: "作業の依頼への回答（実施した）",
        details: [{ label: "プロジェクト", value: decision.project }],
        message: "作業の依頼に「実施した」と回答します。よろしいですか。",
        confirmLabel: "実施した",
      },
      {
        dialog: "answer-report-unable",
        title: "作業の依頼への回答（対応できない）",
        details: [{ label: "プロジェクト", value: decision.project }],
        message: "作業の依頼に「対応できない」と回答します。よろしいですか。",
        confirmLabel: "対応できない",
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
      details: registrationDetails(decision),
      message: `revision ${revision ?? ""} のゴール登録を承認します。よろしいですか。`,
      confirmLabel: "ゴール登録を承認する",
    },
    {
      dialog: "reject-registration",
      title: "ゴール登録の却下",
      details: registrationDetails(decision),
      message: `revision ${revision ?? ""} のゴール登録を却下します。よろしいですか。`,
      confirmLabel: "ゴール登録を却下する",
    },
  ];
}

// 一覧に同じ操作名のボタンが複数並んでも、読み上げだけで操作と対象の対応が取れるよう、
// 対象の説明をボタン外の要素に置き、各ボタンから aria-describedby で参照する。
// ボタンの読み上げ名は操作の文言だけにする（issue #324、候補1を採用）。
export function DecisionTargetDescription({ id, targetLabel }: { id: string; targetLabel: string }): ReactElement {
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
  onAction: (intent: DecisionActionsIntent) => boolean;
};

// 確認ダイアログを開くトリガー群と、いま開いている種別のダイアログをまとめて描画する。
// ボタン名は操作の文言だけにし、対象の識別子は aria-describedby で補う。
export function DialogActionSection({
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
          key={current.dialog}
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

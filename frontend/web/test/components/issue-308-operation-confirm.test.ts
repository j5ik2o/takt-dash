// issue #308: 作業の依頼の2択（実施した・対応できない）は、押してもすぐには送らず、
// マージやゴール登録と同じ確認ダイアログを開く。確定したときだけ送り、やめたら送らない。
// 「回答する」（自由記述）と選択肢の回答は、これまでどおりすぐに送る。
//
// 前半は DOM を持たない遷移（decideDecisionTransition）とサーバー描画で確かめる。
// 後半は判断待ちのページ（DecisionsContent）を jsdom の DOM へマウントし、本物のボタンを
// 押して、書き込み（fetch）が何回、どの内容で、どの接続状態で出たかを観測する。
// 確認の種別の名前は実装が決める。テストは、2択の意図から開いたダイアログの状態を
// そのまま使い、種別の名前を書かない。
import test, { mock } from "node:test";
import assert from "node:assert/strict";
import { act, createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { createRoot, type Root } from "react-dom/client";
import { JSDOM } from "jsdom";
import { PendingDecisionType, type DashboardState, type PendingDecision } from "../../src/proto/taktdash.ts";
import { createActionableDashboardState, NOW } from "../support/fixtures.ts";
import { DecisionsContent } from "../../src/components/decisions-content.tsx";
import {
  DecisionActionsView,
  decideDecisionTransition,
  type DecisionTransition,
} from "../../src/components/decision-actions-view.tsx";
import {
  INITIAL_DECISION_DIALOG_STATE,
  type DecisionDialogState,
} from "../../src/state/decision-actions.ts";
import { OPERATION_ANSWER_REPORT } from "../../src/state/proto-values.ts";

const decisions = createActionableDashboardState().pendingDecisions;

const findDecision = (inquiryId: string): PendingDecision => {
  const found = decisions.find((decision) => decision.inquiryId === inquiryId);
  assert.ok(found, `フィクスチャに ${inquiryId} がある`);
  return found;
};

const operation = (): PendingDecision => findDecision("inquiry-operation");
const merge = (): PendingDecision => {
  const found = decisions.find((decision) => decision.type === PendingDecisionType.PENDING_DECISION_TYPE_AWAITING_MERGE);
  assert.ok(found, "フィクスチャにマージ待ちがある");
  return found;
};
const registration = (): PendingDecision => {
  const found = decisions.find(
    (decision) =>
      decision.type === PendingDecisionType.PENDING_DECISION_TYPE_GOAL_REGISTRATION && decision.revision === 7,
  );
  assert.ok(found, "フィクスチャに revision 7 のゴール登録がある");
  return found;
};

// 確認ダイアログが開いてから、確定を受け付けるまでの時間（docs/design/issue-308.md）。
// 一覧のボタンを2回すばやく押したとき、2回目が、同じ座標に出た確定に当たっても送らない。
const CONFIRM_DELAY_MS = 500;

type Report = (typeof OPERATION_ANSWER_REPORT)[keyof typeof OPERATION_ANSWER_REPORT];

// 2択のボタンの名前と、確認ダイアログの文言の案（docs/design/issue-308.md）。
// 書き方はゴール登録の確認にそろえる。見出しは「対象への操作」、本文は「〜します。よろしいですか。」、
// 確定のボタンは押したボタンと同じ言葉にする。対象のプロジェクトをマージの確認と同じく添える。
const CHOICES: { report: Report; trigger: string; title: string; message: string; confirmLabel: string }[] = [
  {
    report: OPERATION_ANSWER_REPORT.COMPLETED,
    trigger: "実施した",
    title: "作業の依頼への回答（実施した）",
    message: "作業の依頼に「実施した」と回答します。よろしいですか。",
    confirmLabel: "実施した",
  },
  {
    report: OPERATION_ANSWER_REPORT.UNABLE,
    trigger: "対応できない",
    title: "作業の依頼への回答（対応できない）",
    message: "作業の依頼に「対応できない」と回答します。よろしいですか。",
    confirmLabel: "対応できない",
  },
];

const OFFLINE_NOTE = "再接続するまで操作できません。";

const transition = (decision: PendingDecision, intent: Parameters<typeof decideDecisionTransition>[0]["intent"], state: DecisionDialogState): DecisionTransition =>
  decideDecisionTransition({ decision, intent, text: "", state });

// 2択のボタンが出す意図から、開いたダイアログの状態を得る。開かずに送るなら失敗にする。
const openByReport = (decision: PendingDecision, report: Report): Extract<DecisionDialogState, { kind: "open" }> => {
  const opened = transition(decision, { type: "answer-report", report }, INITIAL_DECISION_DIALOG_STATE);
  assert.equal(opened.kind, "state", `report ${report} は送らずにダイアログを開く: ${JSON.stringify(opened)}`);
  if (opened.kind !== "state") {
    throw new Error("2択はダイアログを開く");
  }
  assert.equal(opened.state.kind, "open", "2択はダイアログを開いた状態にする");
  if (opened.state.kind !== "open") {
    throw new Error("2択はダイアログを開いた状態にする");
  }
  assert.notEqual(opened.state.target, null, "開いた時点の対象を持つ");
  return opened.state;
};

test("作業の依頼の2択は、送らずに確認ダイアログを開く", () => {
  const dialogs = CHOICES.map(({ report }) => openByReport(operation(), report));
  // 実施した と 対応できない は別の確認になる（どちらの確認かを状態から区別できる）。
  assert.notDeepEqual(dialogs[0], dialogs[1]);
});

test("作業の依頼の確認は、確定したら開いた2択の report を1回分の回答として送る", () => {
  const decision = operation();
  for (const { report } of CHOICES) {
    const opened = openByReport(decision, report);
    const confirmed = transition(decision, { type: "confirm", dialog: opened.dialog }, opened);
    assert.equal(confirmed.kind, "submit", `report ${report} の確定は送る`);
    if (confirmed.kind !== "submit") {
      throw new Error("確定は送る");
    }
    assert.deepEqual(confirmed.action, {
      kind: "answer",
      payload: {
        serverId: decision.serverId,
        project: decision.project,
        goalId: decision.goalId,
        inquiryId: decision.inquiryId,
        report,
      },
    });
  }
});

test("作業の依頼の確認は、やめたら送らずに閉じる", () => {
  for (const { report } of CHOICES) {
    const opened = openByReport(operation(), report);
    assert.deepEqual(transition(operation(), { type: "cancel" }, opened), {
      kind: "state",
      state: INITIAL_DECISION_DIALOG_STATE,
    });
  }
});

test("作業の依頼の確認は、ダイアログが閉じている状態からの確定では送らない", () => {
  for (const { report } of CHOICES) {
    const { dialog } = openByReport(operation(), report);
    const confirmed = transition(operation(), { type: "confirm", dialog }, INITIAL_DECISION_DIALOG_STATE);
    assert.notEqual(confirmed.kind, "submit", `閉じた状態の確定 ${dialog}`);
  }
});

test("選択肢と自由記述の回答は、これまでどおり確認を挟まずに送る", () => {
  const options = transition(findDecision("inquiry-options"), { type: "answer-option", option: "今週" }, INITIAL_DECISION_DIALOG_STATE);
  assert.equal(options.kind, "submit");
  const text = decideDecisionTransition({
    decision: findDecision("inquiry-text"),
    intent: { type: "answer-text" },
    text: "補足します",
    state: INITIAL_DECISION_DIALOG_STATE,
  });
  assert.equal(text.kind, "submit");
});

const renderView = (
  state: Extract<DecisionDialogState, { kind: "open" }> | null,
  busy: boolean,
  disconnected: boolean,
): Document => {
  const html = renderToStaticMarkup(
    createElement(DecisionActionsView, {
      decision: operation(),
      text: "",
      dialog: state?.dialog ?? null,
      dialogTarget: state?.target ?? null,
      busy,
      disconnected,
      onTextChange: () => {},
      onAction: () => {},
    }),
  );
  return new JSDOM(html).window.document;
};

const outsideDialog = (document: Document, name: string): HTMLButtonElement[] =>
  Array.from(document.querySelectorAll<HTMLButtonElement>("button")).filter(
    (button) => button.closest('[role="dialog"]') === null && button.getAttribute("aria-label") === name,
  );

test("作業の依頼の確認ダイアログは、案の見出し・本文・確定のボタン・やめる・プロジェクトを出す", () => {
  for (const choice of CHOICES) {
    const document = renderView(openByReport(operation(), choice.report), false, false);
    const dialog = document.querySelector('[role="dialog"]');
    assert.ok(dialog, `${choice.trigger} の確認ダイアログがある`);
    assert.equal(dialog.getAttribute("aria-modal"), "true");
    const heading = dialog.querySelector("h2");
    assert.equal(heading?.textContent, choice.title);
    assert.equal(heading?.getAttribute("tabindex"), "-1");
    assert.equal(dialog.getAttribute("aria-labelledby"), heading?.id);
    assert.equal(dialog.querySelector(".dialog-message")?.textContent, choice.message);
    const target = Array.from(dialog.querySelectorAll(".dialog-target > div")).map((row) => [
      row.querySelector("dt")?.textContent,
      row.querySelector("dd")?.textContent,
    ]);
    assert.deepEqual(target, [["プロジェクト", operation().project]]);
    const buttons = Array.from(dialog.querySelectorAll("button")).map((button) => button.textContent);
    assert.deepEqual(buttons, ["やめる", choice.confirmLabel]);
    // 押したボタンは一覧に残り、ダイアログの外に1つだけある。
    assert.equal(outsideDialog(document, choice.trigger).length, 1);
  }
});

test("作業の依頼は、閉じた状態では2択のボタンだけを出し、ダイアログを出さない", () => {
  const document = renderView(null, false, false);
  assert.equal(document.querySelector('[role="dialog"]'), null);
  for (const { trigger } of CHOICES) {
    assert.equal(outsideDialog(document, trigger).length, 1);
  }
});

// 性質: 作業の依頼の確認の、送信中・切断のすべての組み合わせで、2択のボタンと確定は
// 送信中か切断中なら押せず、やめるは送信中だけ押せない。切断中は理由をダイアログの中と
// 外に1つずつ添える（issue #307 と同じ規則）。除外は付けない。
test("性質: 作業の依頼の確認は、送信中か切断中なら確定できず、やめるは送信中だけ押せない", () => {
  for (const choice of CHOICES) {
    const opened = openByReport(operation(), choice.report);
    for (const state of [null, opened]) {
      for (const busy of [false, true]) {
        for (const disconnected of [false, true]) {
          const where = JSON.stringify({ trigger: choice.trigger, open: state !== null, busy, disconnected });
          const document = renderView(state, busy, disconnected);
          for (const { trigger } of CHOICES) {
            for (const button of outsideDialog(document, trigger)) {
              assert.equal(button.disabled, busy || disconnected, `${trigger} ${where}`);
            }
          }
          const dialog = document.querySelector('[role="dialog"]');
          assert.equal(dialog !== null, state !== null, `ダイアログ ${where}`);
          if (dialog !== null) {
            const [cancel, confirm, ...rest] = Array.from(dialog.querySelectorAll("button"));
            assert.equal(rest.length, 0);
            assert.equal(cancel?.textContent, "やめる");
            assert.equal(cancel?.disabled, busy, `やめる ${where}`);
            assert.equal(confirm?.textContent, choice.confirmLabel);
            assert.equal(confirm?.disabled, busy || disconnected, `確定 ${where}`);
          }
          const notes = (document.body.textContent ?? "").split(OFFLINE_NOTE).length - 1;
          assert.equal(notes, disconnected ? (state === null ? 1 : 2) : 0, `理由の文 ${where}`);
        }
      }
    }
  }
});

// ---- ここから、判断待ちのページを DOM へマウントして本物のボタンを押す ----

// react-dom/client のマウントに要る DOM を jsdom で用意し、グローバルへ公開する
// （live-provider-wiring.test.ts と同じ手順）。
let domInstalled = false;
const installDomEnvironment = (): void => {
  if (domInstalled) {
    return;
  }
  domInstalled = true;
  const dom = new JSDOM("<!doctype html><html><body></body></html>", { url: "http://localhost/" });
  const scope = globalThis as unknown as Record<string, unknown>;
  const win = dom.window as unknown as Record<string, unknown>;
  for (const key of Object.getOwnPropertyNames(dom.window)) {
    if (key in scope) {
      continue;
    }
    try {
      scope[key] = win[key];
    } catch {
      // 読み出せないプロパティはマウントに不要として飛ばす。
    }
  }
  for (const [key, value] of [
    ["window", dom.window],
    ["document", dom.window.document],
    ["navigator", dom.window.navigator],
  ] as const) {
    Object.defineProperty(scope, key, { value, configurable: true, writable: true });
  }
  scope.IS_REACT_ACT_ENVIRONMENT = true;
};

// 送った書き込み。disconnected は、送った時点で画面が切断中だったか。
type SentWrite = { url: string; body: Record<string, unknown>; disconnected: boolean };

// 判断待ちのページを判断待ち1件（既定は作業の依頼）でマウントし、書き込みを fetch の差し替えで受ける。
// 書き込みの応答は settle を呼ぶまで返さない（送信中を作る）。
// 時計（Date と setTimeout）は node:test の mock.timers で止め、wait で進める。確定を受け付けるまでの
// 時間は、wait で進めた分だけ過ぎる。
const mountOperationPage = async (decision: PendingDecision = operation()) => {
  installDomEnvironment();
  mock.timers.enable({ apis: ["Date", "setTimeout"], now: NOW.getTime() });
  const state: DashboardState = { ...createActionableDashboardState(), pendingDecisions: [decision] };
  const sent: SentWrite[] = [];
  const pending: (() => void)[] = [];
  let disconnected = false;
  (globalThis as unknown as { fetch: unknown }).fetch = (url: string, init?: { body?: string }) => {
    sent.push({ url, body: JSON.parse(init?.body ?? "{}") as Record<string, unknown>, disconnected });
    return new Promise((resolve) => {
      pending.push(() =>
        resolve({ ok: true, json: async () => ({ ok: true }) } as unknown as Response),
      );
    });
  };
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root: Root = createRoot(container);
  const render = async (): Promise<void> => {
    await act(async () => {
      root.render(createElement(DecisionsContent, { snapshotLoaded: true, state, now: NOW, disconnected }));
    });
  };
  await render();

  const dialog = (): HTMLElement | null => container.querySelector<HTMLElement>('[role="dialog"]');
  const trigger = (name: string): HTMLButtonElement => {
    const found = Array.from(container.querySelectorAll<HTMLButtonElement>("button")).filter(
      (button) => button.closest('[role="dialog"]') === null && button.getAttribute("aria-label") === name,
    );
    assert.equal(found.length, 1, `一覧に「${name}」のボタンが1つある`);
    return found[0]!;
  };
  const dialogButton = (label: "cancel" | "confirm"): HTMLButtonElement | undefined => {
    const buttons = Array.from(dialog()?.querySelectorAll<HTMLButtonElement>("button") ?? []);
    return label === "cancel"
      ? buttons.find((button) => button.textContent === "やめる")
      : buttons.find((button) => button.textContent !== "やめる");
  };
  const click = async (element: HTMLElement | undefined): Promise<void> => {
    if (element === undefined) {
      return;
    }
    await act(async () => {
      element.click();
    });
  };
  return {
    container,
    sent,
    dialog,
    trigger,
    dialogButton,
    click,
    get disconnected() {
      return disconnected;
    },
    setDisconnected: async (next: boolean): Promise<void> => {
      disconnected = next;
      await render();
    },
    escape: async (): Promise<void> => {
      await act(async () => {
        document.dispatchEvent(new window.KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
      });
    },
    wait: async (ms: number): Promise<void> => {
      await act(async () => {
        mock.timers.tick(ms);
      });
    },
    // 端末の時計の補正をまねる。Date だけを ms ずらし、時間は過ぎない（setTimeout は動かない）。
    jumpWallClock: async (ms: number): Promise<void> => {
      await act(async () => {
        mock.timers.setTime(Date.now() + ms);
      });
    },
    settle: async (): Promise<void> => {
      await act(async () => {
        for (const resolve of pending.splice(0)) {
          resolve();
        }
      });
    },
    unmount: async (): Promise<void> => {
      await act(async () => {
        root.unmount();
      });
      container.remove();
      mock.timers.reset();
    },
  };
};

test("判断待ちのページで作業の依頼のボタンを押すと、送らずに案の文言の確認ダイアログが出て、見出しへフォーカスが移る", async () => {
  for (const choice of CHOICES) {
    const page = await mountOperationPage();
    try {
      await page.click(page.trigger(choice.trigger));
      assert.equal(page.sent.length, 0, `${choice.trigger} を押しただけでは送らない`);
      const dialog = page.dialog();
      assert.ok(dialog, `${choice.trigger} を押すと確認ダイアログが出る`);
      const heading = dialog.querySelector("h2");
      assert.equal(heading?.textContent, choice.title);
      assert.equal(dialog.querySelector(".dialog-message")?.textContent, choice.message);
      assert.equal(page.dialogButton("confirm")?.textContent, choice.confirmLabel);
      assert.equal(document.activeElement, heading, "開いた直後は見出しにフォーカスがある");
    } finally {
      await page.unmount();
    }
  }
});

test("判断待ちのページで作業の依頼を確定すると、開いた2択の report で書き込みが1回だけ出て、結果が出たら閉じる", async () => {
  for (const choice of CHOICES) {
    const page = await mountOperationPage();
    try {
      await page.click(page.trigger(choice.trigger));
      assert.equal(page.sent.length, 0, `${choice.trigger} を押しただけでは送らない`);
      assert.ok(page.dialogButton("confirm"), "確認ダイアログに確定のボタンがある");
      await page.wait(CONFIRM_DELAY_MS);
      await page.click(page.dialogButton("confirm"));
      // 送信中に確定をもう一度押しても、書き込みは増えない。
      await page.click(page.dialogButton("confirm"));
      assert.equal(page.sent.length, 1, `${choice.trigger} の確定は1回だけ送る`);
      assert.equal(page.sent[0]?.url, "/api/decisions/answer");
      assert.equal(page.sent[0]?.body.report, choice.report);
      assert.equal(page.sent[0]?.body.inquiryId, operation().inquiryId);
      await page.settle();
      assert.equal(page.dialog(), null, "結果が返ったらダイアログを閉じる");
      assert.match(page.container.textContent ?? "", /回答を送信しました。/);
      assert.equal(page.sent.length, 1);
    } finally {
      await page.unmount();
    }
  }
});

test("判断待ちのページで作業の依頼の確認をやめると、送らずに閉じ、押したボタンへフォーカスが戻る", async () => {
  for (const choice of CHOICES) {
    for (const close of ["やめる", "Escape"] as const) {
      const page = await mountOperationPage();
      try {
        await page.click(page.trigger(choice.trigger));
        assert.ok(page.dialog(), `${choice.trigger} を押すと確認ダイアログが出る`);
        if (close === "やめる") {
          await page.click(page.dialogButton("cancel"));
        } else {
          await page.escape();
        }
        assert.equal(page.dialog(), null, `${close} で閉じる`);
        assert.equal(page.sent.length, 0, `${close} では送らない`);
        assert.equal(document.activeElement, page.trigger(choice.trigger), `${close} の後は押したボタンへ戻る`);
      } finally {
        await page.unmount();
      }
    }
  }
});

test("判断待ちのページで作業の依頼の確認を開いたまま切断すると、確定できず、再接続すれば確定できる", async () => {
  const page = await mountOperationPage();
  try {
    await page.click(page.trigger("対応できない"));
    assert.ok(page.dialog(), "確認ダイアログが出る");
    await page.wait(CONFIRM_DELAY_MS);
    await page.setDisconnected(true);
    assert.equal(page.dialogButton("confirm")?.disabled, true, "切断中は確定を押せない");
    assert.equal(page.dialogButton("cancel")?.disabled, false, "切断中もやめるは押せる");
    await page.click(page.dialogButton("confirm"));
    assert.equal(page.sent.length, 0, "切断中は送らない");
    await page.setDisconnected(false);
    await page.click(page.dialogButton("confirm"));
    assert.equal(page.sent.length, 1);
    assert.equal(page.sent[0]?.disconnected, false);
    assert.equal(page.sent[0]?.body.report, OPERATION_ANSWER_REPORT.UNABLE);
  } finally {
    await page.unmount();
  }
});

// 確認ダイアログを開く一覧のボタンと、確定で届く書き込み。マージとゴール登録の確認も同じ
// ConfirmDialog を使うので、開いた直後の確定を受け付けない直しは、すべての確認に効く。
const CONFIRM_FLOWS: { name: string; decision: () => PendingDecision; trigger: string; url: string }[] = [
  { name: "作業の依頼（実施した）", decision: operation, trigger: "実施した", url: "/api/decisions/answer" },
  { name: "作業の依頼（対応できない）", decision: operation, trigger: "対応できない", url: "/api/decisions/answer" },
  { name: "マージ", decision: merge, trigger: "マージを承認", url: "/api/decisions/merge" },
  { name: "ゴール登録の承認", decision: registration, trigger: "承認", url: "/api/decisions/goal-registration/approve" },
  { name: "ゴール登録の却下", decision: registration, trigger: "却下", url: "/api/decisions/goal-registration/reject" },
];

// 一覧のボタンを2回すばやく押すと、1回目で確認ダイアログが開き、2回目のクリックが、同じ座標に
// 出た確定のボタンに当たることがある（Devin Review、PR #404）。開いてから CONFIRM_DELAY_MS の間の
// 確定は送らず、ダイアログを開いたままにする。過ぎた後の確定は1回だけ送る。
test("判断待ちのページで確認を開いた直後の確定は送らず、開いてから 500ms 過ぎた確定は1回だけ送る", async () => {
  for (const flow of CONFIRM_FLOWS) {
    const page = await mountOperationPage(flow.decision());
    try {
      await page.click(page.trigger(flow.trigger));
      assert.ok(page.dialog(), `${flow.name}: 押すと確認ダイアログが出る`);
      // 二重クリックの2回目。開いたのと同じ時刻に確定へ当たる。
      await page.click(page.dialogButton("confirm"));
      assert.equal(page.sent.length, 0, `${flow.name}: 開いた直後の確定は送らない`);
      assert.ok(page.dialog(), `${flow.name}: 開いた直後の確定ではダイアログを閉じない`);
      assert.equal(page.dialogButton("cancel")?.disabled, false, `${flow.name}: やめるは押せる`);
      await page.wait(CONFIRM_DELAY_MS - 1);
      await page.click(page.dialogButton("confirm"));
      assert.equal(page.sent.length, 0, `${flow.name}: 開いてから ${CONFIRM_DELAY_MS - 1}ms の確定も送らない`);
      await page.wait(1);
      await page.click(page.dialogButton("confirm"));
      assert.equal(page.sent.length, 1, `${flow.name}: 開いてから ${CONFIRM_DELAY_MS}ms 過ぎた確定は送る`);
      assert.equal(page.sent[0]?.url, flow.url, flow.name);
    } finally {
      await page.unmount();
    }
  }
});

// 待つ時間は、開くたびに数え直す。前に開いたダイアログで待った分は、次に開いたダイアログに持ち越さない。
test("判断待ちのページで確認をやめて開き直すと、開き直した直後の確定は送らない", async () => {
  for (const flow of CONFIRM_FLOWS) {
    const page = await mountOperationPage(flow.decision());
    try {
      await page.click(page.trigger(flow.trigger));
      await page.wait(CONFIRM_DELAY_MS * 2);
      await page.click(page.dialogButton("cancel"));
      assert.equal(page.dialog(), null, `${flow.name}: やめると閉じる`);
      await page.click(page.trigger(flow.trigger));
      assert.ok(page.dialog(), `${flow.name}: 開き直す`);
      await page.click(page.dialogButton("confirm"));
      assert.equal(page.sent.length, 0, `${flow.name}: 開き直した直後の確定は送らない`);
      await page.wait(CONFIRM_DELAY_MS);
      await page.click(page.dialogButton("confirm"));
      assert.equal(page.sent.length, 1, `${flow.name}: 開き直してから待てば送る`);
    } finally {
      await page.unmount();
    }
  }
});

// 確定を受け付けるまでの時間は、過ぎた時間で数え、壁時計（Date）では数えない（CodeRabbit、PR #404）。
// 開いた直後に端末の時計が進んでも、確定は受け付けない。mock.timers は Date と setTimeout を
// 1つの時計で動かすので、時計が戻る場合（戻すと setTimeout も遅れる）はここでは作れない。
test("判断待ちのページで確認を開いた直後に壁時計が進んでも、確定を受け付けるのは開いてから 500ms 過ぎてから", async () => {
  for (const flow of CONFIRM_FLOWS) {
    const page = await mountOperationPage(flow.decision());
    const jump = CONFIRM_DELAY_MS * 2;
    const name = `${flow.name}（壁時計を ${jump}ms 進める）`;
    try {
      await page.click(page.trigger(flow.trigger));
      assert.ok(page.dialog(), `${name}: 押すと確認ダイアログが出る`);
      await page.jumpWallClock(jump);
      await page.click(page.dialogButton("confirm"));
      assert.equal(page.sent.length, 0, `${name}: 開いた直後の確定は送らない`);
      assert.ok(page.dialog(), `${name}: 開いた直後の確定ではダイアログを閉じない`);
      await page.wait(CONFIRM_DELAY_MS);
      await page.click(page.dialogButton("confirm"));
      assert.equal(page.sent.length, 1, `${name}: 開いてから ${CONFIRM_DELAY_MS}ms 過ぎた確定は送る`);
      assert.equal(page.sent[0]?.url, flow.url, name);
    } finally {
      await page.unmount();
    }
  }
});

// 種を決めた疑似乱数（mulberry32）。失敗したときに、種と操作の列から同じ列を作り直せる。
const random = (seed: number): (() => number) => {
  let state = seed >>> 0;
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
};

const OPERATIONS = [
  "press-completed",
  "press-unable",
  "double-press-completed",
  "double-press-unable",
  // 2択を押し、時計を進めずに確定を押す。二重クリックの2回目が確定に当たったときに当たる。
  "press-then-confirm-completed",
  "press-then-confirm-unable",
  "cancel",
  "escape",
  "confirm",
  "double-confirm",
  // 時計を進める。短い方は確定を受け付けるまでの時間より短く、長い方は長い。
  "wait-short",
  "wait-long",
  "disconnect",
  "reconnect",
  "settle",
] as const;
const SHORT_WAIT_MS = 200;
const LONG_WAIT_MS = CONFIRM_DELAY_MS + 100;
type Operation = (typeof OPERATIONS)[number];

// 性質: 判断待ちのページで、作業の依頼への任意の操作の列（開く・やめる・Escape・確定・
// 二重押し・開いた直後の確定・時計を進める・切断・再接続・結果の返り）に対して、
//   - 書き込みは、確定のボタンを押した操作でだけ出る（2択のボタンを押しただけでは出ない）
//   - 1つのダイアログからは、何回確定を押しても書き込みは1回まで
//   - ダイアログが出てから CONFIRM_DELAY_MS たたない確定では、書き込みは出ない
//   - 押せる確定を、接続中に、ダイアログが出てから（開いたまま2択を押し直したなら、その後からも）
//     CONFIRM_DELAY_MS 過ぎて押せば、書き込みはちょうど1回出る
//   - 切断中は書き込みを出さない
//   - 送る report は、確定を押したときにダイアログに出ていた2択と同じ
// が成り立つ。操作の列は種から作り、除外は付けない（押せないボタンを押す操作も、
// 利用者が押せないだけで、列からは外さない）。開いたまま2択を押し直した後の待ちを数え直すかは
// 実装に任せ、その間の確定は、送っても送らなくてもよい（送ったら1回だけ）。
test("性質: 作業の依頼への任意の操作の列で、書き込みは確定の回数を超えず、開いた直後と切断中は出ない", async () => {
  const RUNS = 150;
  const MAX_LENGTH = 12;
  for (let seed = 1; seed <= RUNS; seed += 1) {
    const next = random(seed);
    const length = 1 + Math.floor(next() * MAX_LENGTH);
    const operations: Operation[] = Array.from({ length }, () => OPERATIONS[Math.floor(next() * OPERATIONS.length)]!);
    const page = await mountOperationPage();
    const confirmedDialogs = new Set<Element>();
    // テストの時計（mock.timers）。ダイアログが出た時刻と、開いたまま2択を押し直した時刻を覚える。
    let now = 0;
    let shownDialog: Element | null = null;
    let shownAt = 0;
    let repressedAt = 0;
    const observeDialog = (): void => {
      const current = page.dialog();
      if (current !== shownDialog) {
        shownDialog = current;
        shownAt = now;
        repressedAt = now;
      }
    };
    // 押し直した時刻を覚えるのは、開いたまま、押せる（送信中でも切断中でもない）2択を押したときだけ。
    // 押せない2択の押下は何も起こさないので、それで確定の期待を「どちらでもよい」に緩めない。
    const press = async (name: string): Promise<void> => {
      const wasOpen = page.dialog() !== null;
      const trigger = page.trigger(name);
      const accepted = !trigger.disabled && !page.disconnected;
      await page.click(trigger);
      if (wasOpen && accepted) {
        repressedAt = now;
      }
      observeDialog();
    };
    try {
      for (const [index, operation] of operations.entries()) {
        const where = `seed=${seed} step=${index} ops=${JSON.stringify(operations.slice(0, index + 1))}`;
        const before = page.sent.length;
        // 確定を押す操作で、送るべき数と report。"either" は、送っても送らなくてもよい（送ったら1回）。
        const expected: { sends: 0 | 1 | "either"; report: Report | undefined; dialog: Element | null } = {
          sends: 0,
          report: undefined,
          dialog: null,
        };
        const pressConfirm = async (): Promise<void> => {
          const dialog = page.dialog();
          const confirm = page.dialogButton("confirm");
          if (dialog !== null && confirm !== undefined && !confirm.disabled && !page.disconnected) {
            if (!confirmedDialogs.has(dialog) && now - shownAt >= CONFIRM_DELAY_MS) {
              expected.sends = now - repressedAt >= CONFIRM_DELAY_MS ? 1 : "either";
              expected.dialog = dialog;
              expected.report = (dialog.textContent ?? "").includes("対応できない")
                ? OPERATION_ANSWER_REPORT.UNABLE
                : OPERATION_ANSWER_REPORT.COMPLETED;
            }
          }
          await page.click(confirm);
        };
        switch (operation) {
          case "press-completed":
          case "press-unable":
          case "double-press-completed":
          case "double-press-unable": {
            const name = operation.endsWith("completed") ? "実施した" : "対応できない";
            await press(name);
            if (operation.startsWith("double")) {
              await press(name);
            }
            break;
          }
          case "press-then-confirm-completed":
          case "press-then-confirm-unable": {
            const wasOpen = page.dialog() !== null;
            await press(operation.endsWith("completed") ? "実施した" : "対応できない");
            if (!wasOpen && page.dialog() !== null) {
              assert.equal(now - shownAt, 0, `開いた直後に確定を押す ${where}`);
            }
            await pressConfirm();
            break;
          }
          case "cancel":
            await page.click(page.dialogButton("cancel"));
            break;
          case "escape":
            await page.escape();
            break;
          case "confirm":
          case "double-confirm": {
            await pressConfirm();
            if (operation === "double-confirm") {
              await page.click(page.dialogButton("confirm"));
            }
            break;
          }
          case "wait-short":
          case "wait-long": {
            const ms = operation === "wait-short" ? SHORT_WAIT_MS : LONG_WAIT_MS;
            await page.wait(ms);
            now += ms;
            break;
          }
          case "disconnect":
            await page.setDisconnected(true);
            break;
          case "reconnect":
            await page.setDisconnected(false);
            break;
          case "settle":
            await page.settle();
            break;
        }
        observeDialog();
        const added = page.sent.slice(before);
        if (expected.sends === "either") {
          assert.ok(added.length <= 1, `書き込みは1回まで ${where} sent=${JSON.stringify(added)}`);
        } else {
          assert.equal(added.length, expected.sends, `書き込みの数 ${where} sent=${JSON.stringify(added)}`);
        }
        if (added.length > 0 && expected.dialog !== null) {
          confirmedDialogs.add(expected.dialog);
        }
        for (const write of added) {
          assert.equal(write.disconnected, false, `切断中は送らない ${where}`);
          assert.equal(write.url, "/api/decisions/answer", where);
          assert.equal(write.body.report, expected.report, `ダイアログに出ていた2択を送る ${where}`);
        }
      }
      assert.ok(page.sent.length <= confirmedDialogs.size, `書き込みは確定したダイアログの数を超えない seed=${seed}`);
    } finally {
      await page.unmount();
    }
  }
});

// 判断待ちページの操作 UI の描画契約。
//
// 現在のテスト環境は DOM を持たないため、操作はサーバー描画した HTML と、
// 状態を受け取らない純粋な表示コンポーネントの props で観測する。
// クリックで確認ダイアログを開く遷移そのものは、この環境では観測できない。
import test from "node:test";
import assert from "node:assert/strict";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { PendingDecisionType, type DashboardState, type PendingDecision } from "../../src/proto/taktdash.ts";
import { createActionableDashboardState, NOW } from "../support/fixtures.ts";
import { DecisionsContent } from "../../src/components/decisions-content.tsx";
import { DecisionActionsView, decideDialogKey, decideDialogRestore } from "../../src/components/decision-actions-view.tsx";
import { DecisionResults } from "../../src/components/decision-results.tsx";
import { toDecisionResult, upsertDecisionResult } from "../../src/state/decision-actions.ts";

// 確認ダイアログの種類。null は閉じている状態を表す。
type DialogKind = "merge" | "approve-registration" | "reject-registration" | null;

const decisions = createActionableDashboardState().pendingDecisions;

const findDecision = (predicate: (decision: PendingDecision) => boolean): PendingDecision => {
  const found = decisions.find(predicate);
  assert.ok(found, "フィクスチャに対象の判断待ちがある");
  return found;
};

const merge = (): PendingDecision =>
  findDecision((decision) => decision.type === PendingDecisionType.PENDING_DECISION_TYPE_AWAITING_MERGE);
const registration = (): PendingDecision =>
  findDecision(
    (decision) =>
      decision.type === PendingDecisionType.PENDING_DECISION_TYPE_GOAL_REGISTRATION && decision.revision === 7,
  );

const stateWith = (items: PendingDecision[]): DashboardState => ({
  ...createActionableDashboardState(),
  pendingDecisions: items,
});

const renderDecisions = (state: DashboardState): string =>
  renderToStaticMarkup(createElement(DecisionsContent, { state, now: NOW }));

const noop = (): void => {};

const renderActions = (decision: PendingDecision, dialog: DialogKind, busy = false): string =>
  renderToStaticMarkup(
    createElement(DecisionActionsView, {
      decision,
      text: "",
      dialog,
      busy,
      onTextChange: noop,
      onAction: noop,
    }),
  );

test("判断待ち一覧は種別ごとの操作要素を描画する", () => {
  const html = renderDecisions(createActionableDashboardState());

  assert.match(html, /<button[^>]*>今週<\/button>/);
  assert.match(html, /<button[^>]*>来週<\/button>/);
  assert.match(html, /<input[^>]*type="text"/);
  assert.match(html, /<button[^>]*>回答する<\/button>/);
  assert.match(html, /<button[^>]*>実施した<\/button>/);
  assert.match(html, /<button[^>]*>対応できない<\/button>/);
  assert.match(html, /<button[^>]*>マージを承認<\/button>/);
  assert.match(html, /<button[^>]*>承認<\/button>/);
  assert.match(html, /<button[^>]*>却下<\/button>/);
});

test("作業の依頼には実施結果の2択だけを描画し、テキスト入力を出さない", () => {
  const html = renderDecisions(
    stateWith([findDecision((decision) => decision.inquiryId === "inquiry-operation")]),
  );

  assert.match(html, /<button[^>]*>実施した<\/button>/);
  assert.match(html, /<button[^>]*>対応できない<\/button>/);
  assert.doesNotMatch(html, /<input/);
  assert.doesNotMatch(html, /回答する/);
});

test("選択肢付きの質問には作業結果の2択を描画しない", () => {
  const html = renderDecisions(
    stateWith([findDecision((decision) => decision.inquiryId === "inquiry-options")]),
  );

  assert.match(html, /<button[^>]*>今週<\/button>/);
  assert.match(html, /<button[^>]*>来週<\/button>/);
  assert.doesNotMatch(html, /実施した/);
  assert.doesNotMatch(html, /対応できない/);
});

test("操作に必要な値が欠けた項目は操作要素を出さず、操作できない理由を表示する", () => {
  const html = renderDecisions(stateWith([findDecision((decision) => decision.inquiryId === "inquiry-without-goal")]));

  assert.doesNotMatch(html, /<button/);
  assert.doesNotMatch(html, /<input/);
  assert.match(html, /class="note"/);
  assert.doesNotMatch(html, /判断待ちはありません/);
});

test("ゴール登録の承認待ちを問い合わせと区別して表示し、脚注を3種の合計にする", () => {
  const html = renderDecisions(stateWith([registration()]));
  const list = html.slice(html.indexOf('class="tray-list"'), html.indexOf('class="foot-notes"'));

  assert.match(list, /ゴール登録の承認待ち/);
  assert.doesNotMatch(list, /問い合わせ（/);
  assert.match(list, /<button[^>]*>承認<\/button>/);
  assert.match(list, /<button[^>]*>却下<\/button>/);

  const foot = html.slice(html.indexOf('class="foot-notes"'));
  assert.match(foot, /ゴール登録の承認待ち/);
  assert.match(foot, /合計/);
});

test("マージ承認は閉じた状態では操作ボタンだけを描画し、確認時だけダイアログを描画する", () => {
  const closed = renderActions(merge(), null);
  assert.doesNotMatch(closed, /role="dialog"/);
  assert.match(closed, /<button[^>]*>マージを承認<\/button>/);

  const opened = renderActions(merge(), "merge");
  const openedDialog = opened.slice(opened.indexOf('role="dialog"'));
  assert.match(opened, /role="dialog"/);
  assert.match(opened, /aria-modal="true"/);
  assert.match(openedDialog, /418/);
  assert.match(openedDialog, /やめる/);
});

test("ゴール登録の承認と却下はそれぞれの確認ダイアログを描画する", () => {
  const approve = renderActions(registration(), "approve-registration");
  assert.match(approve, /role="dialog"/);
  assert.match(approve.slice(approve.indexOf('role="dialog"')), /承認/);

  const reject = renderActions(registration(), "reject-registration");
  assert.match(reject, /role="dialog"/);
  assert.match(reject.slice(reject.indexOf('role="dialog"')), /却下/);
});

test("確認ダイアログの見出し・本文・確定ボタン名はダイアログの種類ごとに対応する", () => {
  const dialogOf = (html: string): string => html.slice(html.indexOf('role="dialog"'));

  const approveDialog = dialogOf(renderActions(registration(), "approve-registration"));
  assert.match(approveDialog, /<h2[^>]*>ゴール登録の承認<\/h2>/);
  assert.match(approveDialog, /revision 7 のゴール登録を承認します。/);
  assert.match(approveDialog, /<button[^>]*>承認<\/button>/);
  assert.doesNotMatch(approveDialog, /却下/);

  const rejectDialog = dialogOf(renderActions(registration(), "reject-registration"));
  assert.match(rejectDialog, /<h2[^>]*>ゴール登録の却下<\/h2>/);
  assert.match(rejectDialog, /revision 7 のゴール登録を却下します。/);
  assert.match(rejectDialog, /<button[^>]*>却下<\/button>/);
  assert.doesNotMatch(rejectDialog, /承認/);

  const mergeDialog = dialogOf(renderActions(merge(), "merge"));
  assert.match(mergeDialog, /<h2[^>]*>マージの承認<\/h2>/);
  assert.match(mergeDialog, /プルリクエスト #418 のマージを承認します。/);
  assert.match(mergeDialog, /<button[^>]*>マージする<\/button>/);
  assert.doesNotMatch(mergeDialog, /revision/);
});

test("送信中は確認ダイアログのボタンを無効化する", () => {
  const html = renderActions(merge(), "merge", true);
  const dialog = html.slice(html.indexOf('role="dialog"'));
  assert.match(dialog, /disabled/);
});

test("確認ダイアログは見出しを accessible name に使い、確定ボタン名と重複しない", () => {
  const html = renderActions(merge(), "merge");
  assert.match(html, /role="dialog"[^>]*aria-labelledby="[^"]+"/);
  assert.match(html, /<h2[^>]*tabindex="-1"[^>]*>マージの承認<\/h2>/);
  assert.match(html, /<button[^>]*>マージする<\/button>/);

  const heading = html.match(/<h2[^>]*>([^<]*)<\/h2>/);
  assert.ok(heading, "ダイアログの見出しがある");
  assert.notEqual(heading[1], "マージする");
});

test("確認ダイアログの Tab はフォーカス位置に依存せず内部へ循環し、Escape は常に閉じる", () => {
  // ダイアログ外（本文・背景クリックで body に移った状態）からの Tab。
  assert.deepEqual(decideDialogKey({ key: "Tab", shiftKey: false, tabbableCount: 2, activeIndex: -1 }), {
    kind: "focus",
    target: "first",
  });
  assert.deepEqual(decideDialogKey({ key: "Tab", shiftKey: true, tabbableCount: 2, activeIndex: -1 }), {
    kind: "focus",
    target: "last",
  });

  // 端での循環。内部中央は既定の移動に任せる。
  assert.deepEqual(decideDialogKey({ key: "Tab", shiftKey: false, tabbableCount: 2, activeIndex: 1 }), {
    kind: "focus",
    target: "first",
  });
  assert.deepEqual(decideDialogKey({ key: "Tab", shiftKey: true, tabbableCount: 2, activeIndex: 0 }), {
    kind: "focus",
    target: "last",
  });
  assert.deepEqual(decideDialogKey({ key: "Tab", shiftKey: false, tabbableCount: 3, activeIndex: 1 }), {
    kind: "none",
  });
  assert.deepEqual(decideDialogKey({ key: "Tab", shiftKey: true, tabbableCount: 3, activeIndex: 1 }), {
    kind: "none",
  });

  // 送信中で操作できる要素が無いときも、背景へ抜けず見出しへ留まる。
  assert.deepEqual(decideDialogKey({ key: "Tab", shiftKey: false, tabbableCount: 0, activeIndex: -1 }), {
    kind: "focus",
    target: "heading",
  });

  // Escape はフォーカス位置に依存せず閉じる。Tab 以外のキーは関与しない。
  assert.deepEqual(decideDialogKey({ key: "Escape", shiftKey: false, tabbableCount: 2, activeIndex: -1 }), {
    kind: "cancel",
  });
  assert.deepEqual(decideDialogKey({ key: "Escape", shiftKey: false, tabbableCount: 0, activeIndex: -1 }), {
    kind: "cancel",
  });
  assert.deepEqual(decideDialogKey({ key: "Enter", shiftKey: false, tabbableCount: 2, activeIndex: 0 }), {
    kind: "none",
  });
});

test("確認ダイアログの復帰は開いたトリガーの有効性で即時復帰・待機・復帰を決める", () => {
  const enabled = {
    merge: { disabled: false },
    "approve-registration": { disabled: false },
    "reject-registration": { disabled: false },
  };

  // 表示中は復帰しない。開いたダイアログからフォーカスを奪わない。
  assert.deepEqual(
    decideDialogRestore({ dialog: "merge", previousDialog: null, pendingRestore: null, triggers: enabled }),
    { kind: "clear" },
  );

  // 非 busy で閉じたら、閉じた時点でトリガーへ即時復帰する。
  assert.deepEqual(
    decideDialogRestore({ dialog: null, previousDialog: "merge", pendingRestore: null, triggers: enabled }),
    { kind: "restore", target: "merge" },
  );

  // busy 中に閉じたら、トリガーが無効なので復帰を待つ。
  assert.deepEqual(
    decideDialogRestore({
      dialog: null,
      previousDialog: "approve-registration",
      pendingRestore: null,
      triggers: { ...enabled, "approve-registration": { disabled: true } },
    }),
    { kind: "wait", target: "approve-registration" },
  );

  // busy 解除後（直前の dialog は既に null、復帰待ちを保持）は同じトリガーへ復帰する。
  assert.deepEqual(
    decideDialogRestore({
      dialog: null,
      previousDialog: null,
      pendingRestore: "approve-registration",
      triggers: enabled,
    }),
    { kind: "restore", target: "approve-registration" },
  );

  // 戻り先が無ければ復帰しない。
  assert.deepEqual(
    decideDialogRestore({ dialog: null, previousDialog: null, pendingRestore: null, triggers: enabled }),
    { kind: "none" },
  );

  // トリガーが DOM に無い場合も復帰せず、解除後に備えて待つ。
  assert.deepEqual(
    decideDialogRestore({
      dialog: null,
      previousDialog: "reject-registration",
      pendingRestore: null,
      triggers: { ...enabled, "reject-registration": null },
    }),
    { kind: "wait", target: "reject-registration" },
  );
});

test("一覧の操作要素と入力欄は対象を識別できる name を持つ", () => {
  const mergeHtml = renderDecisions(stateWith([merge()]));
  assert.match(mergeHtml, /aria-label="マージを承認（[^"]*corporate-site[^"]*PR #418[^"]*）"/);

  const registrationHtml = renderDecisions(stateWith([registration()]));
  assert.match(registrationHtml, /aria-label="承認（[^"]*takt[^"]*revision 7[^"]*）"/);
  assert.match(registrationHtml, /aria-label="却下（[^"]*takt[^"]*revision 7[^"]*）"/);

  const optionsHtml = renderDecisions(
    stateWith([findDecision((decision) => decision.inquiryId === "inquiry-options")]),
  );
  assert.match(optionsHtml, /aria-label="今週（[^"]*inquiry-options[^"]*）"/);

  const textHtml = renderDecisions(
    stateWith([findDecision((decision) => decision.inquiryId === "inquiry-text")]),
  );
  assert.match(textHtml, /aria-label="回答（[^"]*inquiry-text[^"]*）"/);
  assert.match(textHtml, /aria-label="回答する（[^"]*inquiry-text[^"]*）"/);
});

test("操作の結果は成功を status、失敗を alert として表示する", () => {
  const html = renderToStaticMarkup(
    createElement(DecisionResults, {
      results: [
        {
          key: "inquiry-options",
          label: "esa の問い合わせ（質問 / 作業の依頼）",
          ok: true,
          message: "回答を送信しました。",
        },
        {
          key: "merge-418",
          label: "corporate-site の人のマージ待ち",
          ok: false,
          message: "マージを承認できませんでした。",
        },
      ],
    }),
  );

  assert.match(html, /role="status"/);
  assert.match(html, /role="alert"/);
  assert.match(html, /回答を送信しました。/);
  assert.match(html, /マージを承認できませんでした。/);
});

test("操作の結果が無いときは status も alert も描画しない", () => {
  const html = renderToStaticMarkup(createElement(DecisionResults, { results: [] }));
  assert.doesNotMatch(html, /role="status"/);
  assert.doesNotMatch(html, /role="alert"/);
});

test("操作結果は key 付きで追加され、同じ key は置換、別 key は別項目として表示される", () => {
  const renderResults = (results: ReturnType<typeof upsertDecisionResult>): string =>
    renderToStaticMarkup(createElement(DecisionResults, { results }));

  const success = upsertDecisionResult([], toDecisionResult(merge(), { ok: true, message: "マージを承認しました。" }));
  const successHtml = renderResults(success);
  assert.match(successHtml, /role="status"/);
  assert.doesNotMatch(successHtml, /role="alert"/);
  assert.match(successHtml, /マージを承認しました。/);

  const failure = upsertDecisionResult(success, toDecisionResult(merge(), { ok: false, message: "操作に失敗しました。" }));
  assert.equal(failure.length, 1);
  const failureHtml = renderResults(failure);
  assert.match(failureHtml, /role="alert"/);
  assert.doesNotMatch(failureHtml, /role="status"/);
  assert.match(failureHtml, /操作に失敗しました。/);

  const mixed = upsertDecisionResult(
    failure,
    toDecisionResult(registration(), { ok: true, message: "ゴール登録を承認しました。" }),
  );
  assert.equal(mixed.length, 2);
  const mixedHtml = renderResults(mixed);
  assert.match(mixedHtml, /role="status"/);
  assert.match(mixedHtml, /role="alert"/);
  assert.match(mixedHtml, /ゴール登録を承認しました。/);
});

test("本文の HTML はエスケープして描画する", () => {
  const target: PendingDecision = {
    ...findDecision((decision) => decision.inquiryId === "inquiry-text"),
    body: '<script>alert("x")</script>',
  };
  const html = renderDecisions(stateWith([target]));

  assert.match(html, /&lt;script/);
  assert.doesNotMatch(html, /<script>alert/);
});

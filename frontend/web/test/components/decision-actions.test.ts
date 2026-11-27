// 判断待ちページの操作 UI の描画契約。
//
// 現在のテスト環境は DOM を持たないため、操作はサーバー描画した HTML と、
// 状態を受け取らない純粋な表示コンポーネントの props、DOM 非依存の遷移関数で観測する。
// クリックで開く操作そのもの（マウント後のイベント）は、この環境では観測できない。
import test from "node:test";
import assert from "node:assert/strict";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { JSDOM } from "jsdom";
import { PendingDecisionType, type DashboardState, type PendingDecision } from "../../src/proto/taktdash.ts";
import { createActionableDashboardState, NOW } from "../support/fixtures.ts";
import { DecisionsContent } from "../../src/components/decisions-content.tsx";
import {
  DecisionActionsView,
  acceptsDecisionIntent,
  decideDecisionTransition,
  decideDialogKey,
  decideDialogRestore,
} from "../../src/components/decision-actions-view.tsx";
import { DecisionResults } from "../../src/components/decision-results.tsx";
import {
  INITIAL_DECISION_DIALOG_STATE,
  openDecisionDialog,
  pullRequestLinkUrl,
  toDecisionResult,
  upsertDecisionResult,
  type DecisionAction,
  type DecisionDialogKind,
  type DecisionDialogState,
} from "../../src/state/decision-actions.ts";

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
  renderToStaticMarkup(createElement(DecisionsContent, { snapshotLoaded: true, state, now: NOW, disconnected: false }));

const noop = (): void => {};

const renderActions = (
  decision: PendingDecision,
  dialog: DialogKind,
  busy = false,
  dialogTarget: ReturnType<typeof openDecisionDialog> | null = null,
): string =>
  renderToStaticMarkup(
    createElement(DecisionActionsView, {
      decision,
      text: "",
      dialog,
      busy,
      disconnected: false,
      dialogTarget,
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

test("判断待ち一覧はまだ読めていないサーバーがあるとき0件でも「判断待ちはありません」と断定しない", () => {
  const base = createActionableDashboardState();
  const html = renderDecisions({
    ...base,
    pendingDecisions: [],
    servers: base.servers.map((server) => ({ ...server, connected: false, lastReadAt: undefined })),
  });
  assert.doesNotMatch(html, /判断待ちはありません/);
  assert.match(html, /まだ読めていないサーバー・プロジェクトがあります/);
  assert.match(html, /判断待ちがあるかどうかは分かりません/);
});

test("判断待ち一覧はサーバーが既読でも未読のプロジェクトがあるとき0件でも「判断待ちはありません」と断定しない", () => {
  const base = createActionableDashboardState();
  const html = renderDecisions({
    ...base,
    pendingDecisions: [],
    projects: base.projects.map((project, index) =>
      index === 0 ? { ...project, lastReadAt: undefined } : project,
    ),
  });
  assert.doesNotMatch(html, /判断待ちはありません/);
  assert.match(html, /まだ読めていないサーバー・プロジェクトがあります/);
  assert.match(html, /判断待ちがあるかどうかは分かりません/);
});

test("判断待ち一覧はまだ読めていないサーバーがあっても読めた分の一覧はそのまま描画する", () => {
  const base = createActionableDashboardState();
  const html = renderDecisions({
    ...base,
    servers: base.servers.map((server) => ({ ...server, connected: false, lastReadAt: undefined })),
  });
  assert.match(html, /まだ読めていないサーバー・プロジェクトがあります/);
  assert.match(html, /リリースの日取りはどうしますか/);
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

test("マージ確認は開いた後に PR が変わっても、開いた時点の PR を表示する", () => {
  const dialogTarget = openDecisionDialog(merge(), "merge");
  const changed: PendingDecision = {
    ...merge(),
    pullRequest: { number: 419, url: "https://example.test/pr/419" },
  };

  const html = renderActions(changed, "merge", false, dialogTarget);
  const dialog = html.slice(html.indexOf('role="dialog"'));
  assert.match(dialog, /418/);
  assert.doesNotMatch(dialog, /419/);
});

test("ゴール登録の確認は開いた後に revision が変わっても、開いた時点の revision を表示する", () => {
  const dialogTarget = openDecisionDialog(registration(), "approve-registration");
  const changed: PendingDecision = { ...registration(), revision: 8 };

  const html = renderActions(changed, "approve-registration", false, dialogTarget);
  const dialog = html.slice(html.indexOf('role="dialog"'));
  assert.match(dialog, /revision 7/);
  assert.doesNotMatch(dialog, /revision 8/);
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

test("確認ダイアログの Tab はフォーカス位置に依存せず内部へ循環し、送信中でなければ Escape は閉じる", () => {
  // ダイアログ外（本文・背景クリックで body に移った状態）からの Tab。
  assert.deepEqual(decideDialogKey({ busy: false, key: "Tab", shiftKey: false, tabbableCount: 2, activeIndex: -1 }), {
    kind: "focus",
    target: "first",
  });
  assert.deepEqual(decideDialogKey({ busy: false, key: "Tab", shiftKey: true, tabbableCount: 2, activeIndex: -1 }), {
    kind: "focus",
    target: "last",
  });

  // 端での循環。内部中央は既定の移動に任せる。
  assert.deepEqual(decideDialogKey({ busy: false, key: "Tab", shiftKey: false, tabbableCount: 2, activeIndex: 1 }), {
    kind: "focus",
    target: "first",
  });
  assert.deepEqual(decideDialogKey({ busy: false, key: "Tab", shiftKey: true, tabbableCount: 2, activeIndex: 0 }), {
    kind: "focus",
    target: "last",
  });
  assert.deepEqual(decideDialogKey({ busy: false, key: "Tab", shiftKey: false, tabbableCount: 3, activeIndex: 1 }), {
    kind: "none",
  });
  assert.deepEqual(decideDialogKey({ busy: false, key: "Tab", shiftKey: true, tabbableCount: 3, activeIndex: 1 }), {
    kind: "none",
  });

  // 送信中で操作できる要素が無いときも、背景へ抜けず見出しへ留まる。
  assert.deepEqual(decideDialogKey({ busy: false, key: "Tab", shiftKey: false, tabbableCount: 0, activeIndex: -1 }), {
    kind: "focus",
    target: "heading",
  });

  // 送信中でなければ、Escape はフォーカス位置に依存せず閉じる。Tab 以外のキーは関与しない。
  assert.deepEqual(decideDialogKey({ busy: false, key: "Escape", shiftKey: false, tabbableCount: 2, activeIndex: -1 }), {
    kind: "cancel",
  });
  assert.deepEqual(decideDialogKey({ busy: false, key: "Escape", shiftKey: false, tabbableCount: 0, activeIndex: -1 }), {
    kind: "cancel",
  });
  assert.deepEqual(decideDialogKey({ busy: false, key: "Enter", shiftKey: false, tabbableCount: 2, activeIndex: 0 }), {
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
    decideDialogRestore({ focusElsewhere: false, dialog: "merge", previousDialog: null, pendingRestore: null, triggers: enabled }),
    { kind: "clear" },
  );

  // 非 busy で閉じたら、閉じた時点でトリガーへ即時復帰する。
  assert.deepEqual(
    decideDialogRestore({ focusElsewhere: false, dialog: null, previousDialog: "merge", pendingRestore: null, triggers: enabled }),
    { kind: "restore", target: "merge" },
  );

  // busy 中に閉じたら、トリガーが無効なので復帰を待つ。
  assert.deepEqual(
    decideDialogRestore({ focusElsewhere: false,
      dialog: null,
      previousDialog: "approve-registration",
      pendingRestore: null,
      triggers: { ...enabled, "approve-registration": { disabled: true } },
    }),
    { kind: "wait", target: "approve-registration" },
  );

  // busy 解除後（直前の dialog は既に null、復帰待ちを保持）は同じトリガーへ復帰する。
  assert.deepEqual(
    decideDialogRestore({ focusElsewhere: false,
      dialog: null,
      previousDialog: null,
      pendingRestore: "approve-registration",
      triggers: enabled,
    }),
    { kind: "restore", target: "approve-registration" },
  );

  // 戻り先が無ければ復帰しない。
  assert.deepEqual(
    decideDialogRestore({ focusElsewhere: false, dialog: null, previousDialog: null, pendingRestore: null, triggers: enabled }),
    { kind: "none" },
  );

  // トリガーが DOM に無い場合も復帰せず、解除後に備えて待つ。
  assert.deepEqual(
    decideDialogRestore({ focusElsewhere: false,
      dialog: null,
      previousDialog: "reject-registration",
      pendingRestore: null,
      triggers: { ...enabled, "reject-registration": null },
    }),
    { kind: "wait", target: "reject-registration" },
  );
});

const openTransitionState = (decision: PendingDecision, dialog: DecisionDialogKind): DecisionDialogState => {
  const transition = decideDecisionTransition({
    decision,
    intent: { type: "open", dialog },
    text: "",
    state: INITIAL_DECISION_DIALOG_STATE,
  });
  assert.equal(transition.kind, "state");
  if (transition.kind !== "state") {
    throw new Error("開く操作はダイアログ状態を返す");
  }
  return transition.state;
};

const confirmTransition = (
  decision: PendingDecision,
  dialog: DecisionDialogKind,
  state: DecisionDialogState,
) =>
  decideDecisionTransition({
    decision,
    intent: { type: "confirm", dialog },
    text: "",
    state,
  });

const sentRevision = (action: DecisionAction | undefined): number | undefined =>
  action?.kind === "approve-registration" || action?.kind === "reject-registration"
    ? action.payload.revision
    : undefined;

test("マージ確認の遷移は開いた時点の PR を保持し、PR が変わった確認は送信しない", () => {
  const opened = openTransitionState(merge(), "merge");
  assert.deepEqual(opened, {
    kind: "open",
    dialog: "merge",
    target: { kind: "merge", prNumber: 418, url: "https://example.test/pr/418" },
  });

  // 同じ対象のまま confirm すれば、開いた時点の PR で送信する。
  const submitted = confirmTransition(merge(), "merge", opened);
  assert.equal(submitted.kind, "submit");
  if (submitted.kind !== "submit") {
    throw new Error("同じ対象の確認は送信する");
  }
  assert.equal(submitted.action?.kind, "merge");
  if (submitted.action?.kind !== "merge") {
    throw new Error("マージの送信内容を返す");
  }
  assert.equal(submitted.action.payload.prNumber, 418);

  // SSE 更新で PR 419 に変わった後は、開いた時点の 418 で送信せず stale の結果を返す。
  const changed: PendingDecision = {
    ...merge(),
    pullRequest: { number: 419, url: "https://example.test/pr/419" },
  };
  const stale = confirmTransition(changed, "merge", opened);
  assert.equal(stale.kind, "result");
  if (stale.kind !== "result") {
    throw new Error("対象が変わった確認は送信せず結果を返す");
  }
  assert.equal(stale.result.ok, false);
  assert.notEqual(stale.result.message, "");
  assert.deepEqual(stale.state, INITIAL_DECISION_DIALOG_STATE);
});

test("ゴール登録の遷移は開いた時点の revision を保持し、revision が変わった確認は送信しない", () => {
  const cases: { dialog: DecisionDialogKind; kind: DecisionAction["kind"] }[] = [
    { dialog: "approve-registration", kind: "approve-registration" },
    { dialog: "reject-registration", kind: "reject-registration" },
  ];
  for (const { dialog, kind } of cases) {
    const opened = openTransitionState(registration(), dialog);
    assert.deepEqual(opened, { kind: "open", dialog, target: { kind: dialog, revision: 7 } });

    const submitted = confirmTransition(registration(), dialog, opened);
    assert.equal(submitted.kind, "submit");
    if (submitted.kind !== "submit") {
      throw new Error(`${dialog} の確認は送信する`);
    }
    assert.equal(submitted.action?.kind, kind);
    assert.equal(sentRevision(submitted.action), 7);

    const changed: PendingDecision = { ...registration(), revision: 8 };
    const stale = confirmTransition(changed, dialog, opened);
    assert.equal(stale.kind, "result");
    if (stale.kind !== "result") {
      throw new Error(`${dialog} の revision 変更後の確認は送信しない`);
    }
    assert.equal(stale.result.ok, false);
    assert.deepEqual(stale.state, INITIAL_DECISION_DIALOG_STATE);
  }
});

test("対象が欠けたマージ確認は送信せず失敗の結果を返す", () => {
  const missing: PendingDecision = { ...merge(), pullRequest: undefined };
  const opened = openTransitionState(missing, "merge");
  assert.deepEqual(opened, { kind: "open", dialog: "merge", target: null });

  const stale = confirmTransition(missing, "merge", opened);
  assert.equal(stale.kind, "result");
  if (stale.kind !== "result") {
    throw new Error("対象が欠けた確認は送信せず結果を返す");
  }
  assert.equal(stale.result.ok, false);
  assert.deepEqual(stale.state, INITIAL_DECISION_DIALOG_STATE);
});

test("ダイアログの中断は閉じ、回答は対象の有無に関わらず送信する", () => {
  const opened = openTransitionState(merge(), "merge");
  const cancelled = decideDecisionTransition({
    decision: merge(),
    intent: { type: "cancel" },
    text: "",
    state: opened,
  });
  assert.deepEqual(cancelled, { kind: "state", state: INITIAL_DECISION_DIALOG_STATE });

  const options = findDecision((decision) => decision.inquiryId === "inquiry-options");
  const answered = decideDecisionTransition({
    decision: options,
    intent: { type: "answer-option", option: "今週" },
    text: "",
    state: INITIAL_DECISION_DIALOG_STATE,
  });
  assert.equal(answered.kind, "submit");
  if (answered.kind !== "submit") {
    throw new Error("回答は対象が無くても送信する");
  }
  assert.equal(answered.action?.kind, "answer");
  if (answered.action?.kind !== "answer") {
    throw new Error("回答の送信内容を返す");
  }
  assert.equal(answered.action.payload.text, "今週");
});

test("選択肢の回答は先頭以外を選んでも、選んだ選択肢をそのまま送信する", () => {
  const options = findDecision((decision) => decision.inquiryId === "inquiry-options");
  assert.deepEqual(options.options, ["今週", "来週"], "先頭と末尾の異なる選択肢を持つ");

  const answered = decideDecisionTransition({
    decision: options,
    intent: { type: "answer-option", option: "来週" },
    text: "",
    state: INITIAL_DECISION_DIALOG_STATE,
  });
  assert.equal(answered.kind, "submit");
  if (answered.kind !== "submit") {
    throw new Error("回答は送信する");
  }
  assert.equal(answered.action?.kind, "answer");
  if (answered.action?.kind !== "answer") {
    throw new Error("回答の送信内容を返す");
  }
  assert.equal(answered.action.payload.text, "来週", "選んだ選択肢をそのまま送信する");
});

// ボタンの aria-describedby が指す識別子を解決し、要素の本文を返す。要素が無ければ undefined。
// 読み上げ名は操作の文言だけにし、対象の識別子は別要素から補う新契約を、描画結果から観測する。
const descriptionTexts = (html: string): Map<string, string> => {
  const byId = new Map<string, string>();
  for (const match of html.matchAll(/<[^>]*\bid="([^"]+)"[^>]*>([\s\S]*?)<\/[^>]+>/g)) {
    byId.set(match[1]!, match[2]!);
  }
  return byId;
};

// 指定した操作文言のボタンを1件取り出し、その aria-label と、aria-describedby が指す説明文を返す。
const describedButton = (html: string, actionName: string) => {
  const descriptions = descriptionTexts(html);
  const button = new RegExp(`<button\\b[^>]*\\baria-label="${actionName}"[^>]*>([\\s\\S]*?)</button>`).exec(html);
  if (button === null) {
    throw new Error(`操作ボタン「${actionName}」が描画されていない`);
  }
  const describedBy = /\baria-describedby="([^"]+)"/.exec(button[0])?.[1];
  if (describedBy === undefined) {
    throw new Error(`操作ボタン「${actionName}」は aria-describedby を持たない`);
  }
  const text = describedBy
    .split(/\s+/)
    .map((id) => descriptions.get(id) ?? "")
    .join(" ");
  return { label: actionName, description: text };
};

test("一覧の操作ボタンは操作の文言だけを読み上げ名にし、対象の識別子は説明から補う", () => {
  const mergeButton = describedButton(renderDecisions(stateWith([merge()])), "マージを承認");
  assert.equal(mergeButton.label, "マージを承認");
  assert.match(mergeButton.description, /corporate-site/);
  assert.match(mergeButton.description, /PR #418/);

  const registrationHtml = renderDecisions(stateWith([registration()]));
  const approveButton = describedButton(registrationHtml, "承認");
  assert.match(approveButton.description, /takt/);
  assert.match(approveButton.description, /revision 7/);
  const rejectButton = describedButton(registrationHtml, "却下");
  assert.match(rejectButton.description, /takt/);
  assert.match(rejectButton.description, /revision 7/);

  const optionsHtml = renderDecisions(
    stateWith([findDecision((decision) => decision.inquiryId === "inquiry-options")]),
  );
  const optionButton = describedButton(optionsHtml, "今週");
  assert.match(optionButton.description, /inquiry-options/);

  const operationHtml = renderDecisions(
    stateWith([findDecision((decision) => decision.inquiryId === "inquiry-operation")]),
  );
  const completedButton = describedButton(operationHtml, "実施した");
  assert.match(completedButton.description, /inquiry-operation/);

  // 削除された自由記述送信の対象識別ケース。入力欄ではなく「回答する」ボタンについて、
  // 名前が操作文言のみであることと、説明参照先が当該の問い合わせを示すことを確認する。
  const textHtml = renderDecisions(
    stateWith([findDecision((decision) => decision.inquiryId === "inquiry-text")]),
  );
  const submitButton = describedButton(textHtml, "回答する");
  assert.equal(submitButton.label, "回答する");
  assert.match(submitButton.description, /inquiry-text/);
});

test("操作ボタンの読み上げ名は対象の識別子を埋め込まず、複数の対象が並んでも説明で区別できる", () => {
  const html = renderDecisions(createActionableDashboardState());

  // 同じ「承認」「却下」が複数並んでも、aria-label は操作の文言のまま。
  const approveNames = [...html.matchAll(/<button\b[^>]*\baria-label="承認"/g)];
  assert.equal(approveNames.length, 1, "操作文言そのものを name にし、対象の括弧書きを name に含めない");

  // 操作ボタンの aria-label に対象の括弧書きを残さない。入力欄は対象付きラベルを保つ別契約なので対象外。
  const buttonLabels = [...html.matchAll(/<button\b[^>]*\baria-label="([^"]*)"/g)].map((match) => match[1] ?? "");
  assert.ok(buttonLabels.length > 0, "操作ボタンが読み上げ名を持つ");
  for (const label of buttonLabels) {
    assert.doesNotMatch(label, /（/, `操作ボタンの読み上げ名「${label}」は対象の括弧書きを含まない`);
  }
});

test("自由記述の入力欄は対象を識別できる名前を保つ", () => {
  const html = renderDecisions(
    stateWith([findDecision((decision) => decision.inquiryId === "inquiry-text")]),
  );
  const input = /<input[^>]*\baria-label="([^"]+)"/.exec(html)?.[1];
  assert.ok(input !== undefined, "自由記述の入力欄が描画されている");
  assert.match(input, /inquiry-text/, "入力欄の読み上げ名は対象の識別子を含む");
});

test("同じ inquiryId でもゴールが違う問い合わせは別の項目として描画し、各項目の操作ボタンが自分の対象を参照する", () => {
  const base = findDecision((decision) => decision.inquiryId === "inquiry-options");
  const first: PendingDecision = { ...base, goalId: "goal-2" };
  const second: PendingDecision = { ...base, goalId: "goal-3" };

  const document = new JSDOM(renderDecisions(stateWith([first, second]))).window.document;

  const rows = Array.from(document.querySelectorAll(".tray-item"));
  assert.equal(rows.length, 2, "2件の判断待ち項目を描画する");

  // 項目ごとに、その行の対象（ゴール）と「今週」ボタンの説明参照先を解決し、
  // ボタンが同じ行の対象だけを説明していることを確認する。集合の一致では参照の交換を検出できない。
  const resolvedGoals = new Map<string, string>();
  for (const row of rows) {
    const rowTarget = row.querySelector(".target")?.textContent ?? "";
    const goal = /ゴール (goal-\d)/.exec(rowTarget)?.[1];
    assert.ok(goal !== undefined, `行に対象のゴールが表示されている（対象: ${rowTarget}）`);

    const button = row.querySelector('button[aria-label="今週"]');
    assert.ok(button !== null, "行の「今週」ボタンが描画されている");
    assert.equal(button.getAttribute("aria-label"), "今週", "読み上げ名は操作文言のみ");

    const describedBy = button.getAttribute("aria-describedby");
    assert.ok(describedBy !== null, "「今週」ボタンは説明参照を持つ");
    const descriptionIds = describedBy.split(/\s+/);
    const description = descriptionIds
      .map((id) => {
        const element = document.getElementById(id);
        assert.ok(element !== null, `説明参照先 ${id} が文書内に存在する`);
        return element.textContent ?? "";
      })
      .join(" ");

    assert.match(description, new RegExp(goal), `行の対象 ${goal} を説明に含む`);
    assert.match(description, /inquiry-options/, "説明は同じ問い合わせを指す");
    resolvedGoals.set(goal, description);
  }

  // 2件の説明参照先は別要素で、それぞれ別の対象説明へ到達する（交換・衝突を検出する）。
  assert.equal(new Set(resolvedGoals.keys()).size, 2, "2行の対象が区別できる");
  assert.notEqual(resolvedGoals.get("goal-2"), resolvedGoals.get("goal-3"), "2行の説明は別の対象を示す");
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

// issue #304: 複数のプロジェクトの判断待ちが並んでも、確認ダイアログだけで対象を確かめられる。
test("マージの確認ダイアログはプロジェクト名と、新しいタブで開くプルリクエストへのリンクを表示する", () => {
  const decision = merge();
  const html = renderActions(decision, "merge", false, openDecisionDialog(decision, "merge"));
  const dialog = new JSDOM(html).window.document.querySelector('[role="dialog"]');
  assert.ok(dialog, "確認ダイアログがある");

  const rows = Array.from(dialog.querySelectorAll("dl > div")).map((row) => [
    row.querySelector("dt")?.textContent,
    row.querySelector("dd")?.textContent,
  ]);
  assert.deepEqual(rows, [
    ["プロジェクト", decision.project],
    ["プルリクエスト", "#418 を開く（新しいタブ）"],
  ]);

  const links = Array.from(dialog.querySelectorAll("a"));
  assert.equal(links.length, 1);
  assert.equal(links[0]?.getAttribute("href"), "https://example.test/pr/418");
  assert.equal(links[0]?.getAttribute("target"), "_blank");
  assert.equal(links[0]?.getAttribute("rel"), "noopener noreferrer");
});

test("マージの確認ダイアログは開いた後に PR が変わっても、開いた時点の PR へのリンクを保つ", () => {
  const dialogTarget = openDecisionDialog(merge(), "merge");
  const changed: PendingDecision = {
    ...merge(),
    pullRequest: { number: 419, url: "https://example.test/pr/419" },
  };
  const html = renderActions(changed, "merge", false, dialogTarget);
  const link = new JSDOM(html).window.document.querySelector('[role="dialog"] a');
  assert.equal(link?.getAttribute("href"), "https://example.test/pr/418");
});

test("プルリクエストを開ける URL が無いマージ待ちは、マージの承認を出さず理由を表示する", () => {
  const unopenable = [
    "",
    "javascript:alert(1)",
    "/pr/418",
    "ftp://example.test/pr/418",
    // // を欠く形は、http の画面では画面の中の相対パスとして開く。
    "http:git.example/org/repo/pull/418",
    "https:git.example/org/repo/pull/418",
    "http:/git.example/org/repo/pull/418",
    "http://",
    " https://example.test/pr/418",
  ];
  for (const url of unopenable) {
    const decision: PendingDecision = { ...merge(), pullRequest: { number: 418, url } };
    const html = renderActions(decision, null);
    assert.doesNotMatch(html, /<button/, `URL が ${JSON.stringify(url)} のとき操作ボタンを出さない`);
    assert.match(html, /プルリクエストの URL が無いため、マージを承認できません。/);
    assert.equal(openDecisionDialog(decision, "merge"), undefined);

    const list = renderDecisions(stateWith([decision]));
    assert.doesNotMatch(list, /<a [^>]*href=/, "開けない URL は一覧でもリンクにしない");
    assert.match(list, /対象: プルリクエスト #418/);
  }
});

test("判断待ち一覧のマージ待ちは、対象を新しいタブで開くプルリクエストへのリンクにする", () => {
  const html = renderDecisions(stateWith([merge()]));
  const link = new JSDOM(html).window.document.querySelector(".tray-item .target a");
  assert.ok(link, "対象のリンクがある");
  assert.equal(link.textContent, "プルリクエスト #418");
  assert.equal(link.getAttribute("href"), "https://example.test/pr/418");
  assert.equal(link.getAttribute("target"), "_blank");
  assert.equal(link.getAttribute("rel"), "noopener noreferrer");
});

test("ゴール登録の確認ダイアログは対象の一覧もリンクも出さない", () => {
  const dialog = new JSDOM(renderActions(registration(), "approve-registration")).window.document.querySelector(
    '[role="dialog"]',
  );
  assert.ok(dialog, "確認ダイアログがある");
  assert.equal(dialog.querySelector("dl"), null);
  assert.equal(dialog.querySelector("a"), null);
});

test("プルリクエストの URL の判定は、古いブラウザに無い URL.canParse を使わない", () => {
  const original = Object.getOwnPropertyDescriptor(URL, "canParse");
  assert.ok(original, "この実行環境には URL.canParse がある");
  Object.defineProperty(URL, "canParse", { value: undefined, configurable: true, writable: true });
  try {
    assert.equal(pullRequestLinkUrl("https://example.test/pr/418"), "https://example.test/pr/418");
    assert.equal(pullRequestLinkUrl("HTTP://example.test/pr/418"), "HTTP://example.test/pr/418");
    assert.equal(pullRequestLinkUrl("not a url"), undefined);
    assert.match(renderDecisions(stateWith([merge()])), /マージを承認/);
  } finally {
    Object.defineProperty(URL, "canParse", original);
  }
});

// 切断中は、最後に読めた判断待ちを残したまま、回答の操作だけを止める（issue #307）。
const OFFLINE_NOTE = "再接続するまで操作できません。";

const renderDecisionsConnection = (disconnected: boolean): string =>
  renderToStaticMarkup(
    createElement(DecisionsContent, {
      snapshotLoaded: true,
      state: createActionableDashboardState(),
      now: NOW,
      disconnected,
    }),
  );

const controlsOf = (html: string): string[] => {
  const document = new JSDOM(html).window.document;
  return Array.from(document.querySelectorAll(".tray-list button, .tray-list input")).map(
    (element) => element.outerHTML,
  );
};

test("切断中は判断待ちの回答ボタンと入力欄をすべて無効にし、再接続するまで操作できないと添える", () => {
  const html = renderDecisionsConnection(true);
  const controls = controlsOf(html);

  assert.ok(controls.length > 0, "フィクスチャに操作要素がある");
  for (const control of controls) {
    assert.match(control, /\sdisabled=""/, `切断中は押せない: ${control}`);
  }
  const document = new JSDOM(html).window.document;
  const items = Array.from(document.querySelectorAll(".tray-list > li")).filter(
    (item) => item.querySelector("button, input") !== null,
  );
  assert.ok(items.length > 0);
  for (const item of items) {
    assert.ok(item.textContent?.includes(OFFLINE_NOTE), "操作のある項目ごとに理由を添える");
  }
});

test("接続中は判断待ちの回答ボタンを押せ、操作できない旨を出さない", () => {
  const html = renderDecisionsConnection(false);
  const buttons = controlsOf(html).filter(
    (control) => control.startsWith("<button") && !control.includes('type="submit"'),
  );

  assert.ok(buttons.length > 0);
  for (const button of buttons) {
    assert.doesNotMatch(button, /\sdisabled=""/, `接続中は押せる: ${button}`);
  }
  assert.ok(!html.includes(OFFLINE_NOTE));
});

test("切断中は開いていたマージの確認でも確定できず、やめるだけは押せる", () => {
  const html = renderToStaticMarkup(
    createElement(DecisionActionsView, {
      decision: merge(),
      text: "",
      dialog: "merge",
      busy: false,
      disconnected: true,
      dialogTarget: null,
      onTextChange: noop,
      onAction: noop,
    }),
  );
  const document = new JSDOM(html).window.document;
  const dialogButtons = Array.from(document.querySelectorAll('[role="dialog"] button'));
  const cancel = dialogButtons.find((button) => button.textContent === "やめる");
  const confirm = dialogButtons.find((button) => button.textContent === "マージする");
  assert.ok(cancel && confirm);
  assert.equal(cancel.hasAttribute("disabled"), false);
  assert.equal(confirm.hasAttribute("disabled"), true);
  assert.ok(document.querySelector('[role="dialog"]')?.textContent?.includes(OFFLINE_NOTE));
});

// 性質: 判断待ちのすべての種別、送信中・切断・確認ダイアログ・入力のすべての組み合わせで、
// 送信につながる操作要素は「送信中でも切断中でもない」ときだけ押せ、「やめる」は送信中だけ
// 押せず、理由の文は切断中で操作のある項目にだけ出る（issue #307）。除外は付けない。
test("性質: 判断待ちの操作要素は、送信中か切断中なら押せず、やめるは送信中だけ押せない", () => {
  const dialogsFor = (decision: PendingDecision): DialogKind[] => {
    if (decision.type === PendingDecisionType.PENDING_DECISION_TYPE_AWAITING_MERGE) {
      return [null, "merge"];
    }
    if (decision.type === PendingDecisionType.PENDING_DECISION_TYPE_GOAL_REGISTRATION) {
      return [null, "approve-registration", "reject-registration"];
    }
    return [null];
  };
  let checked = 0;
  for (const decision of decisions) {
    for (const dialog of dialogsFor(decision)) {
      for (const busy of [false, true]) {
        for (const disconnected of [false, true]) {
          for (const text of ["", " ", "\u3000", " 回答 ", "回答"]) {
            const html = renderToStaticMarkup(
              createElement(DecisionActionsView, {
                decision,
                text,
                dialog,
                busy,
                disconnected,
                dialogTarget: dialog === null ? null : (openDecisionDialog(decision, dialog) ?? null),
                onTextChange: noop,
                onAction: noop,
              }),
            );
            const document = new JSDOM(html).window.document;
            const controls = Array.from(document.querySelectorAll("button, input"));
            const where = JSON.stringify({ type: decision.type, dialog, busy, disconnected, text });
            for (const control of controls) {
              const disabled = control.hasAttribute("disabled");
              if (control.textContent === "やめる") {
                assert.equal(disabled, busy, `やめる ${where}`);
              } else if (control.getAttribute("type") === "submit") {
                assert.equal(disabled, busy || disconnected || text.trim() === "", `回答する ${where}`);
              } else {
                assert.equal(disabled, busy || disconnected, `${control.outerHTML} ${where}`);
              }
              checked += 1;
            }
            const notes = html.split("再接続するまで操作できません。").length - 1;
            const expected = controls.length === 0 || !disconnected ? 0 : dialog === null ? 1 : 2;
            assert.equal(notes, expected, `理由の文 ${where}`);
          }
        }
      }
    }
  }
  assert.ok(checked > 0);
});

// 性質: 操作要素の名前の集合は、送信中・切断に依らない。切断の前後で同じ操作が並び、
// 再接続すれば接続中と同じ操作ができる（不変条件6、issue #307）。除外は付けない。
test("性質: 判断待ちの操作要素の名前の集合は、送信中と切断に依らない", () => {
  const dialogs: DialogKind[] = [null, "merge", "approve-registration", "reject-registration"];
  const namesOf = (decision: PendingDecision, dialog: DialogKind, busy: boolean, disconnected: boolean): string[] => {
    const html = renderToStaticMarkup(
      createElement(DecisionActionsView, {
        decision,
        text: "回答",
        dialog,
        busy,
        disconnected,
        dialogTarget: dialog === null ? null : (openDecisionDialog(decision, dialog) ?? null),
        onTextChange: noop,
        onAction: noop,
      }),
    );
    const document = new JSDOM(html).window.document;
    return Array.from(document.querySelectorAll("button, input, a[href]"))
      .map((element) => `${element.tagName}:${element.getAttribute("aria-label") ?? element.textContent ?? ""}`)
      .sort();
  };
  for (const decision of decisions) {
    for (const dialog of dialogs) {
      const connected = namesOf(decision, dialog, false, false);
      for (const busy of [false, true]) {
        for (const disconnected of [false, true]) {
          assert.deepEqual(
            namesOf(decision, dialog, busy, disconnected),
            connected,
            JSON.stringify({ type: decision.type, dialog, busy, disconnected }),
          );
        }
      }
    }
  }
});

test("切断中は、開く・確定・回答の操作を入口で受け付けず、やめるだけを受け付ける", () => {
  const intents = [
    { type: "answer-text" },
    { type: "answer-option", option: "今週" },
    { type: "open", dialog: "merge" },
    { type: "confirm", dialog: "merge" },
    { type: "cancel" },
  ] as const;
  for (const intent of intents) {
    assert.equal(acceptsDecisionIntent(intent, false), true, `接続中 ${intent.type}`);
    assert.equal(acceptsDecisionIntent(intent, true), intent.type === "cancel", `切断中 ${intent.type}`);
  }
});

test("確認ダイアログの Escape は、送信中には取り消さない", () => {
  assert.deepEqual(decideDialogKey({ busy: true, key: "Escape", shiftKey: false, tabbableCount: 0, activeIndex: -1 }), {
    kind: "none",
  });
  assert.deepEqual(decideDialogKey({ busy: false, key: "Escape", shiftKey: false, tabbableCount: 2, activeIndex: -1 }), {
    kind: "cancel",
  });
});

test("確認ダイアログの復帰を待つ間に別の要素へフォーカスが移っていたら、上書きせず待ちをやめる", () => {
  const enabled = {
    merge: { disabled: false },
    "approve-registration": { disabled: false },
    "reject-registration": { disabled: false },
  };
  assert.deepEqual(
    decideDialogRestore({ focusElsewhere: true, dialog: null, previousDialog: null, pendingRestore: "merge", triggers: enabled }),
    { kind: "drop" },
  );
  // トリガーがまだ押せない間は、よそにフォーカスがあっても待ち続ける。
  assert.deepEqual(
    decideDialogRestore({
      focusElsewhere: true,
      dialog: null,
      previousDialog: null,
      pendingRestore: "merge",
      triggers: { ...enabled, merge: { disabled: true } },
    }),
    { kind: "wait", target: "merge" },
  );
});

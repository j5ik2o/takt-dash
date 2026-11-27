// issue #315 の契約：画面の文言とラベルを、操作の内容と日本語の表示に合わせる。
//
// 設計は docs/design/issue-315.md。指摘 D4〜D7、D13〜D19、D21、D22 の「テストで固定すること」を、
// サーバー描画した HTML を jsdom で読んで観測する。D7 の「閉じる」は
// test/components/issue-315-decision-results.test.ts、D20 は test/components/issue-315-activity-days.test.ts、
// 狭い幅の見た目は e2e/tests/issue-315-narrow.spec.ts が固定する。
import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { register } from "node:module";
import { fileURLToPath } from "node:url";
import { createElement, type ComponentType } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { JSDOM } from "jsdom";
import * as grpc from "@grpc/grpc-js";
import { PendingDecisionType, type DashboardState, type PendingDecision } from "../../src/proto/taktdash.ts";
import {
  NOW,
  createActionableDashboardState,
  createActivityResponse,
  createDashboardState,
  createGoalDetailResponse,
  createRunDetailResponse,
  createTaskDetailResponse,
} from "../support/fixtures.ts";
import { startTaktDashServer } from "../support/grpc-server.ts";
import { contrastRatio } from "../support/contrast.ts";
import { closeBackendClient } from "../../src/grpc/client.ts";
import { DecisionsContent } from "../../src/components/decisions-content.tsx";
import { DecisionActionsView } from "../../src/components/decision-actions-view.tsx";
import { DecisionResults } from "../../src/components/decision-results.tsx";
import { RunDetailContent } from "../../src/components/run-detail-content.tsx";
import { RunDetailLoader } from "../../src/components/run-detail-loader.tsx";
import { GoalDetailContent } from "../../src/components/goal-detail-content.tsx";
import { TaskDetailContent } from "../../src/components/task-detail-content.tsx";
import { ActivityContent } from "../../src/components/activity-content.tsx";
import OverviewPage from "../../app/page.tsx";
import { buildOverviewView } from "../../src/state/dashboard.ts";
import { metadata } from "../../app/layout.tsx";
import {
  buildMergeAction,
  submitDecisionAction,
  toDecisionResult,
  type DecisionIntent,
  type DecisionResult,
} from "../../src/state/decision-actions.ts";
import { formatClock } from "../../src/state/format.ts";

const documentOf = (html: string): Document => new JSDOM(html).window.document;
const normalize = (text: string | null | undefined): string => (text ?? "").replace(/\s+/g, " ").trim();
const noop = (): void => {};
// 操作の意図を受けて受理する。DecisionActionsView の onAction は boolean を返す。
const acceptAction: (intent: DecisionIntent) => boolean = () => true;

// ---------------------------------------------------------------------------
// 判断待ち（D4〜D7）
// ---------------------------------------------------------------------------

const decisions = createActionableDashboardState().pendingDecisions;

const findDecision = (predicate: (decision: PendingDecision) => boolean): PendingDecision => {
  const found = decisions.find(predicate);
  assert.ok(found, "フィクスチャに対象の判断待ちがある");
  return found;
};

const merge = (): PendingDecision =>
  findDecision((decision) => decision.type === PendingDecisionType.PENDING_DECISION_TYPE_AWAITING_MERGE);
// #404（issue #308）で確認ダイアログが入った作業の依頼。
const operation = (): PendingDecision => findDecision((decision) => decision.inquiryId === "inquiry-operation");
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

type DialogKind =
  | "merge"
  | "approve-registration"
  | "reject-registration"
  | "answer-report-completed"
  | "answer-report-unable";

const renderActions = (decision: PendingDecision, dialog: DialogKind | null): string =>
  renderToStaticMarkup(
    createElement(DecisionActionsView, {
      decision,
      text: "",
      dialog,
      busy: false,
      disconnected: false,
      dialogTarget: null,
      onTextChange: noop,
      onAction: acceptAction,
    }),
  );

const dialogOf = (html: string): Element => {
  const dialog = documentOf(html).querySelector('[role="dialog"]');
  assert.ok(dialog, "確認ダイアログがある");
  return dialog;
};

const buttonTexts = (root: ParentNode): string[] =>
  Array.from(root.querySelectorAll("button"), (button) => normalize(button.textContent));

// ダイアログの対象の一覧（dt → dd）。
const dialogDetails = (dialog: Element): Map<string, string> =>
  new Map(
    Array.from(dialog.querySelectorAll("dl.dialog-target > div"), (row) => [
      normalize(row.querySelector("dt")?.textContent),
      normalize(row.querySelector("dd")?.textContent),
    ]),
  );

test("D4: 判断待ち一覧のマージの操作のボタンは「マージする」で、画面のどこにも「マージを承認」が出ない", () => {
  const html = renderDecisions(stateWith([merge()]));
  const item = documentOf(html).querySelector(".tray-item");
  assert.ok(item, "マージ待ちの項目がある");
  assert.deepEqual(buttonTexts(item), ["マージする"]);
  assert.equal(item.querySelector("button")?.getAttribute("aria-label"), "マージする");
  assert.doesNotMatch(html, /マージを承認/);
});

test("D4: マージの確認ダイアログは見出し・本文・確定ボタンを「マージ」の呼び名でそろえる", () => {
  const dialog = dialogOf(renderActions(merge(), "merge"));
  assert.equal(normalize(dialog.querySelector("h2")?.textContent), "プルリクエストのマージ");
  assert.equal(
    normalize(dialog.querySelector(".dialog-message")?.textContent),
    "プルリクエスト #418 をマージします。よろしいですか。",
  );
  assert.deepEqual(buttonTexts(dialog), ["やめる", "マージする"]);
  assert.doesNotMatch(dialog.outerHTML, /承認/);
});

test("D4: マージの成功の文は、backend がしたこと（マージの依頼）を PR の番号付きで言う", async () => {
  const action = buildMergeAction(merge());
  assert.ok(action, "マージの操作を組み立てられる");
  const fetchImpl = (async () => new Response(JSON.stringify({ ok: true }), { status: 200 })) as typeof fetch;
  const outcome = await submitDecisionAction(action, fetchImpl);
  assert.equal(outcome.ok, true);
  assert.equal(outcome.message, "PR #418 のマージを依頼しました。");
});

test("D4: マージできない項目の理由の文も「マージできません」にそろえる", () => {
  const cases: Array<[PendingDecision, string]> = [
    [{ ...merge(), goalId: undefined }, "ゴール ID が無いため、マージできません。"],
    [{ ...merge(), pullRequest: { number: 0, url: "https://example.test/pr/418" } }, "プルリクエストの番号が無いため、マージできません。"],
    [{ ...merge(), pullRequest: { number: 418, url: "" } }, "プルリクエストの URL が無いため、マージできません。"],
  ];
  for (const [decision, reason] of cases) {
    const html = renderActions(decision, null);
    assert.equal(normalize(documentOf(html).querySelector("p.note")?.textContent), reason);
    assert.doesNotMatch(html, /承認/);
  }
});

test("D5: ゴール登録の一覧のボタンは、何を承認・却下するのかを書く", () => {
  const html = renderDecisions(stateWith([registration()]));
  const item = documentOf(html).querySelector(".tray-item");
  assert.ok(item, "ゴール登録の項目がある");
  assert.deepEqual(buttonTexts(item), ["ゴール登録を承認する", "ゴール登録を却下する"]);
  const labels = Array.from(item.querySelectorAll("button"), (button) => button.getAttribute("aria-label"));
  assert.deepEqual(labels, ["ゴール登録を承認する", "ゴール登録を却下する"]);
  assert.doesNotMatch(html, /aria-label="承認"|aria-label="却下"/);
});

test("D5: ゴール登録の確認ダイアログの確定ボタンは、操作の名前を書き、プロジェクトと登録案を対象として出す", () => {
  const cases: Array<[DialogKind, string, string]> = [
    ["approve-registration", "ゴール登録の承認", "ゴール登録を承認する"],
    ["reject-registration", "ゴール登録の却下", "ゴール登録を却下する"],
  ];
  for (const [kind, heading, confirm] of cases) {
    const dialog = dialogOf(renderActions(registration(), kind));
    assert.equal(normalize(dialog.querySelector("h2")?.textContent), heading, "見出しは変えない");
    assert.deepEqual(buttonTexts(dialog), ["やめる", confirm]);
    const details = dialogDetails(dialog);
    assert.equal(details.get("プロジェクト"), "takt");
    assert.equal(details.get("登録案"), "新しいゴールを登録する提案です。");
    assert.equal(dialog.querySelector("a"), null, "登録案にはリンクを付けない");
  }
});

test("D5: 登録案の本文の最初の1文を案の名前にし、本文が空なら登録案の行を出さない", () => {
  const long: PendingDecision = {
    ...registration(),
    body: "採用ページの見出しを直すゴールです。背景として、いまの見出しは長すぎます。",
  };
  const longDetails = dialogDetails(dialogOf(renderActions(long, "approve-registration")));
  assert.equal(longDetails.get("登録案"), "採用ページの見出しを直すゴールです。");

  const empty: PendingDecision = { ...registration(), body: "" };
  const emptyDetails = dialogDetails(dialogOf(renderActions(empty, "approve-registration")));
  assert.equal(emptyDetails.get("プロジェクト"), "takt");
  assert.equal(emptyDetails.has("登録案"), false);
});

test("D6: 確認ダイアログの確定ボタンだけが danger の見た目を持ち、「やめる」は持たない", () => {
  const cases: Array<[PendingDecision, DialogKind]> = [
    [merge(), "merge"],
    [registration(), "approve-registration"],
    [registration(), "reject-registration"],
    // #404 で入った作業の依頼の2択の確定も、取り消せない書き込みなので danger にする（不変条件4）。
    [operation(), "answer-report-completed"],
    [operation(), "answer-report-unable"],
  ];
  for (const [decision, kind] of cases) {
    const dialog = dialogOf(renderActions(decision, kind));
    const [cancel, confirm] = Array.from(dialog.querySelectorAll("button"));
    assert.ok(cancel && confirm, `${kind} のダイアログに2つのボタンがある`);
    assert.equal(normalize(cancel.textContent), "やめる");
    assert.equal(cancel.classList.contains("danger"), false, `${kind} の「やめる」は danger を持たない`);
    assert.equal(confirm.classList.contains("danger"), true, `${kind} の確定ボタンは danger を持つ`);
  }
  // 一覧のボタンは開くだけなので、danger を付けない。
  const list = documentOf(renderDecisions(stateWith([merge(), registration(), operation()])));
  assert.equal(list.querySelectorAll(".tray-item button.danger").length, 0);
});

const cssPath = fileURLToPath(new URL("../../app/globals.css", import.meta.url));

type CssRule = { selectors: string[]; body: string };

const cssRules = (css: string): CssRule[] =>
  Array.from(css.replace(/\/\*[\s\S]*?\*\//g, "").matchAll(/([^{}]+)\{([^{}]*)\}/g), (match) => ({
    selectors: (match[1] ?? "").split(",").map((selector) => selector.trim()),
    body: match[2] ?? "",
  }));

const declarationOf = (rules: CssRule[], selector: string, property: string): string | undefined => {
  let latest: string | undefined;
  for (const rule of rules) {
    if (!rule.selectors.includes(selector)) {
      continue;
    }
    for (const match of rule.body.matchAll(new RegExp(`(?:^|;)\\s*${property}\\s*:\\s*([^;]+)`, "g"))) {
      latest = match[1]?.trim();
    }
  }
  return latest;
};

// var(--name) を :root のトークンの hex へ解決する。hex の直書きはそのまま返す。
const resolveColor = (css: string, value: string): string => {
  const token = /^var\((--[a-z0-9-]+)\)$/.exec(value)?.[1];
  if (token === undefined) {
    return value;
  }
  const root = /:root\s*\{([^}]*)\}/.exec(css)?.[1] ?? "";
  const raw = new RegExp(`${token}\\s*:\\s*([^;]+);`).exec(root)?.[1]?.trim();
  assert.ok(raw, `${token} が :root にある`);
  return resolveColor(css, raw);
};

test("D6: danger の確定ボタンは白地の「やめる」と違う地色で、文字色との比が 4.5:1 以上", async () => {
  const css = await readFile(cssPath, "utf8");
  const rules = cssRules(css);
  const background = declarationOf(rules, ".actions button.danger", "background");
  const color = declarationOf(rules, ".actions button.danger", "color");
  assert.ok(background, ".actions button.danger に地色がある");
  assert.ok(color, ".actions button.danger に文字色がある");
  assert.notEqual(background, "var(--surface)", "「やめる」と同じ白地にしない");
  assert.match(background, /^var\(--[a-z0-9-]+\)$/, "地色はトークンを使う");
  const ratio = contrastRatio(resolveColor(css, color), resolveColor(css, background));
  assert.ok(ratio >= 4.5, `文字色と地色の比 ${ratio.toFixed(2)} が 4.5 以上`);
});

test("D6: danger の確定ボタンは hover でも teal にならず、danger の地色と文字色のまま読める", async () => {
  const css = await readFile(cssPath, "utf8");
  const rules = cssRules(css);
  // .actions button:hover:not(:disabled) は .actions button.danger より詳細度が高いので、danger の hover を明示する。
  const selector = ".actions button.danger:hover:not(:disabled)";
  const background = declarationOf(rules, selector, "background");
  const color = declarationOf(rules, selector, "color");
  assert.ok(background, `${selector} に地色がある`);
  assert.ok(color, `${selector} に文字色がある`);
  assert.notEqual(color, "var(--accent)", "hover で文字を teal にしない");
  assert.notEqual(background, "var(--surface)", "hover で白地に戻さない");
  const ratio = contrastRatio(resolveColor(css, color), resolveColor(css, background));
  assert.ok(ratio >= 4.5, `hover の文字色と地色の比 ${ratio.toFixed(2)} が 4.5 以上`);
});

test("D6: 無効の danger の確定ボタンは、ほかの無効のボタンと同じ押せない見た目に戻す", async () => {
  const css = await readFile(cssPath, "utf8");
  const rules = cssRules(css);
  const selector = ".actions button.danger:disabled";
  assert.equal(declarationOf(rules, selector, "background"), "var(--surface)", `${selector} の地色`);
  const disabledColor = declarationOf(rules, ".actions button:disabled", "color");
  assert.ok(disabledColor, ".actions button:disabled に文字色がある");
  assert.equal(declarationOf(rules, selector, "color"), disabledColor, `${selector} の文字色はほかの無効と同じ`);
});

test("D5: 確認ダイアログは高さに上限を持ち、長い対象でも中を縦にスクロールして確定と「やめる」に届く", async () => {
  const css = await readFile(cssPath, "utf8");
  const rules = cssRules(css);
  const maxHeight = declarationOf(rules, ".dialog", "max-height");
  assert.ok(maxHeight !== undefined && maxHeight !== "none", ".dialog に max-height がある");
  assert.equal(declarationOf(rules, ".dialog", "overflow-y"), "auto");
});

test("D7: 結果の行は狭い幅で折り返し、「閉じる」の押す場所は 44px 以上", async () => {
  const css = await readFile(cssPath, "utf8");
  const rules = cssRules(css);
  assert.equal(declarationOf(rules, ".decision-results .result", "flex-wrap"), "wrap");
  // 「閉じる」は .actions の外にあるので、.actions button の 44px は当たらない。
  assert.equal(declarationOf(rules, ".decision-results .result button", "min-height"), "44px");
  assert.equal(declarationOf(rules, ".decision-results .result button", "min-width"), "44px");
});

test("D7: 操作の結果は、結果が出た時刻を持つ", () => {
  const before = Date.now();
  const result = toDecisionResult(merge(), { ok: true, message: "PR #418 のマージを依頼しました。" }) as DecisionResult & {
    at?: unknown;
  };
  assert.ok(result.at instanceof Date, "結果に時刻 at がある");
  assert.ok(result.at.getTime() >= before && result.at.getTime() <= Date.now(), "at は結果を作った時刻");
});

test("D7: 結果の一覧の各行に、結果が出た時刻を time 要素で出す", () => {
  const earlier = new Date("2026-11-13T07:20:00Z");
  const later = new Date("2026-11-13T07:29:00Z");
  const results = [
    { ...toDecisionResult(merge(), { ok: true, message: "PR #418 のマージを依頼しました。" }), at: later },
    { ...toDecisionResult(registration(), { ok: false, message: "操作に失敗しました。" }), at: earlier },
  ];
  const document = documentOf(renderToStaticMarkup(createElement(DecisionResults, { results })));
  const rows = Array.from(document.querySelectorAll(".decision-results li"));
  assert.equal(rows.length, 2);
  rows.forEach((row, index) => {
    const at = results[index]!.at;
    const time = row.querySelector("time");
    assert.ok(time, `${index + 1} 行目に time 要素がある`);
    assert.equal(normalize(time.textContent), formatClock(at));
    assert.equal(time.getAttribute("datetime"), at.toISOString());
  });
});

// ---------------------------------------------------------------------------
// run 詳細（D13、D15、D16、D22）
// ---------------------------------------------------------------------------

const renderRunDetail = (detail = createRunDetailResponse("run-aaa")): string =>
  renderToStaticMarkup(createElement(RunDetailContent, { detail, now: NOW }));

const stepHeads = (html: string): string[] =>
  Array.from(documentOf(html).querySelectorAll(".steps .step-head"), (head) => normalize(head.textContent));

test("D13: run 詳細のステップの状態は日本語のラベルで出し、英語の値を出さない", () => {
  const heads = stepHeads(renderRunDetail());
  assert.equal(heads.length, 2);
  assert.match(heads[0]!, /完了/);
  assert.match(heads[1]!, /実行中/);
  for (const head of heads) {
    assert.doesNotMatch(head, /\bdone\b|\brunning\b/);
  }

  const labels: Array<[string, string]> = [
    ["blocked", "止まった"],
    ["error", "エラー"],
    ["rate_limited", "利用上限"],
    ["pending", "未着手"],
  ];
  for (const [value, label] of labels) {
    const base = createRunDetailResponse("run-aaa");
    const detail = { ...base, stepLogs: [{ ...base.stepLogs[0]!, status: value }] };
    const [head] = stepHeads(renderRunDetail(detail));
    assert.match(head ?? "", new RegExp(label), `${value} は ${label}`);
    assert.ok(!(head ?? "").includes(value), `${value} をそのまま出さない`);
  }
});

test("D13: 知らないステップの状態は、届いた値のまま出す", () => {
  const base = createRunDetailResponse("run-aaa");
  const detail = { ...base, stepLogs: [{ ...base.stepLogs[0]!, status: "paused" }] };
  const [head] = stepHeads(renderRunDetail(detail));
  assert.match(head ?? "", /paused/);
});

test("D13: run 詳細の指示書の状態は日本語のラベルで出し、英語の値を出さない", () => {
  const labels: Array<[string, string]> = [
    ["pending", "未配信"],
    ["deliveredSameSession", "配信済み（同じセッション）"],
    ["deliveredNextStep", "配信済み（次のステップ）"],
    ["unconsumedWarned", "未消費（警告済み）"],
  ];
  for (const [value, label] of labels) {
    const base = createRunDetailResponse("run-aaa");
    const detail = { ...base, instructions: [{ ...base.instructions[0]!, state: value }] };
    const head = normalize(documentOf(renderRunDetail(detail)).querySelector(".instructions .ins-head")?.textContent);
    assert.ok(head.includes(label), `${value} は ${label}（実際: ${head}）`);
    assert.ok(!head.includes(value), `${value} をそのまま出さない`);
  }
});

test("D15: run 詳細のステップが0件なら、空であることを示す文を出し、空の一覧を出さない", () => {
  const document = documentOf(renderRunDetail({ ...createRunDetailResponse("run-aaa"), stepLogs: [] }));
  const section = document.querySelector('section[aria-labelledby="h-steps"]');
  assert.ok(section, "ステップの節がある");
  assert.equal(normalize(section.querySelector("p.note")?.textContent), "ステップはまだありません。");
  assert.equal(section.querySelector("ul.steps"), null);
});

const summaryValue = (html: string, key: string): string => {
  const field = Array.from(documentOf(html).querySelectorAll(".sum-grid .f")).find(
    (candidate) => normalize(candidate.querySelector(".k")?.textContent) === key,
  );
  assert.ok(field, `要約に「${key}」の欄がある`);
  return normalize(field.querySelector(".v")?.textContent);
};

const withServerEnv = async <T>(address: string, run: () => Promise<T>): Promise<T> => {
  await closeBackendClient();
  const previous = process.env.TAKTDASH_BACKEND_ADDR;
  process.env.TAKTDASH_BACKEND_ADDR = address;
  try {
    return await run();
  } finally {
    await closeBackendClient();
    if (previous === undefined) {
      delete process.env.TAKTDASH_BACKEND_ADDR;
    } else {
      process.env.TAKTDASH_BACKEND_ADDR = previous;
    }
  }
};

const renderRunDetailPage = async (): Promise<string> =>
  renderToStaticMarkup(await RunDetailLoader({ params: Promise.resolve({ slug: "run-aaa" }) }));

test("D16: run 詳細ページのサーバー欄は、概要から引いたサーバーの表示名を出す", async () => {
  const server = await startTaktDashServer({
    runDetail: createRunDetailResponse("run-aaa"),
    overview: createDashboardState(),
  });
  try {
    const html = await withServerEnv(server.address, renderRunDetailPage);
    assert.equal(summaryValue(html, "サーバー"), "Mac mini");
  } finally {
    await server.shutdown();
  }
});

test("D16: 概要が NOT_FOUND を返しても run 詳細ページは描画でき、サーバー欄は ID で代える", async () => {
  const server = await startTaktDashServer({
    runDetail: createRunDetailResponse("run-aaa"),
    overview: { code: grpc.status.NOT_FOUND, details: "未設定" },
  });
  try {
    const html = await withServerEnv(server.address, renderRunDetailPage);
    assert.equal(summaryValue(html, "サーバー"), "mini");
  } finally {
    await server.shutdown();
  }
});

test("D16: RunDetailContent に表示名を渡さないときは、サーバー欄に ID を出す", () => {
  assert.equal(summaryValue(renderRunDetail(), "サーバー"), "mini");
});

const backLinks = (html: string): Array<{ text: string; href: string | null }> =>
  Array.from(documentOf(html).querySelectorAll("a.back"), (link) => ({
    text: normalize(link.textContent),
    href: link.getAttribute("href"),
  }));

test("D22: run 詳細の戻るリンクは、ゴールがあればそのゴールの詳細へ、無ければプロジェクトへ戻す", () => {
  const withGoal = backLinks(renderRunDetail({ ...createRunDetailResponse("run-aaa"), goalId: "goal 1" }));
  assert.deepEqual(withGoal, [
    { text: "ゴール詳細に戻る", href: "/goals/goal%201" },
    { text: "ゴール詳細に戻る", href: "/goals/goal%201" },
  ]);

  const withoutGoal = backLinks(renderRunDetail({ ...createRunDetailResponse("run-aaa"), goalId: "" }));
  assert.deepEqual(withoutGoal, [
    { text: "プロジェクトに戻る", href: "/projects#project-corporate-site" },
    { text: "プロジェクトに戻る", href: "/projects#project-corporate-site" },
  ]);
});

// ---------------------------------------------------------------------------
// ゴール詳細（D13、D14）
// ---------------------------------------------------------------------------

const renderGoalDetail = (detail = createGoalDetailResponse()): string =>
  renderToStaticMarkup(createElement(GoalDetailContent, { detail }));

const questionItems = (html: string): Element[] =>
  Array.from(documentOf(html).querySelectorAll('section[aria-labelledby="h-questions"] li'));

test("D13: ゴール詳細の質問の種類と状態は日本語のラベルで出し、英語の値を出さない", () => {
  const [first, second] = questionItems(renderGoalDetail());
  assert.ok(first && second, "質問が2件ある");
  const firstHead = normalize(first.querySelector(".ins-head")?.textContent);
  const secondHead = normalize(second.querySelector(".ins-head")?.textContent);
  assert.match(firstHead, /質問/);
  assert.match(firstHead, /回答済み/);
  assert.match(secondHead, /作業の依頼/);
  assert.match(secondHead, /回答待ち/);
  for (const head of [firstHead, secondHead]) {
    assert.doesNotMatch(head, /question|operation_request|answered|pending/);
  }

  const other = createGoalDetailResponse();
  const approval = { ...other.questions[0]!, kind: "approval", status: "withdrawn" };
  const [approvalItem] = questionItems(renderGoalDetail({ ...other, questions: [approval] }));
  const approvalHead = normalize(approvalItem?.querySelector(".ins-head")?.textContent);
  assert.match(approvalHead, /承認の依頼/);
  assert.match(approvalHead, /取り下げ済み/);
  assert.doesNotMatch(approvalHead, /approval|withdrawn/);
});

test("D13: ゴール詳細の回答の判断と作業の報告は、日本語のラベルを「」に入れて出す", () => {
  const [first] = questionItems(renderGoalDetail());
  const firstText = normalize(first?.textContent);
  assert.match(firstText, /回答の判断は「拒否」です。/);
  assert.doesNotMatch(firstText, /deny/);

  const base = createGoalDetailResponse();
  const cases: Array<[Partial<(typeof base.questions)[number]>, string, string]> = [
    [{ answerDecision: "allow" }, "回答の判断は「許可」です。", "allow"],
    [{ answerDecision: undefined, answerReport: "completed" }, "作業の報告は「実施した」です。", "completed"],
    [{ answerDecision: undefined, answerReport: "unable" }, "作業の報告は「対応できない」です。", "unable"],
  ];
  for (const [override, sentence, raw] of cases) {
    const question = { ...base.questions[0]!, answerText: undefined, ...override };
    const [item] = questionItems(renderGoalDetail({ ...base, questions: [question] }));
    const text = normalize(item?.textContent);
    assert.ok(text.includes(sentence), `${raw} は「${sentence}」（実際: ${text}）`);
    assert.ok(!text.includes(raw), `${raw} をそのまま出さない`);
  }
});

test("D14: ゴール詳細は、いつも未取得になる開始ブランチの行を出さず、統合ブランチとゴールブランチは出す", () => {
  const document = documentOf(renderGoalDetail());
  const section = document.querySelector('section[aria-labelledby="h-branches"]');
  assert.ok(section, "ブランチの節がある");
  const rows = Array.from(section.querySelectorAll(".kv li"), (row) => [
    normalize(row.querySelector("span")?.textContent),
    normalize(row.querySelector("b")?.textContent),
  ]);
  assert.deepEqual(rows, [
    ["統合ブランチ", "main"],
    ["ゴールブランチ", "goal/45-frontend"],
  ]);
  assert.doesNotMatch(normalize(document.body.textContent), /開始ブランチ/);
});

// ---------------------------------------------------------------------------
// タスク詳細（D22）
// ---------------------------------------------------------------------------

const renderTaskDetail = (detail = createTaskDetailResponse()): string =>
  renderToStaticMarkup(createElement(TaskDetailContent, { detail }));

test("D22: タスク詳細は、h1 を繰り返すだけの「目的」の欄を出さない", () => {
  const keys = Array.from(documentOf(renderTaskDetail()).querySelectorAll(".sum-grid .k"), (key) =>
    normalize(key.textContent),
  );
  assert.ok(!keys.includes("目的"), `要約の欄に「目的」が無い（実際: ${keys.join("、")}）`);
  assert.ok(keys.includes("状態") && keys.includes("要約"), "状態と要約の欄は残す");
});

test("D22: タスク詳細の戻るリンクは、ゴールがあればそのゴールの詳細へ、無ければプロジェクトへ戻す", () => {
  const withGoal = backLinks(renderTaskDetail(createTaskDetailResponse({ goalId: "goal 1" })));
  assert.deepEqual(withGoal, [
    { text: "ゴール詳細に戻る", href: "/goals/goal%201" },
    { text: "ゴール詳細に戻る", href: "/goals/goal%201" },
  ]);

  const withoutGoal = backLinks(renderTaskDetail(createTaskDetailResponse({ goalId: undefined })));
  assert.deepEqual(withoutGoal, [
    { text: "プロジェクトに戻る", href: "/projects#project-corporate-site" },
    { text: "プロジェクトに戻る", href: "/projects#project-corporate-site" },
  ]);
});

// ---------------------------------------------------------------------------
// アクティビティ（D15、D18）
// ---------------------------------------------------------------------------

const renderActivity = (response = createActivityResponse()): Document =>
  documentOf(renderToStaticMarkup(createElement(ActivityContent, { state: createDashboardState(), response, now: NOW })));

test("D15: 日ごとの出来事が7日とも0件なら、空であることを示す文を出し、0件の行を並べない", () => {
  const zeroDays = createActivityResponse().dailySummaries.map((day) => ({ ...day, total: 0, attention: 0 }));
  for (const dailySummaries of [zeroDays, []]) {
    const document = renderActivity({ ...createActivityResponse(), dailySummaries });
    const section = document.querySelector('section[aria-labelledby="h-daily"]');
    assert.ok(section, "日ごとの出来事の節がある");
    assert.equal(normalize(section.querySelector("p.note")?.textContent), "この 7 日間の出来事はまだありません。");
    assert.equal(section.querySelector("ul.daily"), null);
  }
});

test("D15: 1日でも出来事がある7日分は、0件の日を含めて7行を出し、空の文を出さない", () => {
  const days = createActivityResponse().dailySummaries.map((day, index) => ({
    ...day,
    total: index === 3 ? 2 : 0,
    attention: 0,
  }));
  const document = renderActivity({ ...createActivityResponse(), dailySummaries: days });
  const section = document.querySelector('section[aria-labelledby="h-daily"]');
  assert.ok(section, "日ごとの出来事の節がある");
  assert.equal(section.querySelectorAll("ul.daily > li").length, 7);
  assert.equal(section.querySelector("p.note"), null);
});

test("D15: 今日の出来事（サーバー別）が0件なら、空であることを示す文を出し、空の一覧を出さない", () => {
  const document = renderActivity({ ...createActivityResponse(), todayCountsByServer: [] });
  const section = document.querySelector('section[aria-labelledby="h-today"]');
  assert.ok(section, "今日の出来事の節がある");
  assert.equal(normalize(section.querySelector("p.note")?.textContent), "今日の出来事はまだありません。");
  assert.equal(section.querySelector("ul.counts"), null);
});

test("D18: 日ごとの出来事の注意の件数は「うち要対応 N件」と書く", () => {
  const rows = Array.from(renderActivity().querySelectorAll("ul.daily > li"));
  assert.equal(rows.length, 7);
  let withAttention = 0;
  for (const row of rows) {
    const attention = row.querySelector(".attention");
    const text = normalize(row.textContent);
    if (attention === null) {
      assert.doesNotMatch(text, /うち/);
      continue;
    }
    withAttention += 1;
    assert.match(normalize(attention.textContent), /^うち要対応 \d+件$/);
  }
  assert.ok(withAttention > 0, "注意の件数がある日がある");
});

// ---------------------------------------------------------------------------
// 概要（D19）
// ---------------------------------------------------------------------------

test("D19: 概要の実行中の run の表は、中身に合わせて列名を「最後のログ」にし、モバイルの幅の列名も同じにする", () => {
  const html = renderToStaticMarkup(
    createElement(OverviewPage, { view: buildOverviewView(createDashboardState(), NOW, true) }),
  );
  const document = documentOf(html);
  const section = document.querySelector('section[aria-labelledby="h-runs"]');
  assert.ok(section, "実行中の run の節がある");
  const headers = Array.from(section.querySelectorAll("thead th"), (header) => normalize(header.textContent));
  assert.deepEqual(headers, ["プロジェクト", "ゴール", "ステップ", "最後のログ", "状態"]);
  const cell = section.querySelector("td.run-elapsed");
  assert.ok(cell, "最後のログの列のセルがある");
  assert.equal(cell.getAttribute("data-label"), "最後のログ");
  assert.match(normalize(cell.textContent), /^最後のログは .+前$/, "中身の文は変えない");
});

// ---------------------------------------------------------------------------
// 404・エラー・読み込み中（D17）
// ---------------------------------------------------------------------------

// issue #437: error.tsx は useRouter() を呼ぶが、renderToStaticMarkup には App Router が無く、
// 「invariant expected app router to be mounted」で落ちる。このファイルが error.tsx を読むときだけ、
// refresh が何もしないルータを返す next/navigation に差し替える。ほかの画面の import には影響させない。
const routerStub = "export const useRouter = () => ({ refresh: () => {} });";
const routerHook = `
export async function resolve(specifier, context, nextResolve) {
  if (specifier === "next/navigation" && context.parentURL?.endsWith("/app/error.tsx")) {
    return { url: ${JSON.stringify(`data:text/javascript,${encodeURIComponent(routerStub)}`)}, shortCircuit: true };
  }
  return nextResolve(specifier, context);
}
`;
register(`data:text/javascript,${encodeURIComponent(routerHook)}`);

// まだ無いファイルを静的に import すると型検査が通らないため、パスを値として読み込む。
const loadAppModule = async (name: string): Promise<{ default: ComponentType<Record<string, unknown>> }> => {
  const href = new URL(`../../app/${name}`, import.meta.url).href;
  return (await import(href)) as { default: ComponentType<Record<string, unknown>> };
};

const renderAppModule = async (name: string, props: Record<string, unknown> = {}): Promise<Document> => {
  const loaded = await loadAppModule(name);
  return documentOf(renderToStaticMarkup(createElement(loaded.default, props)));
};

const ENGLISH_DEFAULTS = /could not be found|Something went wrong|This page|Loading/i;

test("D17: 404 の画面は日本語で、URL の確かめ方と概要へ戻るリンクを出す", async () => {
  const document = await renderAppModule("not-found.tsx");
  assert.equal(normalize(document.querySelector("h1")?.textContent), "ページが見つかりません");
  assert.match(
    normalize(document.body.textContent),
    /URL が正しいかを確かめてください。run やゴールは、終わってから時間がたつと消えることがあります。/,
  );
  const back = Array.from(document.querySelectorAll("a")).find((link) => normalize(link.textContent) === "概要に戻る");
  assert.ok(back, "概要に戻るリンクがある");
  assert.equal(back.getAttribute("href"), "/");
  assert.doesNotMatch(normalize(document.body.textContent), ENGLISH_DEFAULTS);
});

test("D17: エラーの画面は日本語で、もう一度読み込む操作と概要へ戻るリンクを出し、エラーの文は出さない", async () => {
  const document = await renderAppModule("error.tsx", {
    error: Object.assign(new Error("backend の内部の文 secret-detail"), { digest: "digest-1" }),
    reset: noop,
  });
  const text = normalize(document.body.textContent);
  assert.equal(normalize(document.querySelector("h1")?.textContent), "画面を表示できませんでした");
  assert.match(text, /もう一度読み込んでください。続くときは、backend の状態を確かめてください。/);
  assert.deepEqual(buttonTexts(document), ["もう一度読み込む"]);
  const back = Array.from(document.querySelectorAll("a")).find((link) => normalize(link.textContent) === "概要に戻る");
  assert.ok(back, "概要に戻るリンクがある");
  assert.equal(back.getAttribute("href"), "/");
  assert.doesNotMatch(text, /secret-detail|digest-1/, "エラーの文を画面に出さない");
  assert.doesNotMatch(text, ENGLISH_DEFAULTS);
});

test("D17: 読み込み中の画面は日本語の文を status として出す", async () => {
  const document = await renderAppModule("activity/loading.tsx");
  const status = document.querySelector('[role="status"]');
  assert.ok(status, "status の要素がある");
  assert.equal(normalize(status.textContent), "読み込んでいます…");
});

// ---------------------------------------------------------------------------
// サイトの説明（D21）
// ---------------------------------------------------------------------------

test("D21: サイトの説明は、判断待ちの操作（書き込み）があることに合わせ、読み取り用と書かない", () => {
  assert.equal(
    metadata.description,
    "taktDash は、takt manager 群の状態を集約し、判断待ちを操作するダッシュボードです。",
  );
  assert.doesNotMatch(String(metadata.description), /読み取り用/);
});

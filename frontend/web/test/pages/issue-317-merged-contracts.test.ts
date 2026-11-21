// issue #317 と main の合流後の契約（D30 の維持要件 / 取り下げ要件 / 読み上げ名）。
//
// 見出し階層の変更は裁定により取り下げ、main の既存契約を正とする。D30 のうち、
// エラー以外の状態通知の role と会話ページの select の配置は維持する。読み上げ名は
// issue #324 で候補1（aria-describedby）を採用し、操作ボタンの読み上げ名から対象識別子を
// 外して別要素の説明で補う形へ更新した。ゴール一覧の見出しは main の h3 を保ちつつ、
// レベルを飛ばさない（h1 の次は h2 までに留める）。
import test from "node:test";
import assert from "node:assert/strict";
import { createElement, type ComponentType, type ReactElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import {
  createActionableDashboardState,
  createDashboardState,
  NOW,
} from "../support/fixtures.ts";
import { PendingDecisionType, type PendingDecision } from "../../src/proto/taktdash.ts";
import { GoalsContent } from "../../src/components/goals-content.tsx";
import { ConversationContent } from "../../src/components/conversation-content.tsx";
import { DecisionActionsView } from "../../src/components/decision-actions-view.tsx";

const renderPage = (Page: ComponentType<never>, props: unknown): string =>
  renderToStaticMarkup(createElement(Page as ComponentType<never>, props as never) as ReactElement);

const headingLevels = (html: string): number[] =>
  Array.from(html.matchAll(/<h([1-6])\b[^>]*>/g), (match) => Number(match[1]));

// D30 のうち、この合流で取り下げない要件を観測する。
// 見出し階層の変更は裁定で取り下げ、main の既存契約（ゴール一覧 h1→h3）を正とするため、
// ここではゴール一覧が h1 で始まり、ゴール行の h3 を保つことだけを観測する。
test("ゴール一覧の見出しは h1 で始まり、ゴール行に h3 を使う", () => {
  const html = renderPage(GoalsContent, { state: createDashboardState() });
  const levels = headingLevels(html);
  assert.ok(levels.length > 0, "ゴール一覧に見出しがある");
  assert.equal(levels[0], 1, "最初の見出しは h1");
  assert.ok(levels.includes(3), "main のゴール行見出し（h3）を保持する");
});

test("会話の選択用の select は main の内側に置く", () => {
  const page = { project: "esa", provider: "claude", messages: [], skipped: [] };
  const html = renderPage(ConversationContent, {
    project: "esa",
    page,
    picker: createElement(
      "div",
      { className: "conversation-picker" },
      createElement(
        "select",
        { id: "conversation-project", defaultValue: "esa" },
        createElement("option", { value: "esa" }, "esa"),
      ),
    ),
  });

  const mainStart = html.indexOf("<main");
  const mainEnd = html.indexOf("</main>");
  const selectStart = html.indexOf('id="conversation-project"');
  assert.ok(mainStart >= 0 && mainEnd > mainStart, "main が描画されている");
  assert.ok(selectStart >= 0, "プロジェクトの select が描画されている");
  assert.ok(
    selectStart > mainStart && selectStart < mainEnd,
    "select は main の内側にある（main の外に置かない）",
  );
});

test("会話のエラー以外の状態通知には補助技術が読める role が付く", () => {
  const loading = renderPage(ConversationContent, { project: "esa", loading: true });
  const loadingNote = /<p[^>]*class="[^"]*\bnote\b[^"]*"[^>]*>会話を読み込んでいます。<\/p>/.exec(loading)?.[0];
  assert.ok(loadingNote !== undefined, "読み込み中の通知が描画されている");
  assert.match(loadingNote, /role="status"/, "読み込み中の通知は role=\"status\" を持つ");

  const unavailable = renderPage(ConversationContent, {
    project: "esa",
    page: { project: "esa", provider: "claude", messages: [], skipped: [], unavailableReason: "記録がありません" },
  });
  const notice = /<p[^>]*class="[^"]*\bbanner warn\b[^"]*"[^>]*>会話を表示できません[^<]*<\/p>/.exec(unavailable)?.[0];
  assert.ok(notice !== undefined, "「表示できません」の通知が描画されている");
  assert.match(notice, /role="status"/, "エラー以外の状態通知は role=\"status\" を持つ");
});

const decisions = createActionableDashboardState().pendingDecisions;

const findDecision = (predicate: (decision: PendingDecision) => boolean): PendingDecision => {
  const found = decisions.find(predicate);
  assert.ok(found, "フィクスチャに対象の判断待ちがある");
  return found;
};

const renderActions = (decision: PendingDecision): string =>
  renderToStaticMarkup(
    createElement(DecisionActionsView, {
      decision,
      text: "",
      dialog: null,
      busy: false,
      dialogTarget: null,
      onTextChange: () => {},
      onAction: () => {},
    }),
  );

// issue-317-a11y.test.ts の読み上げ名契約は、issue #324 で候補1（aria-describedby）を採用する
// 裁定により、操作ボタンの読み上げ名から対象識別子を外し、別要素の説明で補う形へ更新した。
// ここでは、操作ボタンの読み上げ名が対象識別子を含まない簡潔な操作名であり、
// 対象は aria-describedby の説明要素から読めることを観測する。
test("判断待ちの操作ボタンの読み上げ名は操作の文言だけにし、対象は説明要素で補う", () => {
  const merge = findDecision(
    (decision) => decision.type === PendingDecisionType.PENDING_DECISION_TYPE_AWAITING_MERGE,
  );
  const html = renderActions(merge);
  const labels = Array.from(html.matchAll(/<button\b[^>]*\baria-label="([^"]*)"/g), (match) => match[1] ?? "");
  assert.ok(labels.length > 0, "操作ボタンが読み上げ名を持つ");
  for (const label of labels) {
    assert.doesNotMatch(label, /（/, `読み上げ名「${label}」は操作名（対象）の形ではなく対象を括弧書きしない`);
  }

  const describedButtons = Array.from(
    html.matchAll(/<button\b[^>]*\baria-label="[^"]*"[^>]*\baria-describedby="([^"]+)"/g),
    (match) => match[1] ?? "",
  );
  assert.ok(describedButtons.length > 0, "操作ボタンは説明参照を持つ");
});

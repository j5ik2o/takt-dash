// issue #317 D30 のうち、この PR で取り下げない契約を観測する。
// 見出し階層は main の既存契約（ゴール一覧 h1→h3、プロジェクト h1→h3→h4）を正とし、
// 見出しレベルの変更は取り下げた。選択要素の配置とエラー以外の状態通知の role は維持する。
import test from "node:test";
import assert from "node:assert/strict";
import { createElement, type ComponentType, type ReactElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import {
  createActivityResponse,
  createDashboardState,
  NOW,
} from "../support/fixtures.ts";
import { buildOverviewView } from "../../src/state/dashboard.ts";
import { GoalsContent } from "../../src/components/goals-content.tsx";
import { ConversationContent } from "../../src/components/conversation-content.tsx";
import { OverviewContent } from "../../src/components/overview-content.tsx";
import { ProjectsContent } from "../../src/components/projects-content.tsx";
import { ActivityContent } from "../../src/components/activity-content.tsx";
import { DecisionResults } from "../../src/components/decision-results.tsx";

const renderPage = (Page: ComponentType<never>, props: unknown): string =>
  renderToStaticMarkup(createElement(Page as ComponentType<never>, props as never) as ReactElement);

const headingLevels = (html: string): number[] =>
  Array.from(html.matchAll(/<h([1-6])\b[^>]*>/g), (match) => Number(match[1]));

// 各ページの先頭の見出しが h1 であることを観測する。段差の上限はここでは課さない
// （main のゴール一覧 h1→h3 を正として保持する裁定による）。
const assertStartsWithH1 = (html: string, label: string): void => {
  const levels = headingLevels(html);
  assert.ok(levels.length > 0, `${label} に見出しが無い`);
  assert.equal(levels[0], 1, `${label} の最初の見出しは h1`);
};

test("ゴール一覧の見出しは h1 から始まり、ゴール行の見出しに h3 を使う", () => {
  const state = createDashboardState();
  const html = renderPage(GoalsContent, { snapshotLoaded: true, state, disconnected: false });
  assertStartsWithH1(html, "ゴール一覧");
  assert.match(html, /<h3>/, "main のゴール行見出し（h3）を保持する");
});

test("全ページの先頭の見出しは h1 である", () => {
  assertStartsWithH1(renderPage(GoalsContent, { snapshotLoaded: true, state: createDashboardState(), disconnected: false }), "ゴール一覧");
  assertStartsWithH1(
    renderPage(OverviewContent, { view: buildOverviewView(createDashboardState(), NOW, true) }),
    "概要",
  );
  assertStartsWithH1(
    renderPage(ProjectsContent, { state: createDashboardState(), now: NOW }),
    "プロジェクト",
  );
  assertStartsWithH1(renderPage(ActivityContent, { state: createDashboardState(), response: createActivityResponse(), now: NOW }), "アクティビティ");
  assertStartsWithH1(
    renderPage(ConversationContent, { project: "esa" }),
    "会話",
  );
});

test("会話の選択用の select は main の内側に置く", () => {
  const page = {
    project: "esa",
    provider: "claude",
    messages: [],
    skipped: [],
  };
  // ConversationContent は main を所有する。選択要素を差し込むスロット（picker）が
  // main の先頭に描かれる。
  const html = renderPage(ConversationContent, {
    project: "esa",
    page,
    picker: createElement(
      "div",
      { className: "conversation-picker" },
      createElement("label", { htmlFor: "conversation-project" }, "プロジェクト"),
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
  // 読み込み中の通知。
  const loading = renderPage(ConversationContent, { project: "esa", loading: true });
  const loadingNote = /<p[^>]*class="[^"]*\bnote\b[^"]*"[^>]*>会話を読み込んでいます。<\/p>/.exec(loading)?.[0];
  assert.ok(loadingNote !== undefined, "読み込み中の通知が描画されている");
  assert.match(loadingNote, /role="status"/, "読み込み中の通知は role=\"status\" を持つ");

  // エラーではない「表示できません」の状態通知。
  const unavailable = renderPage(ConversationContent, {
    project: "esa",
    page: { project: "esa", provider: "claude", messages: [], skipped: [], unavailableReason: "記録がありません" },
  });
  const notice = /<p[^>]*class="[^"]*\bbanner warn\b[^"]*"[^>]*>会話を表示できません[^<]*<\/p>/.exec(unavailable)?.[0];
  assert.ok(notice !== undefined, "「表示できません」の通知が描画されている");
  assert.match(notice, /role="status"/, "エラー以外の状態通知は role=\"status\" を持つ");

  // 操作結果の成功は status、失敗は alert。
  const results = renderToStaticMarkup(
    createElement(DecisionResults, {
      results: [{ key: "k", label: "esa の問い合わせ", ok: true, message: "回答を送信しました。" }],
    }),
  );
  assert.match(results, /role="status"/, "操作結果の成功は補助技術が読める role を持つ");
});


// issue #317 の見出し・選択要素・状態通知・読み上げ名の契約（D30）。
//
// 各ページの描画 HTML から見出しの並びを取り、レベルの段差を観測する。会話ページは
// select が main の内側にあること、エラー以外の状態通知に補助技術が読める role が
// あることを観測する。判断待ちの操作ボタンは読み上げ名（aria-label か可視名）を
// 観測し、対象識別子を埋め込まず簡潔な操作名になっていることを確かめる。
import test from "node:test";
import assert from "node:assert/strict";
import { createElement, type ComponentType, type ReactElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import {
  createActionableDashboardState,
  createActivityResponse,
  createDashboardState,
  createProjectsResponse,
  NOW,
} from "../support/fixtures.ts";
import { PendingDecisionType, type PendingDecision } from "../../src/proto/taktdash.ts";
import { buildOverviewView } from "../../src/state/dashboard.ts";
import { GoalsContent } from "../../src/components/goals-content.tsx";
import { ConversationContent } from "../../src/components/conversation-content.tsx";
import { OverviewContent } from "../../src/components/overview-content.tsx";
import { ProjectsContent } from "../../src/components/projects-content.tsx";
import { ActivityContent } from "../../src/components/activity-content.tsx";
import { DecisionResults } from "../../src/components/decision-results.tsx";
import { DecisionActionsView } from "../../src/components/decision-actions-view.tsx";

const renderPage = (Page: ComponentType<never>, props: unknown): string =>
  renderToStaticMarkup(createElement(Page as ComponentType<never>, props as never) as ReactElement);

const headingLevels = (html: string): number[] =>
  Array.from(html.matchAll(/<h([1-6])\b[^>]*>/g), (match) => Number(match[1]));

// 直前の見出しとの段差が 1 以下（同じか1つ下がる）であること。最初の見出しは h1 を期待する。
const assertNoSkippedHeadingLevels = (html: string, label: string): void => {
  const levels = headingLevels(html);
  assert.ok(levels.length > 0, `${label} に見出しが無い`);
  assert.equal(levels[0], 1, `${label} の最初の見出しは h1`);
  for (let index = 1; index < levels.length; index += 1) {
    const previous = levels[index - 1]!;
    const current = levels[index]!;
    assert.ok(
      current - previous <= 1,
      `${label} の見出しが h${previous} から h${current} へ飛んでいる（飛ばしてはいけない）`,
    );
  }
};

test("ゴール一覧の見出しは h1 の次に h2 で続き、レベルを飛ばさない", () => {
  const state = createDashboardState();
  const html = renderPage(GoalsContent, { state });
  assertNoSkippedHeadingLevels(html, "ゴール一覧");
});

test("全ページの見出しレベルが1つずつ下がり、飛ばない", () => {
  assertNoSkippedHeadingLevels(renderPage(GoalsContent, { state: createDashboardState() }), "ゴール一覧");
  assertNoSkippedHeadingLevels(
    renderPage(OverviewContent, { view: buildOverviewView(createDashboardState(), NOW) }),
    "概要",
  );
  assertNoSkippedHeadingLevels(
    renderPage(ProjectsContent, { response: createProjectsResponse(), now: NOW }),
    "プロジェクト",
  );
  assertNoSkippedHeadingLevels(renderPage(ActivityContent, { response: createActivityResponse(), now: NOW }), "アクティビティ");
  assertNoSkippedHeadingLevels(
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

// 可視名と aria-label が同じ名前空間で衝突しないよう、aria-label があればそれを、
// 無ければボタンの内容を読み上げ名として返す。
const buttonNames = (html: string): string[] =>
  Array.from(html.matchAll(/<button\b([^>]*)>([\s\S]*?)<\/button>/g), (match) => {
    const attributes = match[1] ?? "";
    const label = /\baria-label="([^"]*)"/.exec(attributes)?.[1];
    return label ?? (match[2] ?? "").replace(/<[^>]+>/g, "").trim();
  });

// 対象識別子（サーバー・プロジェクト・PR 番号・revision・問い合わせ ID）を埋め込んだ読み上げ名。
const TARGET_HINTS = /corporate-site|takt|esa|PR #|revision |inquiry-/;

test("判断待ちの操作ボタンの読み上げ名は対象識別子を含まない簡潔な操作名にする", () => {
  const cases: PendingDecision[] = [
    findDecision((decision) => decision.type === PendingDecisionType.PENDING_DECISION_TYPE_AWAITING_MERGE),
    findDecision((decision) => decision.inquiryId === "inquiry-options"),
    findDecision((decision) => decision.inquiryId === "inquiry-operation"),
  ];
  for (const decision of cases) {
    const names = buttonNames(renderActions(decision));
    assert.ok(names.length > 0, "操作ボタンが描画されている");
    for (const name of names) {
      assert.doesNotMatch(
        name,
        TARGET_HINTS,
        `読み上げ名「${name}」に対象の識別子を埋め込んでいる（簡潔な操作名にする）`,
      );
      assert.ok(name.length <= 12, `読み上げ名「${name}」が長すぎる`);
    }
  }
});

test("ゴール登録の承認・却下のトリガーは操作を表す短い読み上げ名にする", () => {
  const decision = findDecision(
    (item) =>
      item.type === PendingDecisionType.PENDING_DECISION_TYPE_GOAL_REGISTRATION && item.revision === 7,
  );
  const names = buttonNames(renderActions(decision));
  assert.ok(names.includes("承認"), "承認のトリガーの読み上げ名は「承認」");
  assert.ok(names.includes("却下"), "却下のトリガーの読み上げ名は「却下」");
});

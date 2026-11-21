import test from "node:test";
import assert from "node:assert/strict";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { JSDOM } from "jsdom";
import { GoalsContent } from "../../src/components/goals-content.tsx";
import { ProjectsContent } from "../../src/components/projects-content.tsx";
import { GoalDetailContent } from "../../src/components/goal-detail-content.tsx";
import { dashboardStateFromProto } from "../../src/state/restore-state.ts";
import { createDashboardState, createGoal, createGoalDetailResponse, NOW } from "../support/fixtures.ts";

const goalState = (name: string, nameTruncated: boolean) => ({
  ...createDashboardState(),
  goals: [{ ...createGoal({ goalId: "goal/preview", name }), nameTruncated }],
});

test("省略された目的を展開しても全文とは表示せず第一文と詳細リンクを保つ", () => {
  const state = goalState("確認する。次の作業を始め", true);
  const html = renderToStaticMarkup(createElement(GoalsContent, {
    state: dashboardStateFromProto(state), snapshotLoaded: true, disconnected: false,
  }));
  const document = new JSDOM(html).window.document;
  const heading = document.querySelector("h3 a");
  assert.equal(heading?.textContent, "確認する。");
  assert.equal(heading?.getAttribute("href"), "/goals/goal%2Fpreview");
  const details = document.querySelector("details.goal-full");
  assert.ok(details);
  assert.equal(details.hasAttribute("open"), false);
  assert.match(details.querySelector("summary")?.textContent ?? "", /一部|省略/);
  assert.doesNotMatch(details.querySelector("summary")?.textContent ?? "", /全文/);
  assert.equal(details.querySelector(".goal-text")?.textContent, "確認する。次の作業を始め…");
});

test("プロジェクトの省略された目的には一部の表示と当該ゴールの詳細への導線がある", () => {
  const html = renderToStaticMarkup(createElement(ProjectsContent, {
    state: goalState("確認する。次の作業を始め", true), now: NOW,
  }));
  const document = new JSDOM(html).window.document;
  const details = document.querySelector("details.goal-full");
  assert.ok(details);
  assert.match(details.querySelector("summary")?.textContent ?? "", /一部|省略/);
  assert.doesNotMatch(details.querySelector("summary")?.textContent ?? "", /全文/);
  assert.equal(details.querySelector(".goal-text")?.textContent, "確認する。次の作業を始め…");
  const row = details.closest("li");
  assert.ok(row?.querySelector('a[href="/goals/goal%2Fpreview"]'));
});

test("完全な256バイトの目的には長さだけで省略記号を付けない", () => {
  const name = "a".repeat(256);
  const html = renderToStaticMarkup(createElement(GoalsContent, {
    state: dashboardStateFromProto(goalState(name, false)), snapshotLoaded: true, disconnected: false,
  }));
  const document = new JSDOM(html).window.document;
  assert.equal(document.querySelector("h3 a")?.textContent, name);
});

test("ゴールの題名は引用内の句点で切らず閉じ忘れの引用も保持する", () => {
  for (const [name, expected] of [
    ["手順「確認。続行」を読む。次へ。", "手順「確認。続行」を読む。"],
    ["手順「確認。続行", "手順「確認。続行"],
  ] as const) {
    const html = renderToStaticMarkup(createElement(GoalsContent, {
      state: dashboardStateFromProto(goalState(name, false)), snapshotLoaded: true, disconnected: false,
    }));
    const document = new JSDOM(html).window.document;
    assert.equal(document.querySelector("h3 a")?.textContent, expected);
  }
});

test("ゴール詳細は一覧の上限を超える目的を原文の末尾まで表示する", () => {
  const objective = `確認する。\n${"あ".repeat(3000)}目的の末尾。`;
  const detail = createGoalDetailResponse({ objective });
  const html = renderToStaticMarkup(createElement(GoalDetailContent, { detail }));
  const document = new JSDOM(html).window.document;
  assert.ok(document.body.textContent?.includes(objective));
});

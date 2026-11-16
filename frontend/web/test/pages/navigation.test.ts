// ゴール画面への導線の契約（NAV-01 / NAV-02 / NAV-03）。
//
// 利用者が既存のプロジェクト画面・概要・sidebar からゴール一覧へ到達できることを、
// 描画されたリンクの href で観測する。既存の run への導線が壊れていないことも確認する。
import test from "node:test";
import assert from "node:assert/strict";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { createDashboardState, NOW } from "../support/fixtures.ts";
import { buildOverviewView } from "../../src/state/dashboard.ts";
import { OverviewContent } from "../../src/components/overview-content.tsx";
import { ProjectsContent } from "../../src/components/projects-content.tsx";
import { Sidebar } from "../../src/components/sidebar.tsx";

test("プロジェクト画面のゴールからプロジェクトで絞り込んだゴール一覧へ入れる", () => {
  const html = renderToStaticMarkup(
    createElement(ProjectsContent, { state: createDashboardState(), now: NOW }),
  );
  assert.match(html, /href="\/goals\?project=corporate-site"/);
  assert.match(html, /href="\/goals\?project=esa"/);
  assert.match(html, /採用ページのレイアウトを直す/);
});

test("概要のプロジェクトカードからプロジェクトで絞り込んだゴール一覧へ入れる", () => {
  const view = buildOverviewView(createDashboardState(), NOW);
  const html = renderToStaticMarkup(createElement(OverviewContent, { view }));
  assert.match(html, /href="\/goals\?project=corporate-site"/);
  assert.match(html, /href="\/goals\?project=esa"/);
  // 既存の run への導線を壊していない。
  assert.match(html, /href="\/runs\/run-aaa"/);
});

test("sidebar にゴール一覧の項目が既存項目と同じ並びで加わる", () => {
  const state = createDashboardState();
  const html = renderToStaticMarkup(
    createElement(Sidebar, { servers: state.servers, pendingCount: 2 }),
  );
  assert.match(html, /href="\/goals"/);
  assert.match(html, />ゴール</);
  for (const href of ['href="/"', 'href="/decisions"', 'href="/projects"', 'href="/activity"']) {
    assert.ok(html.includes(href), `${href} の既存項目が残る`);
  }
  assert.match(html, /2件/);
});

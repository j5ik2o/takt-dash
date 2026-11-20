// プロジェクトページのゴール表示の契約（order.md の競合解消方針）。
//
// #137 でプロジェクトカードのゴールは GoalRow になり、プロジェクトで絞り込んだ
// ゴール一覧へのリンクは持たない。合流時に旧来の絞り込みリンクを復活させないことを、
// プロジェクトカード単位の HTML から観測する。
import test from "node:test";
import assert from "node:assert/strict";
import { createElement, type ComponentType, type ReactElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { createDashboardState, NOW } from "../support/fixtures.ts";
import ProjectsPage from "../../app/projects/page.tsx";

const renderPage = (Page: ComponentType<never>, props: unknown): string =>
  renderToStaticMarkup(createElement(Page as ComponentType<never>, props as never) as ReactElement);

const cardFor = (html: string, project: string): string => {
  const nameIndex = html.indexOf(`<h3>${project}</h3>`);
  assert.ok(nameIndex >= 0, `${project} のカード見出しが描画されていない`);
  const start = html.lastIndexOf("<article", nameIndex);
  const end = html.indexOf("</article>", nameIndex);
  assert.ok(start >= 0 && end >= 0, `${project} のカードが描画されていない`);
  return html.slice(start, end + "</article>".length);
};

test("プロジェクトページのゴールは一覧の絞り込みリンクを持たない", () => {
  const html = renderPage(ProjectsPage, { state: createDashboardState(), now: NOW });
  const card = cardFor(html, "corporate-site");

  assert.ok(card.includes("採用ページのレイアウトを直す"), "進行中のゴールをカードに描画する");
  assert.doesNotMatch(card, /href="\/goals\?project=/, "ゴールを一覧の絞り込みリンクにしない");
});

// 概要・プロジェクト・サイドバーのリンクの契約（issue #34 / #35 / #36）。
//
// 概要のプロジェクトカードは /projects の当該カードへ、サイドバーのサーバー名は
// 概要の当該サーバーカードへ、サイドバーのブランドは / へ遷移する。リンク先の
// fragment と遷移先の id は同じ規則で作られ、既存の見出し id と衝突しない。
// 描画結果の HTML からリンクと id を観測し、CSS から hover と :focus-visible を観測する。
import test from "node:test";
import assert from "node:assert/strict";
import { createElement, type ComponentType, type ReactElement } from "react";
import { readFile } from "node:fs/promises";
import { renderToStaticMarkup } from "react-dom/server";
import { fileURLToPath } from "node:url";
import { createDashboardState, createProjectsResponse, NOW } from "../support/fixtures.ts";
import { buildOverviewView } from "../../src/state/dashboard.ts";
import { Sidebar } from "../../src/components/sidebar.tsx";
import OverviewPage from "../../app/page.tsx";
import ProjectsPage from "../../app/projects/page.tsx";

const webRoot = fileURLToPath(new URL("../..", import.meta.url));

const renderPage = (Page: ComponentType<never>, props: unknown): string =>
  renderToStaticMarkup(createElement(Page as ComponentType<never>, props as never) as ReactElement);

const escapeRegExp = (value: string): string => value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

type Anchor = { tag: string; inner: string };

const anchorFor = (html: string, href: string): Anchor => {
  const pattern = new RegExp(`<a\\b[^>]*\\bhref="${escapeRegExp(href)}"[^>]*>([\\s\\S]*?)<\\/a>`, "i");
  const match = pattern.exec(html);
  assert.ok(match, `href="${href}" のリンクが描画されていない`);
  const end = match[0].indexOf(">") + 1;
  return { tag: match[0].slice(0, end), inner: match[1] };
};

const openingTagWithAttribute = (html: string, attribute: string, value: string): { name: string; tag: string } => {
  const pattern = new RegExp(`<([a-z][a-z0-9]*)\\b[^>]*\\b${attribute}="${escapeRegExp(value)}"`, "i");
  const match = pattern.exec(html);
  assert.ok(match, `${attribute}="${value}" を持つ要素が描画されていない`);
  const end = html.indexOf(">", match.index) + 1;
  return { name: match[1].toLowerCase(), tag: html.slice(match.index, end) };
};

const renderOverviewWithState = (state = createDashboardState()): string =>
  renderPage(OverviewPage, { view: buildOverviewView(state, NOW) });

const renderProjectsWithState = (state = createDashboardState()): string =>
  renderPage(ProjectsPage, { response: { ...createProjectsResponse(), state }, now: NOW });

const renderSidebarWithState = (state = createDashboardState()): string =>
  renderToStaticMarkup(createElement(Sidebar, { servers: state.servers, pendingCount: 0 }));

test("概要のプロジェクトカードは当該プロジェクトのカードへのリンクで、カード全体が1つのリンクになる", () => {
  const state = createDashboardState();
  const html = renderOverviewWithState(state);
  for (const project of state.projects) {
    const { tag, inner } = anchorFor(html, `/projects#project-${project.name}`);
    assert.match(tag, /class="[^"]*\bcard\b/, "リンクがカードの見た目を持つ");
    assert.match(inner, new RegExp(escapeRegExp(project.name)), "見出しがリンク内にある");
    assert.match(inner, new RegExp(escapeRegExp(project.about)), "説明がリンク内にある");
    assert.match(inner, new RegExp(`直近24時間に完了したタスクは\\s*${project.completedTasks24h}件`), "完了件数がリンク内にある");
    assert.doesNotMatch(inner, /<a\b/i, "カード内に入れ子のリンクを作らない");
  }
});

test("プロジェクトページのカード id は概要のリンク先の断片と一致する", () => {
  const state = createDashboardState();
  const overview = renderOverviewWithState(state);
  const projects = renderProjectsWithState(state);
  for (const project of state.projects) {
    const href = `/projects#project-${project.name}`;
    anchorFor(overview, href);
    const fragment = href.split("#")[1];
    const { name, tag } = openingTagWithAttribute(projects, "id", fragment);
    assert.equal(name, "article", "カードの article に id を付ける");
    assert.match(tag, /class="[^"]*\bcard\b/, "id はカードの article に付く");
    assert.match(overview, /id="h-projects"/, "概要の既存の見出し id を残す");
  }
});

test("プロジェクト名が既存の見出し id と衝突してもカード id はリンク先と一致する", () => {
  const base = createDashboardState();
  const state = {
    ...base,
    projects: [{ name: "h-projects", about: "衝突を確かめるプロジェクト", serverId: "mini", completedTasks24h: 1 }],
  };
  const overview = renderOverviewWithState(state);
  anchorFor(overview, "/projects#project-h-projects");
  const projects = renderProjectsWithState(state);
  const { name, tag } = openingTagWithAttribute(projects, "id", "project-h-projects");
  assert.equal(name, "article");
  assert.match(tag, /class="[^"]*\bcard\b/);
  assert.match(overview, /id="h-projects"/, "プロジェクト名の h-projects と概要の見出し id が衝突しない");
});

test("サイドバーのブランド「taktDash」はトップページへのリンクになる", () => {
  const html = renderSidebarWithState();
  const { tag, inner } = anchorFor(html, "/");
  assert.match(tag, /class="[^"]*\bbrand\b/, "ブランドのリンクが brand の見た目を持つ");
  assert.match(inner, /^\s*taktDash\s*$/, "リンクの内容はブランド名だけ");
});

test("サイドバーのサーバー名は概要の当該サーバーカードへのリンクで、状態表示はリンク外に残る", () => {
  const state = createDashboardState();
  const sidebar = renderSidebarWithState(state);
  const overview = renderOverviewWithState(state);
  for (const server of state.servers) {
    const href = `/#server-${server.serverId}`;
    const { inner } = anchorFor(sidebar, href);
    assert.match(inner, new RegExp(escapeRegExp(server.name)), "サーバー名がリンク内にある");
    assert.doesNotMatch(inner, /動作中|応答なし/, "状態表示はリンク内に入れない");
    const fragment = href.split("#")[1];
    const { name, tag } = openingTagWithAttribute(overview, "id", fragment);
    assert.equal(name, "article", "サーバーカードの article に id を付ける");
    assert.match(tag, /class="[^"]*\bscard\b/, "id はサーバーカードに付く");
  }
  assert.match(overview, /id="h-servers"/, "既存の見出し id を残す");
});

test("サーバー ID が既存の見出し id と衝突してもカード id はリンク先と一致する", () => {
  const base = createDashboardState();
  const state = {
    ...base,
    servers: [{ serverId: "h-servers", name: "Mac mini", kind: "Mac mini", connected: true, lastReadAt: NOW, concurrency: 4 }],
    runs: [],
  };
  const sidebar = renderSidebarWithState(state);
  anchorFor(sidebar, "/#server-h-servers");
  const overview = renderOverviewWithState(state);
  const { name, tag } = openingTagWithAttribute(overview, "id", "server-h-servers");
  assert.equal(name, "article");
  assert.match(tag, /class="[^"]*\bscard\b/);
  assert.match(overview, /id="h-servers"/, "サーバー ID の h-servers と見出しの id が衝突しない");
});

test("新しいリンクは hover とキーボードフォーカスでリンクと分かる", async () => {
  const css = await readFile(`${webRoot}/app/globals.css`, "utf8");
  for (const selector of ["card-link", "brand", "srv-list a"]) {
    assert.match(css, new RegExp(`\\.${escapeRegExp(selector)}:hover`), `.${selector} の hover の見た目がない`);
    assert.match(
      css,
      new RegExp(`\\.${escapeRegExp(selector)}\\s*:\\s*focus-visible`),
      `.${selector} の :focus-visible の見た目がない`,
    );
  }
});

test("概要・プロジェクト・サイドバーは操作要素を増やさない", () => {
  for (const html of [renderOverviewWithState(), renderProjectsWithState(), renderSidebarWithState()]) {
    assert.doesNotMatch(html, /<button/i);
    assert.doesNotMatch(html, /<form/i);
    assert.doesNotMatch(html, /<input/i);
  }
});

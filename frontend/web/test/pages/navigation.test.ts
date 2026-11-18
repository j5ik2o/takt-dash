// サイドバーの現在ページの契約（AC77-2）。
//
// 現在のパスがナビの href と完全一致したときだけ、そのリンクを現在ページとして
// 強調し、aria-current="page" を付ける。ナビに無い run 詳細のパスではどのリンクも
// 現在ページにしない。描画結果の HTML と、強調の CSS を観測する。
import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { createDashboardState, NOW } from "../support/fixtures.ts";
import { buildOverviewView } from "../../src/state/dashboard.ts";
import { OverviewContent } from "../../src/components/overview-content.tsx";
import { ProjectsContent } from "../../src/components/projects-content.tsx";
import { Sidebar } from "../../src/components/sidebar.tsx";

const cssPath = fileURLToPath(new URL("../../app/globals.css", import.meta.url));

type NavLink = { href: string; tag: string; inner: string };

const renderSidebar = (currentPath?: string): string =>
  renderToStaticMarkup(
    createElement(Sidebar, {
      servers: createDashboardState().servers,
      pendingCount: 0,
      frontendBuild: { commit: "a1b2c3d", branch: "takt/frontend" },
      backendBuild: { commit: "a1b2c3d", branch: "takt/backend" },
      ...(currentPath === undefined ? {} : { currentPath }),
    }),
  );

// ページのナビ（.nav）にあるリンクだけを、開始タグと内容に分けて取り出す。
const navLinks = (html: string): NavLink[] => {
  const nav = /<ul class="nav">([\s\S]*?)<\/ul>/.exec(html)?.[1];
  if (nav === undefined) {
    assert.fail("ページのナビが描画されていない");
  }
  return Array.from(nav.matchAll(/<a\b([^>]*)>([\s\S]*?)<\/a>/g), (match) => {
    const attributes = match[1] ?? "";
    const href = /\bhref="([^"]*)"/.exec(attributes)?.[1] ?? "";
    return { href, tag: `<a${attributes}>`, inner: match[2] ?? "" };
  });
};

const hasCurrentAttribute = (link: NavLink): boolean => /\baria-current="page"/.test(link.tag);

const hasOnClass = (link: NavLink): boolean => /class="[^"]*\bon\b[^"]*"/.test(link.tag);

const currentLinks = (html: string): NavLink[] => navLinks(html).filter(hasCurrentAttribute);

test("判断待ちのパスでは判断待ちのリンクだけを現在ページにする", () => {
  const links = navLinks(renderSidebar("/decisions"));

  assert.equal(links.length, 6, "ナビのリンクは6件（ゴールと会話ページを含む）");
  const current = links.filter(hasCurrentAttribute);
  assert.equal(current.length, 1, "aria-current=\"page\" を持つリンクは1件だけ");
  assert.equal(current[0]?.href, "/decisions", "現在ページは判断待ちのリンク");
  assert.equal(hasOnClass(current[0]!), true, "現在ページのリンクは強調クラスを持つ");

  for (const link of links) {
    if (link.href === "/decisions") {
      continue;
    }
    assert.equal(hasCurrentAttribute(link), false, `${link.href} には aria-current を付けない`);
    assert.equal(hasOnClass(link), false, `${link.href} は強調しない`);
  }
});

test("概要のパスでは概要のリンクだけを現在ページにする", () => {
  const links = navLinks(renderSidebar("/"));

  const current = links.filter(hasCurrentAttribute);
  assert.equal(current.length, 1, "aria-current=\"page\" を持つリンクは1件だけ");
  assert.equal(current[0]?.href, "/", "現在ページは概要のリンク");
  assert.equal(hasOnClass(current[0]!), true, "現在ページのリンクは強調クラスを持つ");
  for (const link of links.filter((candidate) => candidate.href !== "/")) {
    assert.equal(hasOnClass(link), false, `${link.href} は強調しない`);
  }
});

test("ナビに無い run 詳細のパスではどのリンクも現在ページにしない", () => {
  const html = renderSidebar("/runs/run-aaa");

  assert.equal(currentLinks(html).length, 0, "前方一致で概要のリンクを現在ページにしない");
  for (const link of navLinks(html)) {
    assert.equal(hasOnClass(link), false, `${link.href} は強調しない`);
  }
});

test("現在のパスが渡されないときはどのリンクも現在ページにしない", () => {
  const html = renderSidebar();

  assert.equal(currentLinks(html).length, 0, "パスが無ければ現在ページを決めない");
  for (const link of navLinks(html)) {
    assert.equal(hasOnClass(link), false, `${link.href} は強調しない`);
  }
});

test("現在ページのリンクは淡い緑の背景で強調される", async () => {
  const css = await readFile(cssPath, "utf8");

  assert.match(
    css,
    /\.nav\s+a\.on\s*\{[^}]*background\s*:\s*var\(--/,
    ".nav a.on は変数で背景色を指定する",
  );
});

test("プロジェクト画面にプロジェクトごとの進行中ゴールが並ぶ", () => {
  const html = renderToStaticMarkup(
    createElement(ProjectsContent, { state: createDashboardState(), now: NOW }),
  );
  assert.match(html, /進行中のゴール/);
  assert.match(html, /採用ページのレイアウトを直す/);
  assert.match(html, /v2\.4\.0 を公開する/);
  // 既存の run への導線を壊していない。
  assert.match(html, /href="\/runs\/run-aaa"/);
});

test("概要のプロジェクトカードからプロジェクトで絞り込んだゴール一覧へ入れる", () => {
  const view = buildOverviewView(createDashboardState(), NOW, true);
  const html = renderToStaticMarkup(createElement(OverviewContent, { view, disconnected: false }));
  assert.match(html, /href="\/goals\?project=corporate-site"/);
  assert.match(html, /href="\/goals\?project=esa"/);
  // 既存の run への導線を壊していない。
  assert.match(html, /href="\/runs\/run-aaa"/);
});

test("sidebar にゴール一覧の項目が既存項目と同じ並びで加わる", () => {
  const state = createDashboardState();
  const html = renderToStaticMarkup(
    createElement(Sidebar, {
      servers: state.servers,
      pendingCount: 2,
      frontendBuild: { commit: "a1b2c3d", branch: "takt/frontend" },
      backendBuild: { commit: "a1b2c3d", branch: "takt/backend" },
    }),
  );
  assert.match(html, /href="\/goals"/);
  assert.match(html, />ゴール</);
  for (const href of ['href="/"', 'href="/decisions"', 'href="/projects"', 'href="/activity"']) {
    assert.ok(html.includes(href), `${href} の既存項目が残る`);
  }
  assert.match(html, /2件/);
});

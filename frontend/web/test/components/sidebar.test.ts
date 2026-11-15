// サイドバーのページ遷移の契約（order.md:15）。
//
// 会話ページは既存のナビゲーションから到達できる必要があり、既存のページの
// リンクと表示は壊さない。リンクの href と表示名を HTML から観測する。
import test from "node:test";
import assert from "node:assert/strict";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { Sidebar } from "../../src/components/sidebar.tsx";

test("サイドバーは既存の4ページのリンクを保ち、会話ページへのリンクを持つ", () => {
  const html = renderToStaticMarkup(createElement(Sidebar, { servers: [], pendingCount: 0 }));

  const existing = [
    { href: "/", label: "概要" },
    { href: "/decisions", label: "判断待ち" },
    { href: "/projects", label: "プロジェクト" },
    { href: "/activity", label: "アクティビティ" },
  ];
  for (const page of existing) {
    assert.ok(html.includes(`href="${page.href}"`), `既存のリンク ${page.href} を残す: ${html}`);
    assert.ok(html.includes(`>${page.label}<`), `既存の表示名 ${page.label} を残す: ${html}`);
  }

  assert.ok(html.includes('href="/conversations"'), `会話ページへのリンクを持つ: ${html}`);
  assert.ok(html.includes(">会話<"), `会話ページの表示名を持つ: ${html}`);
});

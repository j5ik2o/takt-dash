// サイドバーのページ遷移とビルド情報の契約（issue #73 / AC1 / AC2 / issue #317 D26）。
//
// 会話ページは既存のナビゲーションから到達できる必要があり、既存のページの
// リンクと表示は壊さない。リンクの href と表示名を HTML から観測する。
// サイドバー下端には frontend と backend のビルド元（短縮 SHA とブランチ名）を表示し、
// 両方のコミットが取得できていて異なるときだけ不一致を示す。狭い幅で本文を押し下げない
// よう、下端は折りたためる区画として描画する。
import test from "node:test";
import assert from "node:assert/strict";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { Sidebar } from "../../src/components/sidebar.tsx";
import { createDashboardState, NOW } from "../support/fixtures.ts";

type BuildInfo = { commit: string; branch: string };

const FRONTEND_BUILD: BuildInfo = { commit: "a1b2c3d", branch: "takt/20261116-frontend" };
const BACKEND_BUILD: BuildInfo = { commit: "d4e5f6a", branch: "takt/20261116-backend" };

const renderSidebar = (
  builds: { frontend: BuildInfo; backend: BuildInfo } = {
    frontend: FRONTEND_BUILD,
    backend: BACKEND_BUILD,
  },
  extra: { servers?: Parameters<typeof Sidebar>[0]["servers"]; readAt?: Date } = {},
): string =>
  renderToStaticMarkup(
    createElement(Sidebar, {
      servers: extra.servers ?? [],
      pendingCount: 0,
      readAt: extra.readAt,
      frontendBuild: builds.frontend,
      backendBuild: builds.backend,
    }),
  );

// サイドバー下端（.side-foot）の中身だけを取り出す。ビルド情報がナビや本文ではなく
// 下端に出ていることを、位置の契約として観測する。issue #317 D26 で狭い幅では
// 折りたたむため、要素は details（開閉可能な区画）でもよい。閉じた状態でも中身は
// DOM に残り、開くと読める。
const sideFoot = (html: string): string => {
  const match = /<(?:div|details)\b[^>]*\bclass="side-foot"[^>]*>([\s\S]*?)<\/(?:div|details)>/.exec(html);
  assert.ok(match, `サイドバー下端（.side-foot）が描画されていません: ${html}`);
  return match[1]!;
};

test("サイドバーは既存の4ページのリンクを保ち、会話ページへのリンクを持つ", () => {
  const html = renderSidebar();

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

test("サイドバー下端に frontend と backend の短縮 SHA とブランチ名を表示する", () => {
  const foot = sideFoot(renderSidebar());

  assert.match(foot, /a1b2c3d/, "frontend の短縮 SHA を表示する");
  assert.match(foot, /takt\/20261116-frontend/, "frontend のブランチ名を表示する");
  assert.match(foot, /d4e5f6a/, "backend の短縮 SHA を表示する");
  assert.match(foot, /takt\/20261116-backend/, "backend のブランチ名を表示する");
});

test("サイドバー下端の既存表示（最終読み取り時刻・読めた台数）を残す", () => {
  const foot = sideFoot(renderSidebar());

  assert.match(foot, /最終読み取り時刻/, "最終読み取り時刻を残す");
  assert.match(foot, /0台中0台から読めています/, "読めているサーバーの台数を残す");
});

test("frontend と backend のコミットが異なるときはサイドバー下端に不一致を示す", () => {
  const foot = sideFoot(renderSidebar());

  assert.match(foot, /不一致/, "コミットの不一致を画面で区別できる");
});

test("frontend と backend のコミットが同じときは不一致を示さない", () => {
  const foot = sideFoot(
    renderSidebar({
      frontend: { commit: "a1b2c3d", branch: "takt/frontend" },
      backend: { commit: "a1b2c3d", branch: "takt/backend" },
    }),
  );

  assert.doesNotMatch(foot, /不一致/, "同じコミットでは不一致を出さない");
});

test("片方のコミットが unknown のときは不一致を示さない", () => {
  const foot = sideFoot(
    renderSidebar({
      frontend: { commit: "unknown", branch: "unknown" },
      backend: { commit: "d4e5f6a", branch: "takt/backend" },
    }),
  );

  assert.match(foot, /unknown/, "取得できない側は unknown を表示する");
  assert.doesNotMatch(foot, /不一致/, "差の有無を判定できないときは不一致を出さない");
});

// issue #317 D26: 390px でサイドバー下端の情報は折りたたまれる。閉じた状態でも
// 中身は DOM に残り、開けば同じ内容が読める。
test("サイドバー下端は折りたためる区画として描画し、開けば内容を読める", () => {
  const html = renderSidebar(
    { frontend: FRONTEND_BUILD, backend: BACKEND_BUILD },
    { servers: createDashboardState().servers, readAt: NOW },
  );

  assert.match(
    html,
    /<details\b[^>]*\bclass="side-foot"[^>]*>/,
    "サイドバー下端は details で折りたためる（狭い幅で閉じた状態を出す）",
  );
  const summary = /<details\b[^>]*class="side-foot"[^>]*>\s*<summary\b[^>]*>([\s\S]*?)<\/summary>/.exec(html);
  assert.ok(summary, "折りたたみの要約（summary）があり、閉じた状態でも何の区画か分かる");

  const foot = sideFoot(html);
  assert.match(foot, /最終読み取り時刻/, "開けば最終読み取り時刻を読める");
  assert.match(foot, /2台中1台から読めています/, "開けば読めている台数を読める");
  assert.match(foot, /a1b2c3d/, "開けば frontend のビルド情報を読める");
});

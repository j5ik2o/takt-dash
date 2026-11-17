// サイドバーのページ遷移とビルド情報の契約（issue #73 / AC1 / AC2）。
//
// 会話ページは既存のナビゲーションから到達できる必要があり、既存のページの
// リンクと表示は壊さない。リンクの href と表示名を HTML から観測する。
// サイドバー下端には frontend と backend のビルド元（短縮 SHA とブランチ名）を表示し、
// 両方のコミットが取得できていて異なるときだけ不一致を示す。
import test from "node:test";
import assert from "node:assert/strict";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { Sidebar } from "../../src/components/sidebar.tsx";
import { dashboardStateFromProto, type DashboardRun } from "../../src/state/restore-state.ts";
import { createDashboardState } from "../support/fixtures.ts";

type BuildInfo = { commit: string; branch: string };

const FRONTEND_BUILD: BuildInfo = { commit: "a1b2c3d", branch: "takt/20261116-frontend" };
const BACKEND_BUILD: BuildInfo = { commit: "d4e5f6a", branch: "takt/20261116-backend" };

const renderSidebar = (
  builds: { frontend: BuildInfo; backend: BuildInfo } = {
    frontend: FRONTEND_BUILD,
    backend: BACKEND_BUILD,
  },
): string =>
  renderToStaticMarkup(
    createElement(Sidebar, {
      servers: [],
      pendingCount: 0,
      frontendBuild: builds.frontend,
      backendBuild: builds.backend,
    }),
  );

// サイドバー下端（.side-foot）の中身だけを取り出す。ビルド情報がナビや本文ではなく
// 下端に出ていることを、位置の契約として観測する。
const sideFoot = (html: string): string => {
  const match = /<div class="side-foot">([\s\S]*?)<\/div>/.exec(html);
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

// サーバー行（名前を含む li）の中から、状態の点のクラスと状態テキストを取り出す。
const serverRow = (html: string, serverName: string): { dot: string; state: string } => {
  const item = html.split("<li>").find((part) => part.includes(serverName));
  assert.ok(item, `サーバー ${serverName} の行が描画されていません: ${html}`);
  const dot = /<span class="dot ([^"]*)"/.exec(item)?.[1];
  const state = /<span class="srv-state">([^<]*)<\/span>/.exec(item)?.[1];
  assert.ok(dot !== undefined && state !== undefined, `状態の点とテキストが描画されていません: ${item}`);
  return { dot, state };
};

const renderSidebarWithServers = (runs: readonly DashboardRun[]): string =>
  renderToStaticMarkup(
    createElement(Sidebar, {
      servers: createDashboardState().servers,
      pendingCount: 0,
      runs,
      frontendBuild: FRONTEND_BUILD,
      backendBuild: BACKEND_BUILD,
    }),
  );

test("サイドバーのサーバー状態は概要カードと同じ基準と文言で出す", () => {
  const state = createDashboardState();

  const withRuns = renderSidebarWithServers(dashboardStateFromProto(state).runs);
  assert.deepEqual(
    serverRow(withRuns, "Mac mini"),
    { dot: "live", state: "動作中" },
    "接続済みで run があるサーバーはカードと同じく動作中にする",
  );
  assert.deepEqual(
    serverRow(withRuns, "Mac Studio"),
    { dot: "down", state: "応答なし" },
    "未接続のサーバーはカードと同じく応答なしにする",
  );

  const withoutRuns = renderSidebarWithServers([]);
  assert.deepEqual(
    serverRow(withoutRuns, "Mac mini"),
    { dot: "idle", state: "接続中" },
    "接続済みで run が無いサーバーは、カードと同じく接続中にし、動作中と食い違わせない",
  );
  assert.deepEqual(
    serverRow(withoutRuns, "Mac Studio"),
    { dot: "down", state: "応答なし" },
    "未接続のサーバーの表示は run の有無によらず応答なしのままにする",
  );
});

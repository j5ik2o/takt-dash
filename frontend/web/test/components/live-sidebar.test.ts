// 自動更新の状態からサイドバーを描画する契約（issue #15 / #73）。
//
// 初期状態だけでなく、状態ストリームが渡す DashboardState のサーバー一覧と
// 判断待ち件数を表示する。backend のビルド情報は状態から受け取り、frontend の
// ビルド情報はビルド時に埋め込んだ値を prop で受け取って、サイドバー下端に表示する。
import test from "node:test";
import assert from "node:assert/strict";
import { createElement, type ComponentProps } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { BACKEND_BRANCH, BACKEND_COMMIT, createDashboardState } from "../support/fixtures.ts";
import { StateStreamProvider } from "../../src/components/state-provider.tsx";
import { LiveSidebar } from "../../src/components/live-sidebar.tsx";

type BuildInfo = { commit: string; branch: string };

const FRONTEND_BUILD: BuildInfo = { commit: "a1b2c3d", branch: "takt/20261116-frontend" };

const renderSidebar = (
  initialState: ReturnType<typeof createDashboardState>,
  frontendBuild: BuildInfo = FRONTEND_BUILD,
): string => {
  // children は Provider の props の必須項目のため、型付けした props ごと渡す。
  const props: ComponentProps<typeof StateStreamProvider> = {
    initialState,
    children: createElement(LiveSidebar, { frontendBuild }),
  };
  return renderToStaticMarkup(createElement(StateStreamProvider, props));
};

const sideFoot = (html: string): string => {
  const match = /<div class="side-foot">([\s\S]*?)<\/div>/.exec(html);
  assert.ok(match, `サイドバー下端（.side-foot）が描画されていません: ${html}`);
  return match[1]!;
};

test("サイドバーはライブ状態のサーバー一覧と判断待ち件数を表示する", () => {
  const html = renderSidebar(createDashboardState());
  assert.match(html, /Mac mini/);
  assert.match(html, /Mac Studio/);
  assert.match(html, /2件/);
});

test("サイドバーの表示は別のライブ状態のサーバーと件数に対応する", () => {
  const state = createDashboardState();
  const changed = {
    ...state,
    servers: [{ ...state.servers[0]!, name: "別の Mac mini" }],
    pendingDecisions: [],
  };
  const html = renderSidebar(changed);
  assert.match(html, /別の Mac mini/);
  assert.doesNotMatch(html, /2件/);
});

test("サイドバーは最終読み取り時刻・読めた台数・サーバー別の run 件数を表示する", () => {
  const html = renderSidebar(createDashboardState());

  assert.match(html, /最終読み取り時刻/, "最終読み取り時刻をサイドバーに出す");
  assert.match(html, /16:30/, "最終読み取り時刻の値を出す");
  assert.match(html, /2台中1台から読めています/, "読めているサーバーの台数を出す");
  assert.match(html, /run\s*1件/, "run のあるサーバーの件数を出す");
  assert.match(html, /run\s*0件/, "run の無いサーバーの件数を出す");
});

test("最終読み取り時刻が無いときは不明と表示し、例外にしない", () => {
  const state = createDashboardState();
  const html = renderSidebar({ ...state, readAt: undefined });

  assert.match(html, /不明/, "最終読み取り時刻が無いときは不明と出す");
});

test("ライブ状態の backend のビルド情報と frontend のビルド情報をサイドバー下端に表示する", () => {
  const foot = sideFoot(renderSidebar(createDashboardState()));

  assert.match(foot, new RegExp(BACKEND_COMMIT), "backend の短縮 SHA を出す");
  assert.match(foot, new RegExp(BACKEND_BRANCH.replace(/\//g, "\\/")), "backend のブランチ名を出す");
  assert.match(foot, new RegExp(FRONTEND_BUILD.commit), "frontend の短縮 SHA を出す");
  assert.match(foot, /takt\/20261116-frontend/, "frontend のブランチ名を出す");
});

test("backend のビルド情報が状態に無いときは不明として表示する", () => {
  const state = createDashboardState();
  const foot = sideFoot(renderSidebar({ ...state, buildInfo: undefined }));

  assert.match(foot, /unknown/i, "backend のビルド情報が無ければ unknown を出す");
});

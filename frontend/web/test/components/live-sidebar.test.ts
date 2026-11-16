// 自動更新の状態からサイドバーを描画する契約（issue #15）。
//
// 初期状態だけでなく、状態ストリームが渡す DashboardState のサーバー一覧と
// 判断待ち件数を表示する。
import test from "node:test";
import assert from "node:assert/strict";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { createDashboardState } from "../support/fixtures.ts";
import { StateStreamProvider } from "../../src/components/state-provider.tsx";
import { LiveSidebar } from "../../src/components/live-sidebar.tsx";

const renderSidebar = (initialState: ReturnType<typeof createDashboardState>): string =>
  renderToStaticMarkup(
    createElement(StateStreamProvider, { initialState }, createElement(LiveSidebar)),
  );

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

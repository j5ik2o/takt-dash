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

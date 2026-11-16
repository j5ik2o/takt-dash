// 概要と判断待ちの画面が、StateStreamProvider の Context 値を useLiveState 経由で消費する契約と、
// SSE の受信・切断を本番と同じ結線で Context 値へ反映する結線の契約（issue #20 の配線層）。
//
// 本番のハンドラ結線は stateStreamHandlers に一本化されている。ここでは本番と同じ
// stateStreamHandlers → reduceLiveState を通して遷移を検証し、接続状態の LiveOverview／LiveDecisions を
// StateStreamProvider 配下で描画して観測する。切断時に通知が出ることは、内容コンポーネントの
// prop 契約を固定する live-disconnected.test.ts が担保する。StateStreamProvider は初期値を
// initialState から決めるため、SSR では切断状態の Live* を描画しない（未確認として扱う）。
import test from "node:test";
import assert from "node:assert/strict";
import { createElement, type ComponentProps } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { DashboardState } from "../../src/proto/taktdash.ts";
import { createDashboardState } from "../support/fixtures.ts";
import { LiveOverview } from "../../src/components/live-overview.tsx";
import { LiveDecisions } from "../../src/components/live-decisions.tsx";
import {
  reduceLiveState,
  stateStreamHandlers,
  StateStreamProvider,
  type StateStreamContextValue,
} from "../../src/components/state-provider.tsx";
import { dashboardStateFromProto } from "../../src/state/restore-state.ts";
import { DomainPrimitiveError } from "../../src/state/domain-primitives.ts";

// 本番と同じ結線（stateStreamHandlers → reduceLiveState）を通して、現在の Context 値を
// 進めるハンドラを作る。テスト内の値はこの関数だけで更新し、結線の取り違えを検出する。
const drive = (initial: StateStreamContextValue) => {
  const holder: { current: StateStreamContextValue } = { current: initial };
  const handlers = stateStreamHandlers((event) => {
    holder.current = reduceLiveState(holder.current, event);
  });
  return { holder, handlers };
};

// 本番の唯一の入口（app/layout.tsx）と同じ形で StateStreamProvider を描画する。
// initialState だけを渡し、追加の供給口は使わない。
const renderLive = (page: typeof LiveOverview | typeof LiveDecisions, state: DashboardState): string => {
  const props = {
    initialState: state,
    children: createElement(page),
  } satisfies ComponentProps<typeof StateStreamProvider>;
  return renderToStaticMarkup(createElement(StateStreamProvider, props));
};

const connected = (): StateStreamContextValue => ({
  state: dashboardStateFromProto(createDashboardState()),
  disconnected: false,
});

test("SSE の受信で切断フラグが消え、切断で立ち、切断中も直前の state を保持する", () => {
  const initial = connected();
  const { holder, handlers } = drive(initial);

  handlers.onError();
  assert.equal(holder.current.disconnected, true, "エラーで切断フラグが立つ");
  assert.equal(holder.current.state, initial.state, "切断中も直前の state を保持する");

  const next = dashboardStateFromProto(createDashboardState());
  handlers.onState(next);
  assert.equal(holder.current.disconnected, false, "受信で切断フラグが消える");
  assert.equal(holder.current.state, next, "受信で state を置き換える");
});

test("概要は Provider の Context が接続中なら useLiveState 経由で通知を描画しない", () => {
  const html = renderLive(LiveOverview, createDashboardState());
  assert.doesNotMatch(html, /backend に接続できません/);
});

test("概要は Provider の Context が接続中なら最終読み取り時刻のタイルを持たない", () => {
  const html = renderLive(LiveOverview, createDashboardState());
  assert.doesNotMatch(html, /最終読み取り時刻/, "最終読み取り時刻のタイルはサイドバーへ移した");
  assert.match(html, /Mac mini/, "サーバーの表示を残す");
});

test("判断待ちは Provider の Context が接続中なら useLiveState 経由で通知を描画しない", () => {
  const html = renderLive(LiveDecisions, createDashboardState());
  assert.doesNotMatch(html, /backend に接続できません/);
});

test("判断待ちは Provider の Context が接続中でも最後に読めた状態の表示を残す", () => {
  const html = renderLive(LiveDecisions, createDashboardState());
  assert.match(html, /判断待ち/, "最後に読めた判断待ちの表示を残す");
  // 見出しや脚注にも「判断待ち」が含まれるため、状態に由来する一覧の値で
  // Context の state が一覧へ届いていることを確かめる。
  assert.match(html, /corporate-site/, "最後に読めた判断待ちの対象プロジェクトを残します");
  assert.match(html, /プルリクエスト #418/, "最後に読めた判断待ちの本文を残します");
});

test("RSC の初期状態は検証を通してから Context に載せ、必須の値が空なら描画しない", () => {
  const invalid = DashboardState.create({ servers: [{ serverId: "" }] });
  const props = {
    initialState: invalid,
    children: createElement(LiveOverview),
  } satisfies ComponentProps<typeof StateStreamProvider>;

  assert.throws(
    () => renderToStaticMarkup(createElement(StateStreamProvider, props)),
    (error) => error instanceof DomainPrimitiveError && error.stage === "size",
  );
});

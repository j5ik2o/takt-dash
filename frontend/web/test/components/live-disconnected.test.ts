// 概要と判断待ちとゴール一覧の内容コンポーネントが、渡された切断フラグに応じて通知を描画する契約（issue #20 の prop 契約）。
//
// ここでは内容コンポーネントへ disconnected を prop として直接渡し、通知が出ることと、
// 切断時も最後に読めた状態の表示が残ることを観測する。Provider から useLiveState を経由する
// 配線層の観測は live-provider-wiring.test.ts が担う。docs/requirements.md:47 のキャッシュ表示を
// 消さないことも確かめる。
import test from "node:test";
import assert from "node:assert/strict";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { createDashboardState, NOW } from "../support/fixtures.ts";
import { buildOverviewView } from "../../src/state/dashboard.ts";
import { dashboardStateFromProto } from "../../src/state/restore-state.ts";
import { OverviewContent } from "../../src/components/overview-content.tsx";
import { DecisionsContent } from "../../src/components/decisions-content.tsx";
import { GoalsContent } from "../../src/components/goals-content.tsx";
import { BACKEND_UNAVAILABLE_MESSAGE } from "../../src/components/backend-unavailable-notice.tsx";

const renderOverview = (disconnected: boolean): string =>
  renderToStaticMarkup(
    createElement(OverviewContent, { view: buildOverviewView(createDashboardState(), NOW), disconnected }),
  );

const renderDecisions = (disconnected: boolean): string =>
  renderToStaticMarkup(
    createElement(DecisionsContent, { state: createDashboardState(), now: NOW, disconnected }),
  );

// GoalsContent は内部型の状態を受けるため、proto のフィクスチャを変換して渡す。
const renderGoals = (disconnected: boolean): string =>
  renderToStaticMarkup(
    createElement(GoalsContent, { state: dashboardStateFromProto(createDashboardState()), disconnected }),
  );

test("概要は SSE の切断中に backend 不通の通知を描画する", () => {
  const html = renderOverview(true);
  assert.match(html, /backend に接続できません/);
  assert.ok(html.includes(BACKEND_UNAVAILABLE_MESSAGE), "既存の不通の文言を使います");
});

test("概要は接続中に backend 不通の通知を描画しない", () => {
  const html = renderOverview(false);
  assert.doesNotMatch(html, /backend に接続できません/);
});

test("概要は切断中も最後に読めた状態と時刻の表示を残す", () => {
  const html = renderOverview(true);
  assert.match(html, /最終読み取り時刻/, "キャッシュとして最後に読めた時刻の表示を残します");
  assert.match(html, /Mac mini/, "最後に読めたサーバーの表示を残します");
});

test("判断待ちは SSE の切断中に backend 不通の通知を描画する", () => {
  const html = renderDecisions(true);
  assert.match(html, /backend に接続できません/);
  assert.ok(html.includes(BACKEND_UNAVAILABLE_MESSAGE), "既存の不通の文言を使います");
});

test("判断待ちは接続中に backend 不通の通知を描画しない", () => {
  const html = renderDecisions(false);
  assert.doesNotMatch(html, /backend に接続できません/);
});

test("判断待ちは切断中も最後に読めた状態の表示を残す", () => {
  const html = renderDecisions(true);
  assert.match(html, /判断待ち/, "最後に読めた判断待ちの表示を残します");
  // 見出しや脚注にも「判断待ち」が含まれるため、状態に由来する一覧の値で
  // キャッシュ表示が実際に描画されていることを確かめる。
  assert.match(html, /corporate-site/, "最後に読めた判断待ちの対象プロジェクトを残します");
  assert.match(html, /プルリクエスト #418/, "最後に読めた判断待ちの本文を残します");
});

test("概要は一度接続できた後の切断では設定の確認を促さない", () => {
  const html = renderToStaticMarkup(
    createElement(OverviewContent, {
      view: buildOverviewView(createDashboardState(), NOW),
      disconnected: true,
      unavailableSituation: { failure: "unreachable", respondedBefore: true, durationMs: 5_000 },
    }),
  );
  assert.match(html, /backend に接続できません/);
  assert.doesNotMatch(html, /TAKTDASH_BACKEND_ADDR/);
});

test("概要は切断が60秒を超えたら起動の確認を促す", () => {
  const html = renderToStaticMarkup(
    createElement(OverviewContent, {
      view: buildOverviewView(createDashboardState(), NOW),
      disconnected: true,
      unavailableSituation: { failure: "unreachable", respondedBefore: true, durationMs: 120_000 },
    }),
  );
  assert.match(html, /backend の起動を確認してください/);
  assert.doesNotMatch(html, /TAKTDASH_BACKEND_ADDR/);
});

test("判断待ちは切断中に取得した時刻つきで最後の内容を残す", () => {
  const state = createDashboardState();
  const html = renderToStaticMarkup(
    createElement(DecisionsContent, {
      state,
      now: NOW,
      disconnected: true,
      readAt: state.readAt,
      unavailableSituation: { failure: "unreachable", respondedBefore: true, durationMs: 5_000 },
    }),
  );
  assert.match(html, /最終読み取り時刻/, "最後に取れた内容の取得時刻を表示します");
  assert.doesNotMatch(html, /TAKTDASH_BACKEND_ADDR/);
});


test("概要は1件も読めていない切断中に、現在時刻を最終読み取り時刻として表示しない", () => {
  const state = { ...createDashboardState(), readAt: undefined };
  const html = renderToStaticMarkup(
    createElement(OverviewContent, {
      view: buildOverviewView(state, NOW),
      disconnected: true,
      unavailableSituation: { failure: "unreachable", respondedBefore: true, durationMs: 5_000 },
    }),
  );
  assert.doesNotMatch(html, /最終読み取り時刻/, "読めた時刻が無い場合は時刻の表示を作りません");
  assert.match(html, /takt-mcp からまだ読めていません/, "サーバーカードと同じ未読の文言を出します");
});

test("概要は切断中に通知をちょうど1回だけ描画する", () => {
  const html = renderOverview(true);
  assert.equal(
    html.split(BACKEND_UNAVAILABLE_MESSAGE).length - 1,
    1,
    "通知の表示元は内容の1か所だけにする",
  );
});

test("判断待ちは切断中に通知をちょうど1回だけ描画する", () => {
  const html = renderDecisions(true);
  assert.equal(
    html.split(BACKEND_UNAVAILABLE_MESSAGE).length - 1,
    1,
    "通知の表示元は内容の1か所だけにする",
  );
});

test("ゴール一覧は SSE の切断中に backend 不通の通知を描画する", () => {
  const html = renderGoals(true);
  assert.match(html, /backend に接続できません/);
  assert.ok(html.includes(BACKEND_UNAVAILABLE_MESSAGE), "既存の不通の文言を使います");
});

test("ゴール一覧は接続中に backend 不通の通知を描画しない", () => {
  const html = renderGoals(false);
  assert.doesNotMatch(html, /backend に接続できません/);
});

test("ゴール一覧は切断中も最後に読めた状態の表示を残す", () => {
  const html = renderGoals(true);
  assert.match(html, /採用ページのレイアウトを直す/, "最後に読めたゴールの表示を残します");
});

test("ゴール一覧は切断中に通知をちょうど1回だけ描画する", () => {
  const html = renderGoals(true);
  assert.equal(
    html.split(BACKEND_UNAVAILABLE_MESSAGE).length - 1,
    1,
    "通知の表示元は内容の1か所だけにする",
  );
});

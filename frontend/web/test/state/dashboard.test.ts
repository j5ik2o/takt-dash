// 表示用の変換（C4/C7/C9）の契約。
//
// 待ち時間順・滞留・run の状態は、画面が使う観測可能な値としてここで固定する。
// 閾値は proto の AnomalyThresholds から取り、240/10 を実装側へ直書きさせない。
import test from "node:test";
import assert from "node:assert/strict";
import { PendingDecisionType, type PendingDecision } from "../../src/proto/taktdash.ts";
import { createDashboardState, NOW } from "../support/fixtures.ts";
import {
  buildOverviewView,
  DEFAULT_ANOMALY_THRESHOLDS,
  resolveThresholds,
  sortPendingDecisions,
  summarizePendingDecisions,
  toRunView,
} from "../../src/state/dashboard.ts";

test("判断待ちは待ち時間の長い順に並ぶ", () => {
  const sorted = sortPendingDecisions(createDashboardState().pendingDecisions, NOW);
  assert.deepEqual(
    sorted.map((item: PendingDecision) => item.type),
    [PendingDecisionType.PENDING_DECISION_TYPE_AWAITING_MERGE, PendingDecisionType.PENDING_DECISION_TYPE_INQUIRY],
  );
  assert.equal(sorted[0]?.waitMinutes, 300);
});

test("待ち時間が閾値以上なら滞留として扱い、閾値未満は滞留にしない", () => {
  const thresholds = { pendingDecisionStaleMinutes: 240, runLogSilentMinutes: 10 };
  const state = createDashboardState();
  const atThreshold = sortPendingDecisions(
    [{ ...state.pendingDecisions[0]!, createdAt: new Date(NOW.getTime() - 240 * 60_000) }],
    NOW,
  );
  const justUnder = sortPendingDecisions(
    [{ ...state.pendingDecisions[0]!, createdAt: new Date(NOW.getTime() - 239 * 60_000) }],
    NOW,
  );
  assert.equal(atThreshold[0]?.stale, true);
  assert.equal(justUnder[0]?.stale, false);
  assert.equal(thresholds.pendingDecisionStaleMinutes, 240);
});

test("判断待ちの件数を問い合わせとマージ待ちに分けて数える", () => {
  const counts = summarizePendingDecisions(sortPendingDecisions(createDashboardState().pendingDecisions, NOW));
  assert.equal(counts.total, 2);
  assert.equal(counts.inquiries, 1);
  assert.equal(counts.awaitingMerge, 1);
});

test("run の状態はサーバー応答とログ無音と実行状態から決まる", () => {
  const state = createDashboardState();
  const serverById = new Map(state.servers.map((server) => [server.serverId, server]));

  const live = toRunView(state.runs[0]!, serverById, state.thresholds!, NOW);
  assert.equal(live.connected, true);
  assert.equal(live.label, "実行中");

  const disconnected = toRunView(
    { ...state.runs[0]!, serverId: "down" },
    serverById,
    state.thresholds!,
    NOW,
  );
  assert.equal(disconnected.connected, false);
  assert.match(disconnected.label, /時点/);
});

test("概要の集計は proto の閾値をそのまま表示用に運ぶ", () => {
  const overview = buildOverviewView(createDashboardState(), NOW);
  assert.equal(overview.thresholds.pendingDecisionStaleMinutes, 240);
  assert.equal(overview.thresholds.runLogSilentMinutes, 10);
  assert.equal(overview.runningRuns, 1);
  assert.equal(overview.pendingCounts.total, 2);
  assert.equal(overview.servers.length, 2);
});

test("概要の集計は閾値の変更に追随し、240/10 を決め打ちしない", () => {
  const state = createDashboardState();
  const changed = {
    ...state,
    thresholds: { pendingDecisionStaleMinutes: 60, runLogSilentMinutes: 5 },
  };
  const overview = buildOverviewView(changed, NOW);
  assert.equal(overview.thresholds.pendingDecisionStaleMinutes, 60);
  assert.equal(overview.thresholds.runLogSilentMinutes, 5);
});

test("閾値が無い応答では既定値を1か所で解決し、全画面が同じ値を使う", () => {
  assert.equal(resolveThresholds(undefined), DEFAULT_ANOMALY_THRESHOLDS);
  assert.equal(DEFAULT_ANOMALY_THRESHOLDS.pendingDecisionStaleMinutes, 240);
  assert.equal(DEFAULT_ANOMALY_THRESHOLDS.runLogSilentMinutes, 10);

  const withoutThresholds = { ...createDashboardState(), thresholds: undefined };
  const overview = buildOverviewView(withoutThresholds, NOW);
  assert.equal(overview.thresholds.pendingDecisionStaleMinutes, 240);
  assert.equal(overview.thresholds.runLogSilentMinutes, 10);
});

import test from "node:test";
import assert from "node:assert/strict";
import { buildOverviewView, resolveThresholds, toRunView } from "../../src/state/dashboard.ts";
import { DashboardState } from "../../src/proto/taktdash.ts";
import { createHealthyDashboardState, NOW } from "../support/fixtures.ts";

test("未登録先の run はログの鮮度によらず接続未確認として表示する", () => {
  const state = createHealthyDashboardState();
  const run = state.runs[0];
  assert.ok(run);
  for (const lastLogAt of [NOW, new Date(NOW.getTime() - 20 * 60_000)]) {
    const input = { ...run, lastLogAt };
    const view = toRunView(input, new Map(), resolveThresholds(state.thresholds), NOW);
    assert.equal(view.connected, false);
    assert.equal(view.state, "idle");
    assert.match(view.label, /接続.*未確認/);
    assert.equal(view.run.runSlug, run.runSlug);
    assert.equal(view.run.status, run.status);
  }
});

test("未登録先の run は無音の run として集計しない", () => {
  const base = createHealthyDashboardState();
  const state = DashboardState.create({
    ...base,
    servers: [],
    runs: base.runs.map((run) => ({ ...run, logSilent: true })),
  });
  const view = buildOverviewView(state, NOW, true);
  assert.equal(view.runs.length, 1);
  assert.equal(view.runningRuns, 1);
  assert.equal(view.silentRunCount, 0);
});

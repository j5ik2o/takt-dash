// 新しいログが届いた run 行の1秒の発光を決める契約。
//
// SSE で届く lastLogAt の前進だけを「新しいログ」として扱い、初回観測では光らせない。
// タイマーの配線は client コンポーネント側の責務なので、ここでは純粋関数の判定だけを固定する。
import test from "node:test";
import assert from "node:assert/strict";
import type { Run } from "../../src/proto/taktdash.ts";
import { createDashboardState } from "../support/fixtures.ts";
import { FLASH_MS, initialLogSeen, nextLogSeen } from "../../src/state/run-log-flash.ts";

const baseRun = (overrides: Partial<Run> = {}): Run => ({
  ...createDashboardState().runs[0]!,
  ...overrides,
});

test("発光は1秒続く", () => {
  assert.equal(FLASH_MS, 1_000);
});

test("初回観測ではどの run も光らない", () => {
  const runs = [baseRun()];
  const seen = initialLogSeen(runs);

  const { flashed } = nextLogSeen(seen, runs);

  assert.deepEqual(flashed, [], "マウント時の基準を作った直後は光らせない");
});

test("最後のログが前進した run だけを光らせる", () => {
  const before = baseRun({ runSlug: "run-aaa", lastLogAt: new Date("2026-11-13T07:29:00Z") });
  const unchanged = baseRun({ runSlug: "run-bbb", lastLogAt: new Date("2026-11-13T07:29:00Z") });
  const seen = initialLogSeen([before, unchanged]);
  const advanced = baseRun({ runSlug: "run-aaa", lastLogAt: new Date("2026-11-13T07:30:00Z") });

  const { flashed, seen: next } = nextLogSeen(seen, [advanced, unchanged]);

  assert.deepEqual(flashed, ["run-aaa"], "前進した run だけを光らせる");
  assert.equal(next["run-aaa"], Date.parse("2026-11-13T07:30:00Z"), "比較の基準を今回値へ更新する");
});

test("前進が無い同じ状態を再び渡しても光らせない", () => {
  const run = baseRun({ lastLogAt: new Date("2026-11-13T07:30:00Z") });
  const first = nextLogSeen(initialLogSeen([run]), [run]);
  const second = nextLogSeen(first.seen, [run]);

  assert.deepEqual(first.flashed, [], "初回基準の直後は光らせない");
  assert.deepEqual(second.flashed, [], "同じ値のままなら光らせない");
});

test("新しく現れた run はログがあれば光らせ、ログが無ければ光らせない", () => {
  const existing = baseRun({ runSlug: "run-aaa", lastLogAt: new Date("2026-11-13T07:29:00Z") });
  const withLog = baseRun({ runSlug: "run-bbb", lastLogAt: new Date("2026-11-13T07:30:00Z") });
  const withoutLog = baseRun({ runSlug: "run-ccc", lastLogAt: undefined });
  const seen = initialLogSeen([existing]);

  const { flashed } = nextLogSeen(seen, [existing, withLog, withoutLog]);

  assert.deepEqual(flashed, ["run-bbb"], "ログのある新しい run だけを光らせる");
});

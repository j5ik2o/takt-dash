// ゴールの状態バッジの契約（issue #78）。
//
// ゴールの状態は Goal.status と execution_status の組み合わせで決まる。実行状態を
// 先に評価すると、完了していても実行状態が ACTIVE のゴールに「稼働中」と出す。
// バッジは終端（中止・完了）を先に確定し、残りを一時停止・マージ待ち・進行中へ
// 分ける。1か所の対応表が5種類のラベルと色を返すことを観測する。
import test from "node:test";
import assert from "node:assert/strict";
import { goalBadge, type GoalBadgeKind, type GoalBadgeTone } from "../../src/state/goal-badge.ts";
import { EXECUTION_STATUS, GOAL_STATUS } from "../../src/state/proto-values.ts";

const badgeFor = (status: number, executionStatus: number) =>
  goalBadge({ status, executionStatus });

test("完了したゴールは実行状態が ACTIVE でも完了のバッジにし、稼働中と出さない", () => {
  const badge = badgeFor(GOAL_STATUS.COMPLETED, EXECUTION_STATUS.ACTIVE);

  assert.deepEqual(badge, { kind: "completed", label: "完了", tone: "idle" });
  assert.notEqual(badge.label, "稼働中");
});

test("中止したゴールは中止のバッジにし、完了より中止を優先する", () => {
  assert.deepEqual(badgeFor(GOAL_STATUS.CREATED, EXECUTION_STATUS.ABORTED), {
    kind: "aborted",
    label: "中止",
    tone: "warn",
  });
  assert.deepEqual(badgeFor(GOAL_STATUS.COMPLETED, EXECUTION_STATUS.ABORTED), {
    kind: "aborted",
    label: "中止",
    tone: "warn",
  });
});

test("一時停止したゴールは実行状態とライフサイクルが同時でも一時停止を優先する", () => {
  assert.deepEqual(badgeFor(GOAL_STATUS.CREATED, EXECUTION_STATUS.PAUSED), {
    kind: "paused",
    label: "一時停止",
    tone: "idle",
  });
  assert.deepEqual(badgeFor(GOAL_STATUS.AWAITING_MERGE, EXECUTION_STATUS.PAUSED), {
    kind: "paused",
    label: "一時停止",
    tone: "idle",
  });
});

test("人のマージ待ちのゴールはマージ待ちのバッジにする", () => {
  assert.deepEqual(badgeFor(GOAL_STATUS.AWAITING_MERGE, EXECUTION_STATUS.ACTIVE), {
    kind: "awaiting_merge",
    label: "マージ待ち",
    tone: "warn",
  });
});

test("作成済みで実行中のゴールは進行中のバッジにする", () => {
  assert.deepEqual(badgeFor(GOAL_STATUS.CREATED, EXECUTION_STATUS.ACTIVE), {
    kind: "active",
    label: "進行中",
    tone: "live",
  });
});

test("Goal.status と execution_status の全組み合わせで5種類のラベルと色のどれかを返す", () => {
  const statuses = [GOAL_STATUS.CREATED, GOAL_STATUS.AWAITING_MERGE, GOAL_STATUS.COMPLETED];
  const executionStatuses = [
    EXECUTION_STATUS.ACTIVE,
    EXECUTION_STATUS.PAUSED,
    EXECUTION_STATUS.ABORTED,
  ];
  const kinds = new Set<GoalBadgeKind>(["active", "awaiting_merge", "paused", "completed", "aborted"]);
  const tones = new Set<GoalBadgeTone>(["live", "warn", "idle"]);

  for (const status of statuses) {
    for (const executionStatus of executionStatuses) {
      const badge = badgeFor(status, executionStatus);
      assert.ok(kinds.has(badge.kind), `status=${status} executionStatus=${executionStatus} の kind が5種類の外`);
      assert.ok(tones.has(badge.tone), `status=${status} executionStatus=${executionStatus} の tone が3種類の外`);
      assert.doesNotMatch(badge.label, /稼働中/, `status=${status} executionStatus=${executionStatus}`);
    }
  }
});

// ゴールの状態バッジの契約（issue #78、issue #189）。
//
// ゴールの状態は Goal.status と execution_status の組み合わせで決まる。実行状態を
// 先に評価すると、完了していても実行状態が ACTIVE のゴールに「稼働中」と出す。
// バッジは終端（中止・完了）を先に確定し、残りを一時停止・マージ待ちへ分ける。
// 最後に残る作成済みのゴールは、タスクが登録されていなければ「未着手」、実行中の
// タスクがあれば「実行中」、どちらでもなければ「進行中」とし、実際に run が
// 動いているかどうかを文言で区別する（issue #189）。
import test from "node:test";
import assert from "node:assert/strict";
import { goalBadge, goalStateSentence, type GoalBadgeKind, type GoalBadgeTone } from "../../src/state/goal-badge.ts";
import { EXECUTION_STATUS, GOAL_STATUS } from "../../src/state/proto-values.ts";
import type { TaskStatusCounts } from "../../src/proto/taktdash.ts";

const counts = (overrides: Partial<TaskStatusCounts> = {}): TaskStatusCounts => ({
  pending: 0,
  running: 0,
  completed: 0,
  failed: 0,
  exceeded: 0,
  prFailed: 0,
  ...overrides,
});

const badgeFor = (status: number, executionStatus: number, taskCounts?: TaskStatusCounts) =>
  goalBadge({ status, executionStatus, taskCounts });

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

test("タスクがあり実行中のタスクが無いゴールは進行中のバッジにする", () => {
  assert.deepEqual(badgeFor(GOAL_STATUS.CREATED, EXECUTION_STATUS.ACTIVE, counts({ pending: 1, completed: 2 })), {
    kind: "active",
    label: "進行中",
    tone: "live",
  });
});

test("実行中のタスクがあるゴールは実行中のバッジにする", () => {
  assert.deepEqual(badgeFor(GOAL_STATUS.CREATED, EXECUTION_STATUS.ACTIVE, counts({ running: 1, completed: 2 })), {
    kind: "running",
    label: "実行中",
    tone: "live",
  });
});

test("タスクが登録されていないゴールは未着手のバッジにする", () => {
  assert.deepEqual(badgeFor(GOAL_STATUS.CREATED, EXECUTION_STATUS.ACTIVE, undefined), {
    kind: "untouched",
    label: "未着手",
    tone: "idle",
  });
  assert.deepEqual(badgeFor(GOAL_STATUS.CREATED, EXECUTION_STATUS.ACTIVE, counts()), {
    kind: "untouched",
    label: "未着手",
    tone: "idle",
  });
});

test("タスクが登録されていなくても、完了と一時停止とマージ待ちを先に確定する", () => {
  assert.deepEqual(badgeFor(GOAL_STATUS.COMPLETED, EXECUTION_STATUS.ACTIVE, undefined), {
    kind: "completed",
    label: "完了",
    tone: "idle",
  });
  assert.deepEqual(badgeFor(GOAL_STATUS.CREATED, EXECUTION_STATUS.PAUSED, undefined), {
    kind: "paused",
    label: "一時停止",
    tone: "idle",
  });
  assert.deepEqual(badgeFor(GOAL_STATUS.AWAITING_MERGE, EXECUTION_STATUS.ACTIVE, undefined), {
    kind: "awaiting_merge",
    label: "マージ待ち",
    tone: "warn",
  });
});

test("Goal.status と execution_status の全組み合わせで決まったラベルと色のどれかを返す", () => {
  const statuses = [GOAL_STATUS.CREATED, GOAL_STATUS.AWAITING_MERGE, GOAL_STATUS.COMPLETED];
  const executionStatuses = [
    EXECUTION_STATUS.ACTIVE,
    EXECUTION_STATUS.PAUSED,
    EXECUTION_STATUS.ABORTED,
  ];
  const kinds = new Set<GoalBadgeKind>([
    "active",
    "running",
    "untouched",
    "awaiting_merge",
    "paused",
    "completed",
    "aborted",
  ]);
  const tones = new Set<GoalBadgeTone>(["live", "warn", "idle"]);

  for (const status of statuses) {
    for (const executionStatus of executionStatuses) {
      const badge = badgeFor(status, executionStatus);
      assert.ok(kinds.has(badge.kind), `status=${status} executionStatus=${executionStatus} の kind が対応表の外`);
      assert.ok(tones.has(badge.tone), `status=${status} executionStatus=${executionStatus} の tone が3種類の外`);
      assert.doesNotMatch(badge.label, /稼働中/, `status=${status} executionStatus=${executionStatus}`);
    }
  }
});

const sentenceFor = (status: number, executionStatus: number, taskCounts?: TaskStatusCounts) =>
  goalStateSentence({ status, executionStatus, taskCounts });

test("完了したゴールの文面は完了だけを述べ、実行状態が ACTIVE でも稼働中と出さない", () => {
  assert.equal(sentenceFor(GOAL_STATUS.COMPLETED, EXECUTION_STATUS.ACTIVE), "ゴールは完了しています。");
  assert.equal(
    sentenceFor(GOAL_STATUS.COMPLETED, EXECUTION_STATUS.ACTIVE, counts({ running: 2, completed: 5 })),
    "ゴールは完了しています。実行中のタスクが 2 件残っています。",
  );
});

test("中止したゴールの文面はゴール自体の中止として述べる", () => {
  assert.equal(sentenceFor(GOAL_STATUS.CREATED, EXECUTION_STATUS.ABORTED), "ゴールは中止されました。");
  assert.equal(sentenceFor(GOAL_STATUS.COMPLETED, EXECUTION_STATUS.ABORTED), "ゴールは中止されました。");
});

test("一時停止の文面は状態と、実行が一時停止していることを分けて述べる", () => {
  assert.equal(
    sentenceFor(GOAL_STATUS.CREATED, EXECUTION_STATUS.PAUSED),
    "ゴールは作成済みで、実行は一時停止しています。",
  );
  assert.equal(
    sentenceFor(GOAL_STATUS.AWAITING_MERGE, EXECUTION_STATUS.PAUSED),
    "ゴールは人のマージ待ちで、実行は一時停止しています。",
  );
});

test("人のマージ待ちの文面はマージ待ちだけを述べ、稼働中と出さない", () => {
  assert.equal(sentenceFor(GOAL_STATUS.AWAITING_MERGE, EXECUTION_STATUS.ACTIVE), "ゴールは人のマージ待ちです。");
});

test("作成済みの文面はタスクの実態で述べ、実行状態が ACTIVE でも稼働中と出さない", () => {
  assert.equal(
    sentenceFor(GOAL_STATUS.CREATED, EXECUTION_STATUS.ACTIVE),
    "ゴールは作成済みで、タスクはまだ登録されていません。",
  );
  assert.equal(
    sentenceFor(GOAL_STATUS.CREATED, EXECUTION_STATUS.ACTIVE, counts({ running: 3, pending: 1 })),
    "ゴールは作成済みで、実行中のタスクが 3 件あります。",
  );
  assert.equal(
    sentenceFor(GOAL_STATUS.CREATED, EXECUTION_STATUS.ACTIVE, counts({ pending: 1, completed: 2 })),
    "ゴールは作成済みで、タスクは進行中です。",
  );
});

test("Goal.status と execution_status の全組み合わせで文面に稼働中を含めない", () => {
  const statuses = [GOAL_STATUS.CREATED, GOAL_STATUS.AWAITING_MERGE, GOAL_STATUS.COMPLETED];
  const executionStatuses = [EXECUTION_STATUS.ACTIVE, EXECUTION_STATUS.PAUSED, EXECUTION_STATUS.ABORTED];
  for (const status of statuses) {
    for (const executionStatus of executionStatuses) {
      assert.doesNotMatch(
        sentenceFor(status, executionStatus),
        /稼働中/,
        `status=${status} executionStatus=${executionStatus}`,
      );
    }
  }
});

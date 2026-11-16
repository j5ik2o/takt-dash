// アクティビティの接続イベント集約（AC71-C7）の契約。
//
// 入力は backend が返す最新順の配列。隣接し、両方が接続種別で、同じサーバー・
// プロジェクトのイベントだけを1つのまとまりにする。件数と期間（最古〜最新）を
// 出せる形にし、非隣接・別プロジェクト・非接続種別はまとめない。
import test from "node:test";
import assert from "node:assert/strict";
import { ActivityKind, type ActivityEvent } from "../../src/proto/taktdash.ts";
import { groupActivityEvents } from "../../src/state/activity.ts";
import { NOW } from "../support/fixtures.ts";

const minutesAgo = (minutes: number): Date => new Date(NOW.getTime() - minutes * 60_000);

const connectionEvent = (
  kind: ActivityKind,
  project: string,
  minutes: number,
  serverId = "mini",
): ActivityEvent => ({
  occurredAt: minutesAgo(minutes),
  serverId,
  project,
  kind,
  body: "",
});

test("同じプロジェクトの隣接する接続イベントを件数と最古・最新の1件にまとめる", () => {
  const events: ActivityEvent[] = [
    connectionEvent(ActivityKind.ACTIVITY_KIND_CONNECTION_RESTORED, "esa", 5),
    connectionEvent(ActivityKind.ACTIVITY_KIND_CONNECTION_LOST, "esa", 10),
    connectionEvent(ActivityKind.ACTIVITY_KIND_CONNECTION_LOST, "esa", 15),
  ];

  const groups = groupActivityEvents(events);

  assert.equal(groups.length, 1, "隣接する同一プロジェクトの接続イベントは1行にまとまる");
  assert.equal(groups[0]!.count, 3);
  assert.deepEqual(groups[0]!.events, events);
  assert.equal(groups[0]!.newest.occurredAt?.getTime(), minutesAgo(5).getTime());
  assert.equal(groups[0]!.oldest.occurredAt?.getTime(), minutesAgo(15).getTime());
});

test("別プロジェクトの接続イベントはまとめない", () => {
  const events: ActivityEvent[] = [
    connectionEvent(ActivityKind.ACTIVITY_KIND_CONNECTION_LOST, "esa", 5),
    connectionEvent(ActivityKind.ACTIVITY_KIND_CONNECTION_LOST, "corporate-site", 10),
  ];

  const groups = groupActivityEvents(events);

  assert.equal(groups.length, 2);
  assert.deepEqual(groups.map((group) => group.count), [1, 1]);
});

test("同じプロジェクト名でもサーバーが違う接続イベントはまとめない", () => {
  const events: ActivityEvent[] = [
    connectionEvent(ActivityKind.ACTIVITY_KIND_CONNECTION_LOST, "esa", 5, "mini"),
    connectionEvent(ActivityKind.ACTIVITY_KIND_CONNECTION_LOST, "esa", 10, "studio"),
  ];

  const groups = groupActivityEvents(events);

  assert.equal(groups.length, 2);
  assert.deepEqual(groups.map((group) => group.count), [1, 1]);
});

test("非接続のイベントを挟んだ同一プロジェクトの接続イベントはまとめない", () => {
  const events: ActivityEvent[] = [
    connectionEvent(ActivityKind.ACTIVITY_KIND_CONNECTION_LOST, "esa", 5),
    {
      occurredAt: minutesAgo(10),
      serverId: "mini",
      project: "esa",
      kind: ActivityKind.ACTIVITY_KIND_INQUIRY,
      body: "問い合わせが届きました。",
    },
    connectionEvent(ActivityKind.ACTIVITY_KIND_CONNECTION_LOST, "esa", 15),
  ];

  const groups = groupActivityEvents(events);

  assert.equal(groups.length, 3, "間に別の出来事があれば接続イベントをまとめない");
  assert.deepEqual(groups.map((group) => group.count), [1, 1, 1]);
});

test("非接続のイベントは1件ずつのまとまりとして返す", () => {
  const events: ActivityEvent[] = [
    {
      occurredAt: minutesAgo(5),
      serverId: "mini",
      project: "corporate-site",
      kind: ActivityKind.ACTIVITY_KIND_AWAITING_MERGE,
      body: "人のマージ待ちになりました。",
    },
    {
      occurredAt: minutesAgo(10),
      serverId: "mini",
      project: "esa",
      kind: ActivityKind.ACTIVITY_KIND_INQUIRY,
      body: "問い合わせが届きました。",
    },
  ];

  const groups = groupActivityEvents(events);

  assert.equal(groups.length, 2);
  for (const group of groups) {
    assert.equal(group.count, 1);
    assert.equal(group.events.length, 1);
  }
});

// issue #40 の PR 1a：proto に足す更新間隔の型の生成物（ts-proto）の契約。
//
// 画面の選択肢は生成物の RefreshRate から作り、数値を TypeScript に書き写さない（設計 D5）。
// 要求の欄と applied の JSON の名前・既定値・省略は、BFF の SSE（DashboardState.toJSON）と
// 画面の復元（restoreDashboardState）がそのまま頼る形なので、ここで固定する。
//
// 生成物に無い名前を名前つきで import するとファイル全体が読み込みで落ちるため、
// 名前空間で読み、テストごとに確かめる。
import test from "node:test";
import assert from "node:assert/strict";
import * as proto from "../../src/proto/taktdash.ts";

test("RefreshRate は 0 と 4 つの選択肢をミリ秒の値で持ち、未知の値は UNRECOGNIZED になる", () => {
  const { RefreshRate } = proto;
  assert.equal(RefreshRate.REFRESH_RATE_UNSPECIFIED, 0);
  assert.equal(RefreshRate.REFRESH_RATE_ONE_SECOND, 1000);
  assert.equal(RefreshRate.REFRESH_RATE_TWO_SECONDS, 2000);
  assert.equal(RefreshRate.REFRESH_RATE_FIVE_SECONDS, 5000);
  assert.equal(RefreshRate.REFRESH_RATE_TEN_SECONDS, 10000);
  assert.equal(RefreshRate.UNRECOGNIZED, -1);

  // 画面の選択肢は「0 より大きい数値」で取り出す（設計 D5）。UNSPECIFIED と UNRECOGNIZED は入らない。
  const choices = Object.values(RefreshRate).filter(
    (value): value is number => typeof value === "number" && value > 0,
  );
  assert.deepEqual(choices, [1000, 2000, 5000, 10000]);
});

test("RefreshRate の JSON 表現は enum の名前で、数値からも名前からも読める", () => {
  const { RefreshRate, refreshRateFromJSON, refreshRateToJSON } = proto;
  const names: Array<[number, string]> = [
    [0, "REFRESH_RATE_UNSPECIFIED"],
    [1000, "REFRESH_RATE_ONE_SECOND"],
    [2000, "REFRESH_RATE_TWO_SECONDS"],
    [5000, "REFRESH_RATE_FIVE_SECONDS"],
    [10000, "REFRESH_RATE_TEN_SECONDS"],
  ];
  for (const [millis, name] of names) {
    assert.equal(refreshRateToJSON(millis), name);
    assert.equal(refreshRateFromJSON(name), millis);
    assert.equal(refreshRateFromJSON(millis), millis);
  }
  for (const unknown of [1500, 15000, -5, "ONE_SECOND", "1000"]) {
    assert.equal(refreshRateFromJSON(unknown), RefreshRate.UNRECOGNIZED, `${String(unknown)} は選択肢ではない`);
  }
});

test("WatchStateRequest の既定は要求なし（レート 0・画面は空・試行 0）で、JSON では全部省かれる", () => {
  const { WatchStateRequest } = proto;
  const empty = WatchStateRequest.fromPartial({});

  assert.deepEqual(empty, { refreshRate: 0, screen: "", attempt: 0 });
  assert.deepEqual(WatchStateRequest.create(), empty);
  assert.deepEqual(WatchStateRequest.toJSON(empty), {});
  assert.deepEqual(WatchStateRequest.fromJSON({}), empty);
  // 古い画面・古い BFF が送る空の要求（wire では 0 バイト）は、要求なしとして読める。
  assert.equal(WatchStateRequest.encode(empty).finish().length, 0);
  assert.deepEqual(WatchStateRequest.decode(new Uint8Array()), empty);
});

test("WatchStateRequest の要求の欄は refreshRate・screen・attempt の名前で JSON と wire を往復する", () => {
  const { RefreshRate, WatchStateRequest } = proto;
  const request = WatchStateRequest.fromPartial({
    refreshRate: RefreshRate.REFRESH_RATE_ONE_SECOND,
    screen: "s",
    attempt: 3,
  });

  assert.deepEqual(request, { refreshRate: 1000, screen: "s", attempt: 3 });
  assert.deepEqual(WatchStateRequest.toJSON(request), {
    refreshRate: "REFRESH_RATE_ONE_SECOND",
    screen: "s",
    attempt: 3,
  });
  assert.deepEqual(WatchStateRequest.fromJSON(WatchStateRequest.toJSON(request)), request);
  // 1 番（varint 1000）・2 番（"s"）・3 番（varint 3）。backend の prost と同じバイト列。
  assert.deepEqual(
    [...WatchStateRequest.encode(request).finish()],
    [0x08, 0xe8, 0x07, 0x12, 0x01, 0x73, 0x18, 0x03],
  );
  assert.deepEqual(WatchStateRequest.decode(WatchStateRequest.encode(request).finish()), request);
});

test("DashboardState.appliedRefreshIntervalMs は既定 0 で、0 のとき JSON から省かれる", () => {
  const { DashboardState } = proto;

  assert.equal(DashboardState.fromPartial({}).appliedRefreshIntervalMs, 0);
  assert.equal(DashboardState.create().appliedRefreshIntervalMs, 0);
  assert.equal("appliedRefreshIntervalMs" in (DashboardState.toJSON(DashboardState.create()) as object), false);
  assert.equal(DashboardState.fromJSON({}).appliedRefreshIntervalMs, 0);
  // 古い backend は 9 番を送らないので、空の wire は 0 として読む。
  assert.equal(DashboardState.decode(new Uint8Array()).appliedRefreshIntervalMs, 0);
});

test("DashboardState.appliedRefreshIntervalMs は 9 番の欄で、JSON では appliedRefreshIntervalMs の名前になる", () => {
  const { DashboardState } = proto;
  const state = DashboardState.fromPartial({ appliedRefreshIntervalMs: 1000 });

  assert.equal(state.appliedRefreshIntervalMs, 1000);
  assert.deepEqual(DashboardState.toJSON(state), { appliedRefreshIntervalMs: 1000 });
  assert.equal(DashboardState.fromJSON({ appliedRefreshIntervalMs: 1000 }).appliedRefreshIntervalMs, 1000);
  assert.deepEqual([...DashboardState.encode(state).finish()], [0x48, 0xe8, 0x07]);
  assert.equal(DashboardState.decode(DashboardState.encode(state).finish()).appliedRefreshIntervalMs, 1000);
});

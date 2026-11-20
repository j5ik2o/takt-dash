// 計測基盤の契約（AC72A-1）。
//
// ページ応答を total_ms / rpc_ms / next_ms に分解して1行で記録し、
// RPC ごとの所要時間も1回ごとに記録する。時刻と出力先は注入できる。
import test from "node:test";
import assert from "node:assert/strict";
import { createTimingRecorder, measurePage } from "../../src/telemetry/timing.ts";

const scriptedClock = (values: number[]): (() => number) => {
  let index = 0;
  return () => {
    const value = values[index];
    if (value === undefined) {
      throw new Error(`時計の呼び出しが想定より多くなりました（${index + 1} 回目）。`);
    }
    index += 1;
    return value;
  };
};

test("RPC は 1 回ごとに rpc 名と所要時間をログへ出す", () => {
  const lines: string[] = [];
  const recorder = createTimingRecorder({ log: (line) => lines.push(line) });

  recorder.recordRpc({ rpc: "GetProjects", durationMs: 9.42 });
  recorder.recordRpc({ rpc: "GetActivity", durationMs: 6.5 });

  assert.deepEqual(lines, [
    "[taktDash][timing] scope=rpc rpc=GetProjects duration_ms=9.42",
    "[taktDash][timing] scope=rpc rpc=GetActivity duration_ms=6.50",
  ]);
  assert.ok(Math.abs(recorder.durationMs - 15.92) < 1e-9, "RPC の合計を保持します");
  assert.deepEqual(recorder.rpcs, ["GetProjects", "GetActivity"], "呼ばれた RPC の名前を順に保持します");
});

test("ページは total_ms と gRPC の往復と差分の next_ms を1行で出す", async () => {
  const lines: string[] = [];
  const clock = scriptedClock([1000, 1012]);
  const result = await measurePage(
    "/projects",
    async (timing) => {
      timing.recordRpc({ rpc: "GetOverview", durationMs: 2.5 });
      timing.recordRpc({ rpc: "GetProjects", durationMs: 6 });
      return "描画結果";
    },
    { clock, log: (line) => lines.push(line) },
  );

  assert.equal(result, "描画結果");
  assert.deepEqual(lines, [
    "[taktDash][timing] scope=rpc rpc=GetOverview duration_ms=2.50",
    "[taktDash][timing] scope=rpc rpc=GetProjects duration_ms=6.00",
    "[taktDash][timing] scope=page page=/projects total_ms=12.00 rpc_ms=8.50 next_ms=3.50 rpcs=GetOverview,GetProjects",
  ]);
});

test("ページの計測は読み込みが失敗しても total_ms を出す", async () => {
  const lines: string[] = [];
  const clock = scriptedClock([100, 107]);
  await assert.rejects(
    measurePage(
      "/activity",
      async () => {
        throw new Error("読み込みに失敗しました。");
      },
      { clock, log: (line) => lines.push(line) },
    ),
    /読み込みに失敗しました。/,
  );

  assert.deepEqual(lines, [
    "[taktDash][timing] scope=page page=/activity total_ms=7.00 rpc_ms=0.00 next_ms=7.00 rpcs=",
  ]);
});

test("next_ms は gRPC の往復が全体を超えても負にしない", async () => {
  const lines: string[] = [];
  const clock = scriptedClock([100, 101]);
  await measurePage(
    "/",
    async (timing) => {
      timing.recordRpc({ rpc: "GetOverview", durationMs: 5 });
    },
    { clock, log: (line) => lines.push(line) },
  );

  const pageLine = lines.find((line) => line.includes("scope=page"));
  assert.ok(pageLine !== undefined, "ページの行が出ます");
  assert.match(pageLine, /total_ms=1\.00/);
  assert.match(pageLine, /rpc_ms=5\.00/);
  assert.match(pageLine, /next_ms=0\.00/);
});

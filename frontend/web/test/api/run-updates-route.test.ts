// run 詳細のログの追記分を返す Route Handler の契約（BFF の取得境界）。
//
// ブラウザは開いている run 詳細の分だけ、この Route Handler へ前回の続きの位置（?epoch= と ?after=）を
// 付けて要求する。最初の要求は位置を付けない。Route Handler は backend の GetRunUpdates へ位置を渡し、
// 応答を proto の JSON 表現で返す。位置の誤りは 400、見つからない run は 404、backend 不通は 503 にする。
import test from "node:test";
import assert from "node:assert/strict";
import * as grpc from "@grpc/grpc-js";
import { NextRequest } from "next/server";
import { reserveClosedAddress, startTaktDashServer } from "../support/grpc-server.ts";
import { GET as getRunUpdates } from "../../app/api/runs/[slug]/updates/route.ts";
import type { GetRunUpdatesRequest, GetRunUpdatesResponse } from "../../src/proto/taktdash.ts";
import { closeBackendClient } from "../../src/grpc/client.ts";
import { restoreRunUpdates } from "../../src/state/run-log-stream.ts";

const withServerEnv = async <T>(address: string, run: () => Promise<T>): Promise<T> => {
  await closeBackendClient();
  const previous = process.env.TAKTDASH_BACKEND_ADDR;
  process.env.TAKTDASH_BACKEND_ADDR = address;
  try {
    return await run();
  } finally {
    await closeBackendClient();
    if (previous === undefined) {
      delete process.env.TAKTDASH_BACKEND_ADDR;
    } else {
      process.env.TAKTDASH_BACKEND_ADDR = previous;
    }
  }
};

const runContext = (slug: string) => ({ params: Promise.resolve({ slug }) });

const updatesResponse = (overrides: Partial<GetRunUpdatesResponse> = {}): GetRunUpdatesResponse => ({
  runSlug: "run-aaa",
  epoch: 7,
  reset: false,
  lines: [{ sequence: 13, text: "2026-11-13T09:20:00Z step_complete implement" }],
  activity: { latestTool: "Bash", toolUseCount: 12, updatedAt: new Date("2026-11-13T09:12:00Z") },
  logsRemoved: false,
  ...overrides,
});

test("位置付きの要求は、世代と番号を GetRunUpdates へ渡し、追記分の JSON を返す", async () => {
  let received: GetRunUpdatesRequest | undefined;
  const server = await startTaktDashServer({
    runUpdates: updatesResponse(),
    onGetRunUpdates: (request) => {
      received = request;
    },
  });
  try {
    await withServerEnv(server.address, async () => {
      const response = await getRunUpdates(
        new NextRequest("http://localhost/api/runs/run-aaa/updates?epoch=7&after=12"),
        runContext("run-aaa"),
      );
      assert.equal(response.status, 200);
      assert.equal(response.headers.get("cache-control"), "no-store");
      assert.deepEqual(restoreRunUpdates(await response.json()), {
        epoch: 7,
        reset: false,
        lines: [{ sequence: 13, text: "2026-11-13T09:20:00Z step_complete implement" }],
        activity: { latestTool: "Bash", toolUseCount: 12, updatedAt: new Date("2026-11-13T09:12:00Z") },
        logsRemoved: false,
      });
    });
    assert.equal(received?.runSlug, "run-aaa");
    assert.equal(received?.epoch, 7);
    assert.equal(received?.afterSequence, 12);
  } finally {
    await server.shutdown();
  }
});

test("位置の無い最初の要求は、世代も番号も付けずに GetRunUpdates を呼ぶ", async () => {
  let received: GetRunUpdatesRequest | undefined;
  const server = await startTaktDashServer({
    runUpdates: updatesResponse({ reset: true }),
    onGetRunUpdates: (request) => {
      received = request;
    },
  });
  try {
    await withServerEnv(server.address, async () => {
      const response = await getRunUpdates(new NextRequest("http://localhost/api/runs/run-aaa/updates"), runContext("run-aaa"));
      assert.equal(response.status, 200);
      assert.equal(restoreRunUpdates(await response.json()).reset, true);
    });
    assert.equal(received?.runSlug, "run-aaa");
    assert.equal(received?.epoch, undefined, "位置の無い要求に世代を付けない");
    assert.equal(received?.afterSequence, undefined, "位置の無い要求に番号を付けない");
  } finally {
    await server.shutdown();
  }
});

test("位置の片方だけ、負の値、整数でない値、不正な runSlug の要求は 400 で、backend を呼ばない", async () => {
  const calls: string[] = [];
  const server = await startTaktDashServer({ runUpdates: updatesResponse(), calls });
  try {
    await withServerEnv(server.address, async () => {
      for (const [slug, query] of [
        ["run-aaa", "?epoch=7"],
        ["run-aaa", "?after=12"],
        ["run-aaa", "?epoch=&after="],
        ["run-aaa", "?epoch=-1&after=0"],
        ["run-aaa", "?epoch=7&after=-1"],
        ["run-aaa", "?epoch=7&after=1.5"],
        ["run-aaa", "?epoch=abc&after=1"],
        ["..", ""],
      ] as const) {
        const response = await getRunUpdates(
          new NextRequest(`http://localhost/api/runs/${encodeURIComponent(slug)}/updates${query}`),
          runContext(slug),
        );
        assert.equal(response.status, 400, `slug=${slug} query=${JSON.stringify(query)} は 400 です`);
      }
    });
    assert.deepEqual(calls, [], "指定の誤りは backend へ渡さない");
  } finally {
    await server.shutdown();
  }
});

test("GetRunUpdates が NOT_FOUND のとき Route Handler は 404 を返す", async () => {
  const server = await startTaktDashServer({ runUpdates: { code: grpc.status.NOT_FOUND, details: "missing" } });
  try {
    await withServerEnv(server.address, async () => {
      const response = await getRunUpdates(new NextRequest("http://localhost/api/runs/run-aaa/updates"), runContext("run-aaa"));
      assert.equal(response.status, 404);
    });
  } finally {
    await server.shutdown();
  }
});

test("backend の応答の runSlug が要求と違うときは 404 を返す", async () => {
  const server = await startTaktDashServer({ runUpdates: updatesResponse({ runSlug: "run-other" }) });
  try {
    await withServerEnv(server.address, async () => {
      const response = await getRunUpdates(new NextRequest("http://localhost/api/runs/run-aaa/updates"), runContext("run-aaa"));
      assert.equal(response.status, 404);
    });
  } finally {
    await server.shutdown();
  }
});

test("backend に接続できないとき Route Handler は 503 を返す", async () => {
  const closedAddress = await reserveClosedAddress();
  await withServerEnv(closedAddress, async () => {
    const response = await getRunUpdates(new NextRequest("http://localhost/api/runs/run-aaa/updates"), runContext("run-aaa"));
    assert.equal(response.status, 503);
    assert.deepEqual(await response.json(), { error: "backend に接続できません。" });
  });
});

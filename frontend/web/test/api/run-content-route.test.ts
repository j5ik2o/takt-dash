// run 詳細の本文を返す Route Handler の契約（BFF の取得境界）。
//
// ブラウザはレポートや指示書の本文を開いたときに、この Route Handler へ ?report= か ?instruction= の
// どちらか一方を付けて要求する。Route Handler は backend の GetRunContent へ指定した対象だけを渡し、
// 応答を proto の JSON 表現で返す。指定の誤りは 400、見つからない対象は 404、backend 不通は 503 にする。
import test from "node:test";
import assert from "node:assert/strict";
import * as grpc from "@grpc/grpc-js";
import { NextRequest } from "next/server";
import { reserveClosedAddress, startTaktDashServer } from "../support/grpc-server.ts";
import { GET as getRunContent } from "../../app/api/runs/[slug]/content/route.ts";
import type { GetRunContentRequest, GetRunContentResponse } from "../../src/proto/taktdash.ts";
import { closeBackendClient } from "../../src/grpc/client.ts";
import { restoreRunContentResponse } from "../../src/state/run-content.ts";

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

const contentResponse = (content: string): GetRunContentResponse => ({
  runSlug: "run-aaa",
  content,
  sizeBytes: Buffer.byteLength(content),
  truncated: false,
});

test("?report= の要求は、指定したレポート名だけを GetRunContent へ渡し、本文の JSON を返す", async () => {
  let received: GetRunContentRequest | undefined;
  const server = await startTaktDashServer({
    runContent: contentResponse("レビュー本文"),
    onGetRunContent: (request) => {
      received = request;
    },
  });
  try {
    await withServerEnv(server.address, async () => {
      const url = `http://localhost/api/runs/run-aaa/content?report=${encodeURIComponent("sub/review.md")}`;
      const response = await getRunContent(new NextRequest(url), runContext("run-aaa"));
      assert.equal(response.status, 200);
      assert.equal(response.headers.get("cache-control"), "no-store");
      const body = (await response.json()) as { runSlug: string };
      assert.equal(body.runSlug, "run-aaa");
      // proto の JSON 表現は既定値（truncated=false）を省くため、画面が使う復元を通して確かめる。
      assert.deepEqual(restoreRunContentResponse(body), {
        content: "レビュー本文",
        sizeBytes: Buffer.byteLength("レビュー本文"),
        truncated: false,
      });
    });
    assert.equal(received?.runSlug, "run-aaa");
    assert.equal(received?.reportFilename, "sub/review.md");
    assert.equal(received?.instructionId, undefined, "レポートの要求に指示書の ID を付けない");
  } finally {
    await server.shutdown();
  }
});

test("?instruction= の要求は、指定した指示書の ID だけを GetRunContent へ渡す", async () => {
  let received: GetRunContentRequest | undefined;
  const server = await startTaktDashServer({
    runContent: contentResponse("二つ目"),
    onGetRunContent: (request) => {
      received = request;
    },
  });
  try {
    await withServerEnv(server.address, async () => {
      const response = await getRunContent(
        new NextRequest("http://localhost/api/runs/run-aaa/content?instruction=2"),
        runContext("run-aaa"),
      );
      assert.equal(response.status, 200);
      const body = (await response.json()) as { content: string };
      assert.equal(body.content, "二つ目");
    });
    assert.equal(received?.runSlug, "run-aaa");
    assert.equal(received?.instructionId, 2);
    assert.equal(received?.reportFilename, undefined, "指示書の要求にレポート名を付けない");
  } finally {
    await server.shutdown();
  }
});

test("対象の指定が無い、両方ある、指示書の ID が正の整数でない要求は 400 で、backend を呼ばない", async () => {
  const calls: string[] = [];
  const server = await startTaktDashServer({ runContent: contentResponse("本文"), calls });
  try {
    await withServerEnv(server.address, async () => {
      for (const query of [
        "",
        "?report=plan.md&instruction=1",
        "?report=",
        "?instruction=",
        "?instruction=0",
        "?instruction=-1",
        "?instruction=1.5",
        "?instruction=abc",
      ]) {
        const response = await getRunContent(
          new NextRequest(`http://localhost/api/runs/run-aaa/content${query}`),
          runContext("run-aaa"),
        );
        assert.equal(response.status, 400, `query=${JSON.stringify(query)} は 400 です`);
      }
    });
    assert.deepEqual(calls, [], "指定の誤りは backend へ渡さない");
  } finally {
    await server.shutdown();
  }
});

test("GetRunContent が NOT_FOUND のとき Route Handler は 404 を返す", async () => {
  const server = await startTaktDashServer({ runContent: { code: grpc.status.NOT_FOUND, details: "missing" } });
  try {
    await withServerEnv(server.address, async () => {
      const response = await getRunContent(
        new NextRequest("http://localhost/api/runs/run-aaa/content?report=missing.md"),
        runContext("run-aaa"),
      );
      assert.equal(response.status, 404);
    });
  } finally {
    await server.shutdown();
  }
});

test("backend に接続できないとき Route Handler は 503 を返す", async () => {
  const closedAddress = await reserveClosedAddress();
  await withServerEnv(closedAddress, async () => {
    const response = await getRunContent(
      new NextRequest("http://localhost/api/runs/run-aaa/content?report=plan.md"),
      runContext("run-aaa"),
    );
    assert.equal(response.status, 503);
    assert.deepEqual(await response.json(), { error: "backend に接続できません。" });
  });
});

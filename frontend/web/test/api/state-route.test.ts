// SSE 中継の Route Handler の契約（C14/C16）。
//
// backend の WatchState（サーバーストリーミング）を SSE に中継する。
// 初回接続で backend が返す最初の全件が、そのまま最初のイベントに現れること、
// クライアント切断で gRPC ストリームを閉じることを観測する。
import test from "node:test";
import assert from "node:assert/strict";
import { NextRequest } from "next/server";
import { createDashboardState, NOW } from "../support/fixtures.ts";
import { startTaktDashServer } from "../support/grpc-server.ts";
import { closeBackendClient, getBackendClient } from "../../src/grpc/client.ts";
import { GET } from "../../app/api/state/route.ts";

const readEvents = async (stream: ReadableStream<Uint8Array>, count: number): Promise<string[]> => {
  const decoder = new TextDecoder();
  const reader = stream.getReader();
  const events: string[] = [];
  let buffer = "";
  try {
    while (events.length < count) {
      const { value, done } = await reader.read();
      if (done) {
        break;
      }
      buffer += decoder.decode(value, { stream: true });
      let separator = buffer.indexOf("\n\n");
      while (separator >= 0) {
        events.push(buffer.slice(0, separator));
        buffer = buffer.slice(separator + 2);
        separator = buffer.indexOf("\n\n");
      }
    }
  } finally {
    await reader.cancel();
  }
  return events;
};

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

// SSE を終えた後もプロセス共有のクライアントが破棄されていないことを観測する。
// 同一インスタンスであることと、後続の RPC が成功することの両方を確認する。
const assertSharedClientSurvives = async (
  client: ReturnType<typeof getBackendClient>,
  after: string,
): Promise<void> => {
  assert.equal(getBackendClient(), client, `${after} の後も共有クライアントは同じインスタンスを返す`);
  const overview = await client.getOverview();
  assert.equal(overview.servers.length, 2, `${after} の後も GetOverview が成功する`);
};

test("WatchState の最初の全件が最初の SSE イベントになり、2件目以降も順に届く", async () => {
  const first = createDashboardState();
  const second = { ...createDashboardState(), readAt: new Date(NOW.getTime() + 30_000) };
  const server = await startTaktDashServer({ watch: [first, second] });
  try {
    await withServerEnv(server.address, async () => {
      const response = await GET(new NextRequest("http://localhost/api/state"));
      assert.match(response.headers.get("content-type") ?? "", /text\/event-stream/);
      assert.ok(response.body);
      const events = await readEvents(response.body, 2);
      assert.equal(events.length, 2);
      assert.match(events[0]!, /Mac mini/);
      assert.match(events[1]!, /2026-11-13T07:30:30/);
    });
  } finally {
    await server.shutdown();
  }
});

test("クライアント切断（abort）で gRPC のストリームを閉じる", async () => {
  const controller = new AbortController();
  let cancelled = false;
  const server = await startTaktDashServer({
    watch: [createDashboardState()],
    holdWatchOpen: true,
    onWatchCancelled: () => {
      cancelled = true;
    },
  });
  try {
    await withServerEnv(server.address, async () => {
      const response = await GET(new NextRequest("http://localhost/api/state", { signal: controller.signal }));
      assert.ok(response.body);
      const reader = response.body.getReader();
      const first = await reader.read();
      assert.equal(first.done, false);
      controller.abort();
      await reader.cancel();
    });
  } finally {
    await server.shutdown();
  }
  const deadline = Date.now() + 2_000;
  while (!cancelled && Date.now() < deadline) {
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
  assert.equal(cancelled, true, "切断後に backend の WatchState ストリームがキャンセルされる");
});

test("abort（finish）後も共有 gRPC クライアントは破棄されない", async () => {
  const controller = new AbortController();
  const server = await startTaktDashServer({
    overview: createDashboardState(),
    watch: [createDashboardState()],
    holdWatchOpen: true,
  });
  try {
    await withServerEnv(server.address, async () => {
      const client = getBackendClient();
      const response = await GET(new NextRequest("http://localhost/api/state", { signal: controller.signal }));
      assert.ok(response.body);
      const reader = response.body.getReader();
      const first = await reader.read();
      assert.equal(first.done, false);
      controller.abort();
      await reader.cancel();
      await assertSharedClientSurvives(client, "abort");
    });
  } finally {
    await server.shutdown();
  }
});

test("受信側 cancel 後も共有 gRPC クライアントは破棄されない", async () => {
  const server = await startTaktDashServer({
    overview: createDashboardState(),
    watch: [createDashboardState()],
    holdWatchOpen: true,
  });
  try {
    await withServerEnv(server.address, async () => {
      const client = getBackendClient();
      const response = await GET(new NextRequest("http://localhost/api/state"));
      assert.ok(response.body);
      const reader = response.body.getReader();
      const first = await reader.read();
      assert.equal(first.done, false);
      await reader.cancel();
      await assertSharedClientSurvives(client, "受信側の cancel");
    });
  } finally {
    await server.shutdown();
  }
});

test("backend のストリーム終了（end）後も共有 gRPC クライアントは破棄されない", async () => {
  const server = await startTaktDashServer({
    overview: createDashboardState(),
    watch: [createDashboardState()],
  });
  try {
    await withServerEnv(server.address, async () => {
      const client = getBackendClient();
      const response = await GET(new NextRequest("http://localhost/api/state"));
      assert.ok(response.body);
      const reader = response.body.getReader();
      let done = false;
      while (!done) {
        done = (await reader.read()).done;
      }
      await assertSharedClientSurvives(client, "backend のストリーム end");
    });
  } finally {
    await server.shutdown();
  }
});

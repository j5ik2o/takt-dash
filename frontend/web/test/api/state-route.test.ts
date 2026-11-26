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
import { GET, HEAD } from "../../app/api/state/route.ts";

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

// issue #385: 画面に見捨てられた接続を、サーバー側に溜めない。
const SCREEN_A = "0123456789abcdef0123456789abcdef";
const SCREEN_B = "fedcba9876543210fedcba9876543210";
let screenSerial = 0;
// route.ts の登録簿はプロセスで 1 つなので、テストごとに別の画面の識別子を使う。
const freshScreen = (base: string): string => {
  screenSerial += 1;
  return `${base.slice(0, 24)}${screenSerial.toString(16).padStart(8, "0")}`;
};
const stateUrl = (screen: string, attempt: number): string =>
  `http://localhost/api/state?screen=${screen}&attempt=${attempt}`;

const waitUntil = async (predicate: () => boolean, timeoutMs = 2_000): Promise<void> => {
  const deadline = Date.now() + timeoutMs;
  while (!predicate() && Date.now() < deadline) {
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
};

const readWithin = async <T>(reader: ReadableStreamDefaultReader<T>, ms: number) =>
  Promise.race([
    reader.read(),
    new Promise<"timeout">((resolve) => setTimeout(() => resolve("timeout"), ms)),
  ]);

const decodeEvent = (chunk: Uint8Array | undefined): string => new TextDecoder().decode(chunk);

test("同じ画面の新しい試行が届いたら、古い接続を閉じて backend の購読を外す", async () => {
  let cancelled = 0;
  const calls: string[] = [];
  const server = await startTaktDashServer({
    watch: [createDashboardState()],
    holdWatchOpen: true,
    calls,
    onWatchCancelled: () => {
      cancelled += 1;
    },
  });
  const screen = freshScreen(SCREEN_A);
  try {
    await withServerEnv(server.address, async () => {
      const older = await GET(new NextRequest(stateUrl(screen, 1)));
      const olderReader = older.body!.getReader();
      assert.equal((await olderReader.read()).done, false);
      const newer = await GET(new NextRequest(stateUrl(screen, 2)));
      const newerReader = newer.body!.getReader();
      assert.equal((await newerReader.read()).done, false);
      const olderNext = await readWithin(olderReader, 1_000);
      assert.notEqual(olderNext, "timeout", "古い接続は待たずに終わる");
      assert.equal(olderNext !== "timeout" && olderNext.done, true, "古い接続は終わる");
      await waitUntil(() => cancelled === 1);
      assert.equal(cancelled, 1, "古い接続の backend の購読だけを外す");
      assert.equal(await readWithin(newerReader, 100), "timeout", "新しい接続は開いたまま");
      await newerReader.cancel();
    });
  } finally {
    await server.shutdown();
  }
  assert.deepEqual(calls.filter((call) => call === "WatchState").length, 2);
});

test("新しい試行が開いている間に遅れて届いた古い試行は、backend を購読せずに 204 で断る", async () => {
  const calls: string[] = [];
  let cancelled = 0;
  const server = await startTaktDashServer({
    watch: [createDashboardState()],
    holdWatchOpen: true,
    calls,
    onWatchCancelled: () => {
      cancelled += 1;
    },
  });
  const screen = freshScreen(SCREEN_A);
  try {
    await withServerEnv(server.address, async () => {
      const newer = await GET(new NextRequest(stateUrl(screen, 3)));
      const newerReader = newer.body!.getReader();
      assert.equal((await newerReader.read()).done, false);
      const stale = await GET(new NextRequest(stateUrl(screen, 2)));
      assert.equal(stale.status, 204);
      assert.equal(stale.body, null);
      assert.equal(await readWithin(newerReader, 100), "timeout", "いま使っている接続は閉じない");
      assert.equal(cancelled, 0);
      await newerReader.cancel();
    });
  } finally {
    await server.shutdown();
  }
  assert.equal(calls.filter((call) => call === "WatchState").length, 1, "断った試行は backend を購読しない");
});

test("別の画面の接続と、識別子の無い接続は、付け替えで閉じない", async () => {
  const server = await startTaktDashServer({ watch: [createDashboardState()], holdWatchOpen: true });
  const screenA = freshScreen(SCREEN_A);
  const screenB = freshScreen(SCREEN_B);
  try {
    await withServerEnv(server.address, async () => {
      const readers = [];
      for (const url of [
        stateUrl(screenA, 1),
        "http://localhost/api/state",
        `http://localhost/api/state?screen=${screenA.toUpperCase()}&attempt=9`,
        stateUrl(screenB, 5),
        "http://localhost/api/state",
      ]) {
        const response = await GET(new NextRequest(url));
        assert.equal(response.status, 200);
        const reader = response.body!.getReader();
        assert.equal((await reader.read()).done, false);
        readers.push(reader);
      }
      for (const reader of readers) {
        assert.equal(await readWithin(reader, 50), "timeout", "どの接続も開いたまま");
        await reader.cancel();
      }
    });
  } finally {
    await server.shutdown();
  }
});

test("Next が読めない間は最新の状態 1 件だけを持ち、読めたら最新を送る", async () => {
  const states = [0, 1, 2, 3, 4].map((offset) => ({
    ...createDashboardState(),
    readAt: new Date(NOW.getTime() + offset * 1_000),
  }));
  const server = await startTaktDashServer({ watch: states, holdWatchOpen: true });
  let received = 0;
  try {
    await withServerEnv(server.address, async () => {
      // 中継より先に data を数える聞き手を付け、5 件を受けたことを確かめてから読む。
      const client = getBackendClient();
      const original = client.watchState.bind(client);
      client.watchState = () => {
        const stream = original();
        stream.on("data", () => {
          received += 1;
        });
        return stream;
      };
      const response = await GET(new NextRequest(stateUrl(freshScreen(SCREEN_A), 1)));
      await waitUntil(() => received === states.length);
      assert.equal(received, states.length, "backend の 5 件を受けてから読む");
      // 同じ data を、数える聞き手のあとに中継の聞き手が受ける。待ちを 1 回流してから読む。
      await new Promise((resolve) => setImmediate(resolve));
      const reader = response.body!.getReader();
      const first = await reader.read();
      assert.match(decodeEvent(first.value), /2026-11-13T07:30:00/);
      const second = await reader.read();
      assert.match(decodeEvent(second.value), /2026-11-13T07:30:04/, "途中の版を飛ばして最新を送る");
      assert.equal(await readWithin(reader, 100), "timeout", "飛ばした版は送らない");
      await reader.cancel();
    });
  } finally {
    await server.shutdown();
  }
});

test("Next の cancel のあとに abort が来ても、gRPC のストリームは 1 回だけ閉じる", async () => {
  const controller = new AbortController();
  const server = await startTaktDashServer({ watch: [createDashboardState()], holdWatchOpen: true });
  let cancels = 0;
  try {
    await withServerEnv(server.address, async () => {
      const client = getBackendClient();
      const original = client.watchState.bind(client);
      client.watchState = () => {
        const stream = original();
        const cancel = stream.cancel.bind(stream);
        stream.cancel = () => {
          cancels += 1;
          cancel();
        };
        return stream;
      };
      const response = await GET(
        new NextRequest(stateUrl(freshScreen(SCREEN_A), 1), { signal: controller.signal }),
      );
      const reader = response.body!.getReader();
      assert.equal((await reader.read()).done, false);
      await reader.cancel();
      controller.abort();
    });
  } finally {
    await server.shutdown();
  }
  assert.equal(cancels, 1);
});

test("HEAD は本文を持たず、登録も backend の購読もしない", async () => {
  const calls: string[] = [];
  const server = await startTaktDashServer({ watch: [createDashboardState()], holdWatchOpen: true, calls });
  const screen = freshScreen(SCREEN_A);
  try {
    await withServerEnv(server.address, async () => {
      const live = await GET(new NextRequest(stateUrl(screen, 1)));
      const reader = live.body!.getReader();
      assert.equal((await reader.read()).done, false);
      const head = HEAD();
      assert.equal(head.status, 200);
      assert.equal(head.body, null);
      assert.match(head.headers.get("content-type") ?? "", /text\/event-stream/);
      assert.equal(await readWithin(reader, 100), "timeout", "HEAD は配信中の接続を閉じない");
      await reader.cancel();
    });
  } finally {
    await server.shutdown();
  }
  assert.equal(calls.filter((call) => call === "WatchState").length, 1, "HEAD は backend を購読しない");
});

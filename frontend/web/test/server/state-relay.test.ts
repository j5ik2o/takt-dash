// 状態の配信の中継（relayStateStream）と画面の登録簿（issue #385）。
//
// 本物の ReadableStream・AbortController・登録簿と、偽の gRPC ストリームで中継を動かす。
// 仕様は specs/quint/state-stream-relay/stateStreamRelay.qnt。性質の番号は
// docs/design/issue-385.md の「PBT の性質」。
import test from "node:test";
import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import { DashboardState } from "../../src/proto/taktdash.ts";
import { createScreenStreams, type ScreenStreams } from "../../src/server/screen-streams.ts";
import { relayStateStream, type WatchStream } from "../../src/server/state-relay.ts";
import { parseStreamTicket, withStreamTicket, type StreamTicket } from "../../src/state/stream-ticket.ts";

// on と emit は EventEmitter のものを使う（gRPC の ClientReadableStream と同じ）。
class FakeWatchStream extends EventEmitter implements WatchStream {
  cancels = 0;
  destroys = 0;
  cancel(): void {
    this.cancels += 1;
  }
  destroy(): void {
    this.destroys += 1;
  }
}

// 状態の版を、符号化した中身から読めるようにする。
const versions = new WeakMap<DashboardState, number>();
const stateOf = (version: number): DashboardState => {
  const state = DashboardState.create({});
  versions.set(state, version);
  return state;
};
const encode = (state: DashboardState): Uint8Array => new TextEncoder().encode(String(versions.get(state)));
const decode = (chunk: Uint8Array): number => Number(new TextDecoder().decode(chunk));

const flush = () => new Promise<void>((resolve) => setImmediate(resolve));

type Relay = {
  stream: FakeWatchStream | undefined;
  body: ReadableStream<Uint8Array> | undefined;
  abort: AbortController;
};

const openRelay = (screens: ScreenStreams, ticket: StreamTicket | undefined): Relay => {
  const abort = new AbortController();
  const relay: Relay = { stream: undefined, body: undefined, abort };
  relay.body = relayStateStream({
    openStream: () => {
      relay.stream = new FakeWatchStream();
      return relay.stream;
    },
    signal: abort.signal,
    admit: ticket === undefined ? undefined : (close) => screens.admit(ticket, close),
    encode,
  });
  return relay;
};

const SCREEN_A = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SCREEN_B = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

test("同じ画面の新しい試行で古い接続を閉じ、登録を付け替える", async () => {
  const screens = createScreenStreams();
  const older = openRelay(screens, { screen: SCREEN_A, attempt: 1 });
  const newer = openRelay(screens, { screen: SCREEN_A, attempt: 2 });
  assert.equal(older.stream?.cancels, 1);
  assert.equal(older.stream?.destroys, 1);
  assert.equal(newer.stream?.cancels, 0);
  assert.equal(screens.size(), 1);
  assert.equal((await older.body!.getReader().read()).done, true, "古い接続は終わる");
  newer.abort.abort();
  assert.equal(newer.stream?.cancels, 1);
  assert.equal(screens.size(), 0, "閉じた接続の登録は残らない");
});

test("番号が同じ試行は後から届いたほうを残し、小さい試行は gRPC を開かずに断る", () => {
  const screens = createScreenStreams();
  const first = openRelay(screens, { screen: SCREEN_A, attempt: 5 });
  const duplicate = openRelay(screens, { screen: SCREEN_A, attempt: 5 });
  assert.equal(first.stream?.cancels, 1);
  assert.equal(duplicate.stream?.cancels, 0);
  const stale = openRelay(screens, { screen: SCREEN_A, attempt: 4 });
  assert.equal(stale.body, undefined);
  assert.equal(stale.stream, undefined, "断った試行は gRPC を開かない");
  assert.equal(duplicate.stream?.cancels, 0, "断っても、いま使っている接続は閉じない");
  const maxAttempt = openRelay(screens, { screen: SCREEN_A, attempt: Number.MAX_SAFE_INTEGER });
  assert.equal(duplicate.stream?.cancels, 1);
  assert.equal(maxAttempt.stream?.cancels, 0);
  maxAttempt.abort.abort();
});

test("閉じた古い接続の後始末は、付け替わった新しい登録を消さない", () => {
  const screens = createScreenStreams();
  const older = openRelay(screens, { screen: SCREEN_A, attempt: 1 });
  const newer = openRelay(screens, { screen: SCREEN_A, attempt: 2 });
  older.abort.abort();
  older.stream?.emit("error", new Error("late"));
  assert.equal(older.stream?.cancels, 1);
  assert.equal(screens.size(), 1);
  const stale = openRelay(screens, { screen: SCREEN_A, attempt: 1 });
  assert.equal(stale.body, undefined, "登録が残っているので、古い試行は断る");
  newer.abort.abort();
});

test("識別子の無い接続は登録せず、互いに閉じない", () => {
  const screens = createScreenStreams();
  const a = openRelay(screens, undefined);
  const b = openRelay(screens, undefined);
  assert.equal(a.stream?.cancels, 0);
  assert.equal(b.stream?.cancels, 0);
  assert.equal(screens.size(), 0);
  a.abort.abort();
  b.abort.abort();
});

test("既に abort された要求は gRPC を開かず、登録も残さない", () => {
  const screens = createScreenStreams();
  const abort = new AbortController();
  abort.abort();
  let opened = 0;
  const body = relayStateStream({
    openStream: () => {
      opened += 1;
      return new FakeWatchStream();
    },
    signal: abort.signal,
    admit: (close) => screens.admit({ screen: SCREEN_A, attempt: 1 }, close),
    encode,
  });
  assert.ok(body);
  assert.equal(opened, 0);
  assert.equal(screens.size(), 0);
});

test("backend の end のあとは gRPC を閉じ直さず、遅れた data も送らない", async () => {
  const screens = createScreenStreams();
  const relay = openRelay(screens, { screen: SCREEN_A, attempt: 1 });
  const reader = relay.body!.getReader();
  relay.stream!.emit("data", stateOf(1));
  relay.stream!.emit("end");
  relay.stream!.emit("data", stateOf(2));
  assert.equal(decode((await reader.read()).value!), 1);
  assert.equal((await reader.read()).done, true);
  relay.abort.abort();
  await reader.cancel();
  assert.equal(relay.stream!.cancels, 0);
  assert.equal(screens.size(), 0);
});

// ===== URL の識別子と番号の境界 =====

test("withStreamTicket と parseStreamTicket は往復する", () => {
  const ticket = { screen: "0123456789abcdef0123456789abcdef", attempt: 12 };
  const url = new URL(withStreamTicket("/api/state", ticket), "http://localhost");
  assert.deepEqual(parseStreamTicket(url.searchParams), ticket);
  assert.equal(withStreamTicket("/api/state?x=1", ticket), `/api/state?x=1&screen=${ticket.screen}&attempt=12`);
});

test("形の合わない識別子と番号は、識別子なしと同じ扱いにする", () => {
  const valid = "0123456789abcdef0123456789abcdef";
  const cases: Array<[string, string]> = [
    ["", "attempt=1"],
    [`screen=${valid}`, ""],
    [`screen=${valid.toUpperCase()}`, "attempt=1"],
    [`screen=${valid.slice(1)}`, "attempt=1"],
    [`screen=${valid}0`, "attempt=1"],
    [`screen=${valid.slice(2)}%E3%81%82`, "attempt=1"],
    [`screen=${valid.slice(1)}%00`, "attempt=1"],
    [`screen=${valid}`, "attempt=0"],
    [`screen=${valid}`, "attempt=-1"],
    [`screen=${valid}`, "attempt=1.5"],
    [`screen=${valid}`, "attempt=1e3"],
    [`screen=${valid}`, "attempt=01"],
    [`screen=${valid}`, "attempt=12345678901234567"],
    [`screen=${valid}`, "attempt=9007199254740992"],
    [`screen=${valid}`, "attempt="],
    [`screen=`, "attempt=1"],
  ];
  for (const [screen, attempt] of cases) {
    const params = new URLSearchParams([screen, attempt].filter(Boolean).join("&"));
    assert.equal(parseStreamTicket(params), undefined, `${screen}&${attempt}`);
  }
  assert.deepEqual(parseStreamTicket(new URLSearchParams(`screen=${valid}&attempt=9007199254740991`)), {
    screen: valid,
    attempt: Number.MAX_SAFE_INTEGER,
  });
  assert.deepEqual(
    parseStreamTicket(new URLSearchParams(`screen=${valid}&attempt=2&screen=${"f".repeat(32)}&attempt=9`)),
    { screen: valid, attempt: 2 },
    "同じ名前が 2 つあれば最初の値を読む",
  );
});

// ===== 状態機械の性質テスト（PBT） =====
//
// 画面 2 つと識別子なしの接続を、番号の重複・逆順を含めて開き、data・error・end・abort・
// 読み・cancel をシード固定の乱数で並べる。テストの中に期待の模型を持ち、各手のあとに
// 実物（偽の gRPC の cancel の回数、登録簿の大きさ、読めた版）と照らし合わせる。

const mulberry32 = (seed: number): (() => number) => {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
};

type ModelConn = {
  id: number;
  ticket: StreamTicket | undefined;
  relay: Relay;
  reader: ReadableStreamDefaultReader<Uint8Array> | undefined;
  // 期待: rejected は開かない。open は開いている。closed は閉じた（cancelled なら gRPC を閉じた）。
  status: "rejected" | "open" | "closed";
  expectCancels: number;
  produced: number;
  delivered: number[];
  done: boolean;
  inflight: boolean;
};

const runRelayMachine = async (seed: number, steps: number): Promise<void> => {
  const rand = mulberry32(seed);
  const pickOf = <T>(items: T[]): T | undefined => (items.length === 0 ? undefined : items[Math.floor(rand() * items.length)]);
  const screens = createScreenStreams();
  const conns: ModelConn[] = [];
  const registry = new Map<string, ModelConn>();
  const highest = new Map<string, number>();
  let step = 0;
  let op = "init";
  const where = () => `seed=${seed} step=${step} op=${op}`;

  const closeModel = (conn: ModelConn, cancelsStream: boolean) => {
    if (conn.status !== "open") return;
    conn.status = "closed";
    if (cancelsStream) conn.expectCancels = 1;
    if (conn.ticket !== undefined && registry.get(conn.ticket.screen) === conn) registry.delete(conn.ticket.screen);
  };

  const readOnce = (conn: ModelConn) => {
    conn.inflight = true;
    void conn.reader!.read().then((result) => {
      conn.inflight = false;
      if (result.done) {
        conn.done = true;
        return;
      }
      const version = decode(result.value);
      const last = conn.delivered.at(-1) ?? 0;
      assert.ok(version > last, `性質 9: 読んだ版は増える一方 ${last} -> ${version} ${where()}`);
      conn.delivered.push(version);
    });
  };

  const openOp = async () => {
    const kind = rand();
    let ticket: StreamTicket | undefined;
    if (kind < 0.8) {
      const screen = rand() < 0.5 ? SCREEN_A : SCREEN_B;
      const top = highest.get(screen) ?? 0;
      const roll = rand();
      const attempt = roll < 0.5 ? top + 1 : roll < 0.7 ? Math.max(1, top) : roll < 0.9 ? Math.max(1, top - 1) : 1 + Math.floor(rand() * 5);
      highest.set(screen, Math.max(top, attempt));
      ticket = { screen, attempt };
    }
    const holder = ticket === undefined ? undefined : registry.get(ticket.screen);
    const relay = openRelay(screens, ticket);
    const conn: ModelConn = {
      id: conns.length,
      ticket,
      relay,
      reader: undefined,
      status: "open",
      expectCancels: 0,
      produced: 0,
      delivered: [],
      done: false,
      inflight: false,
    };
    if (ticket !== undefined && holder !== undefined && holder.ticket!.attempt > ticket.attempt) {
      conn.status = "rejected";
    } else {
      if (holder !== undefined) closeModel(holder, true);
      if (ticket !== undefined) registry.set(ticket.screen, conn);
      conn.reader = relay.body!.getReader();
    }
    conns.push(conn);
  };

  const withOpenStream = (run: (conn: ModelConn) => void) => {
    const conn = pickOf(conns.filter((c) => c.relay.stream !== undefined));
    if (conn !== undefined) run(conn);
  };

  const ops: Array<[string, number, () => void | Promise<void>]> = [
    ["open", 3, openOp],
    ["data", 6, () =>
      withOpenStream((conn) => {
        if (conn.status === "open") conn.produced += 1;
        conn.relay.stream!.emit("data", stateOf(conn.status === "open" ? conn.produced : 999));
      })],
    ["error", 1, () =>
      withOpenStream((conn) => {
        closeModel(conn, true);
        conn.relay.stream!.emit("error", new Error("boom"));
      })],
    ["end", 1, () =>
      withOpenStream((conn) => {
        closeModel(conn, false);
        conn.relay.stream!.emit("end");
      })],
    ["abort", 1, () =>
      withOpenStream((conn) => {
        closeModel(conn, true);
        conn.relay.abort.abort();
      })],
    ["cancel", 1, async () => {
      const conn = pickOf(conns.filter((c) => c.reader !== undefined && !c.done));
      if (conn === undefined) return;
      closeModel(conn, true);
      conn.done = true;
      await conn.reader!.cancel();
    }],
    ["drain", 4, async () => {
      const conn = pickOf(conns.filter((c) => c.reader !== undefined && !c.done && !c.inflight));
      if (conn === undefined) return;
      const before = conn.delivered.length;
      // 読める分を読み切る。読みが止まったら、その読みは次の data を待って残る。
      while (!conn.done && !conn.inflight) {
        readOnce(conn);
        await flush();
      }
      // 性質 8: Next が読まない間に何件来ても、列の 1 件と持ち分の 1 件の 2 件まで。
      assert.ok(conn.delivered.length - before <= 2, `性質 8: 詰まった間に積むのは 2 件まで ${where()}`);
      // 性質 10: 開いている接続を読み切れば、最後に読んだのは最新の版。
      if (conn.status === "open" && conn.produced > 0) {
        assert.equal(conn.delivered.at(-1), conn.produced, `性質 10: 最新の版を捨てない ${where()}`);
      }
    }],
  ];
  const total = ops.reduce((sum, [, weight]) => sum + weight, 0);
  const pick = () => {
    let roll = rand() * total;
    for (const entry of ops) {
      roll -= entry[1];
      if (roll < 0) return entry;
    }
    return ops[0]!;
  };

  for (step = 0; step < steps; step += 1) {
    const [name, , run] = pick();
    op = name;
    await run();
    await flush();
    for (const conn of conns) {
      const stream = conn.relay.stream;
      if (conn.status === "rejected") {
        // 性質 4: 断った接続は gRPC を開かない。
        assert.equal(stream, undefined, `性質 4 ${where()}`);
        assert.equal(conn.relay.body, undefined, where());
        continue;
      }
      assert.ok(stream !== undefined, where());
      // 性質 1: gRPC の cancel と destroy は 1 回まで。
      assert.ok(stream.cancels <= 1 && stream.destroys <= 1, `性質 1 ${where()}`);
      // 性質 2・3・5・7: 閉じるのは、模型が閉じた接続だけ（自分の出来事か、同じ画面の新しい試行）。
      assert.equal(stream.cancels, conn.expectCancels, `接続 ${conn.id} の cancel の回数 ${where()}`);
      assert.equal(stream.destroys, conn.expectCancels, `接続 ${conn.id} の destroy の回数 ${where()}`);
    }
    for (const screen of [SCREEN_A, SCREEN_B]) {
      const open = conns.filter((c) => c.ticket?.screen === screen && c.status === "open");
      assert.ok(open.length <= 1, `性質 2: 画面ごとに開いているのは 1 本まで ${where()}`);
      // 性質 3: 開いている接続より後に開いた同じ画面の接続（断ったものを除く）は無い。
      if (open[0] !== undefined) {
        const later = conns.filter((c) => c.ticket?.screen === screen && c.id > open[0]!.id && c.status !== "rejected");
        assert.equal(later.length, 0, `性質 3 ${where()}`);
      }
    }
    // 性質 6: 登録簿は開いている接続の分だけ。
    assert.equal(screens.size(), registry.size, `性質 6 ${where()}`);
  }
  for (const conn of conns) {
    conn.relay.abort.abort();
  }
};

test("性質: 200 系列 × 60 手のどの並びでも、付け替え・断り・間引き・後始末の不変条件を守る", async () => {
  for (let seed = 1; seed <= 200; seed += 1) {
    await runRelayMachine(0x385 * 1000 + seed, 60);
  }
});

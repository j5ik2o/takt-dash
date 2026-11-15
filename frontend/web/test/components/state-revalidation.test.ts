// 状態受信時の画面再取得の契約（issue #15）。
//
// 同じ接続で状態を受信するたびに、RSC で取得している画面を再取得する契機を
// 呼ぶ。切断時は状態を置き換えず、再取得も促さない。
import test from "node:test";
import assert from "node:assert/strict";
import { createStateHandlers, openStateStream, type EventSourceLike } from "../../src/components/state-stream.ts";

class FakeEventSource implements EventSourceLike {
  static instances: FakeEventSource[] = [];
  onmessage: ((event: { data: string }) => void) | null = null;
  onerror: ((event: unknown) => void) | null = null;
  closed = false;
  constructor(public readonly url: string) {
    FakeEventSource.instances.push(this);
  }
  close() {
    this.closed = true;
  }
}

type ScheduledReconnect = {
  run: () => void;
  delayMs: number;
  cancelled: boolean;
};

/** 再接続の予約を記録し、テストが任意の時点で実行する。 */
class FakeScheduler {
  readonly scheduled: ScheduledReconnect[] = [];
  readonly schedule = (run: () => void, delayMs: number): (() => void) => {
    const entry: ScheduledReconnect = { run, delayMs, cancelled: false };
    this.scheduled.push(entry);
    return () => {
      entry.cancelled = true;
    };
  };
  latest(): ScheduledReconnect {
    const entry = this.scheduled[this.scheduled.length - 1];
    assert.ok(entry, "再接続が予約されています");
    return entry;
  }
  runLatest() {
    const entry = this.latest();
    if (!entry.cancelled) {
      entry.run();
    }
  }
}

test("同じ接続で状態を受信するたびに画面の再取得を促す", () => {
  FakeEventSource.instances = [];
  const events: string[] = [];
  const handlers = createStateHandlers({
    onState: (state) => events.push(`状態:${state.servers.length}`),
    onError: () => events.push("切断"),
    revalidate: () => events.push("再取得"),
  });

  const stream = openStateStream({
    url: "/api/state",
    createEventSource: (url: string) => new FakeEventSource(url),
    ...handlers,
  });
  const source = FakeEventSource.instances[0]!;
  source.onmessage?.({ data: JSON.stringify({ servers: [{ serverId: "mini", name: "Mac mini" }] }) });
  source.onmessage?.({ data: JSON.stringify({ servers: [] }) });

  assert.deepEqual(events, ["状態:1", "再取得", "状態:0", "再取得"]);
  stream.close();
});

test("切断時は画面の再取得を促さず、待機した後に再接続する", () => {
  FakeEventSource.instances = [];
  const events: string[] = [];
  const scheduler = new FakeScheduler();
  const handlers = createStateHandlers({
    onState: () => events.push("状態"),
    onError: () => events.push("切断"),
    revalidate: () => events.push("再取得"),
  });

  const stream = openStateStream({
    url: "/api/state",
    createEventSource: (url: string) => new FakeEventSource(url),
    scheduleReconnect: scheduler.schedule,
    ...handlers,
  });
  const first = FakeEventSource.instances[0]!;
  first.onerror?.({});

  assert.deepEqual(events, ["切断"], "切断時に画面の再取得を促しません");
  assert.equal(first.closed, true, "エラーになった接続は閉じます");
  assert.equal(FakeEventSource.instances.length, 1, "エラーの直後には再接続しません");
  assert.equal(scheduler.latest().delayMs, 1000, "最初の再接続は1秒待ちます");
  scheduler.runLatest();
  assert.equal(FakeEventSource.instances.length, 2, "予約した待機の後に再接続します");
  stream.close();
});

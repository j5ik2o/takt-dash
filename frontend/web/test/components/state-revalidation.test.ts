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

test("切断時は画面の再取得を促さない", () => {
  FakeEventSource.instances = [];
  const events: string[] = [];
  const handlers = createStateHandlers({
    onState: () => events.push("状態"),
    onError: () => events.push("切断"),
    revalidate: () => events.push("再取得"),
  });

  const stream = openStateStream({
    url: "/api/state",
    createEventSource: (url: string) => new FakeEventSource(url),
    ...handlers,
  });
  const source = FakeEventSource.instances[0]!;
  source.onerror?.({});

  assert.deepEqual(events, ["切断"]);
  assert.equal(FakeEventSource.instances.length, 2, "切断後も再接続します");
  stream.close();
});

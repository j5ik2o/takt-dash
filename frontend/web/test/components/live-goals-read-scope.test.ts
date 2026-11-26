import test from "node:test";
import assert from "node:assert/strict";
import { act, createElement, type ComponentProps } from "react";
import { createRoot } from "react-dom/client";
import { JSDOM } from "jsdom";
import { DashboardState } from "../../src/proto/taktdash.ts";
import { LiveGoals } from "../../src/components/live-goals.tsx";
import { StateStreamProvider } from "../../src/components/state-provider.tsx";
import { createHealthyDashboardState } from "../support/fixtures.ts";
import { parseStreamUrl } from "../support/stream-url.ts";

class FakeEventSource {
  static instances: FakeEventSource[] = [];
  onmessage: ((event: { data: string }) => void) | null = null;
  onerror: ((event: unknown) => void) | null = null;
  closed = false;
  constructor(public readonly url: string) {
    FakeEventSource.instances.push(this);
  }
  close(): void {
    this.closed = true;
  }
}

test("削除済みURLのライブ画面は初回取得失敗から正常な対象なしへ同じ購読で回復する", async () => {
  const dom = new JSDOM("<!doctype html><html><body></body></html>", { url: "http://localhost/goals?project=esa" });
  const replacements = {
    window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    EventSource: FakeEventSource, IS_REACT_ACT_ENVIRONMENT: true,
  };
  const descriptors = new Map(Object.keys(replacements).map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(replacements)) {
    Object.defineProperty(globalThis, key, { value, configurable: true, writable: true });
  }
  const container = dom.window.document.createElement("div");
  dom.window.document.body.append(container);
  const root = createRoot(container);
  try {
    const props: ComponentProps<typeof StateStreamProvider> = {
      initialState: DashboardState.create({}), initialSnapshotLoaded: false, initialFailure: "unreachable", initialRespondedBefore: false,
      children: createElement(LiveGoals, { project: "esa" }),
    };
    await act(async () => root.render(createElement(StateStreamProvider, props)));
    assert.equal(container.querySelectorAll('[role="alert"]').length, 1);
    assert.match(container.textContent ?? "", /分かりません/);
    assert.doesNotMatch(container.textContent ?? "", /ゴールはありません/);
    const source = FakeEventSource.instances[0];
    assert.ok(source);
    assert.equal(parseStreamUrl(source.url).path, "/api/state");
    const base = createHealthyDashboardState();
    const removed = DashboardState.create({ ...base,
      projects: base.projects.filter((project) => project.name !== "esa"),
      goals: base.goals.filter((goal) => goal.project !== "esa"),
    });
    await act(async () => source.onmessage?.({ data: JSON.stringify(DashboardState.toJSON(removed)) }));
    assert.equal(container.querySelectorAll('[role="alert"]').length, 0);
    assert.equal((container.textContent ?? "").match(/ゴールはありません/g)?.length, 1);
    assert.doesNotMatch(container.textContent ?? "", /まだ読めていない|分かりません/);
    await act(async () => source.onerror?.({}));
    assert.equal(container.querySelectorAll('[role="alert"]').length, 1);
    assert.equal((container.textContent ?? "").match(/ゴールはありません/g)?.length, 1);
    assert.doesNotMatch(container.textContent ?? "", /まだ読めていない|分かりません/);
    assert.equal(source.closed, true);
    await act(async () => new Promise((resolve) => setTimeout(resolve, 1100)));
    const reconnected = FakeEventSource.instances[1];
    assert.ok(reconnected);
    await act(async () => reconnected.onmessage?.({ data: JSON.stringify(DashboardState.toJSON(removed)) }));
    assert.equal(container.querySelectorAll('[role="alert"]').length, 0);
    assert.equal((container.textContent ?? "").match(/ゴールはありません/g)?.length, 1);
    assert.equal(FakeEventSource.instances.length, 2);
    assert.equal(reconnected.closed, false);
  } finally {
    await act(async () => root.unmount());
    container.remove();
    dom.window.close();
    for (const [key, descriptor] of descriptors) {
      if (descriptor === undefined) Reflect.deleteProperty(globalThis, key);
      else Object.defineProperty(globalThis, key, descriptor);
    }
  }
  assert.equal(FakeEventSource.instances[0]?.closed, true);
  assert.equal(FakeEventSource.instances[1]?.closed, true);
});

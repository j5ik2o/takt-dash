import test, { after } from "node:test";
import assert from "node:assert/strict";
import { act, createElement, type ComponentProps } from "react";
import { renderToString } from "react-dom/server";
import { JSDOM } from "jsdom";
import DecisionsPage from "../../app/decisions/page.tsx";
import { LiveDecisions } from "../../src/components/live-decisions.tsx";
import { StateStreamProvider } from "../../src/components/state-provider.tsx";
import { DashboardState } from "../../src/proto/taktdash.ts";
import { createActionableDashboardState, NOW } from "../support/fixtures.ts";
import {
  BACKEND_UNREACHABLE_LONG_MESSAGE,
  BACKEND_UNREACHABLE_MESSAGE,
} from "../../src/components/backend-unavailable-notice.tsx";

const dom = new JSDOM("<!doctype html><html><body></body></html>", { url: "http://localhost/" });
const scope = globalThis as unknown as Record<string, unknown>;
const previous = new Map<string, PropertyDescriptor | undefined>();
for (const [key, value] of Object.entries({
  window: dom.window,
  document: dom.window.document,
  navigator: dom.window.navigator,
  IS_REACT_ACT_ENVIRONMENT: true,
})) {
  previous.set(key, Object.getOwnPropertyDescriptor(globalThis, key));
  Object.defineProperty(globalThis, key, { value, configurable: true, writable: true });
}
// React の入力イベント判定はDOMを読み込む際に決まるため、DOMの用意後に読み込む。
const { createRoot, hydrateRoot } = await import("react-dom/client");

class FakeEventSource {
  static instances: FakeEventSource[] = [];
  onmessage: ((event: { data: string }) => void) | null = null;
  onerror: ((event: unknown) => void) | null = null;
  constructor(readonly url: string) {
    FakeEventSource.instances.push(this);
  }
  close(): void {}
}
previous.set("EventSource", Object.getOwnPropertyDescriptor(globalThis, "EventSource"));
scope.EventSource = FakeEventSource;
after(() => {
  dom.window.close();
  for (const [key, descriptor] of previous) {
    if (descriptor === undefined) {
      delete scope[key];
    } else {
      Object.defineProperty(globalThis, key, descriptor);
    }
  }
});

const stateAt = (createdAt: Date): DashboardState => {
  const state = createActionableDashboardState();
  const question = state.pendingDecisions.find((decision) => decision.inquiryId === "inquiry-text");
  assert.ok(question);
  return { ...state, pendingDecisions: [{ ...question, createdAt }] };
};

const provide = (
  state: DashboardState,
  children: ComponentProps<typeof StateStreamProvider>["children"],
) => {
  const props: ComponentProps<typeof StateStreamProvider> = { initialState: state, initialSnapshotLoaded: true, children };
  return createElement(StateStreamProvider, props);
};

test("秒境界をまたぐ初回描画でも回答欄を置換せず、表示直後の本文を保持する", async (context) => {
  context.mock.timers.enable({ apis: ["Date"], now: NOW.getTime() });
  const state = stateAt(new Date(NOW.getTime() - 3_000));
  const page = DecisionsPage({});
  const element = provide(state, page);
  const container = document.createElement("div");
  document.body.appendChild(container);
  container.innerHTML = renderToString(element);
  assert.equal(container.querySelector(".wait b")?.textContent, "3秒 待ち");
  const input = container.querySelector("input");
  assert.ok(input);
  assert.equal(container.querySelector<HTMLButtonElement>('button[type="submit"]')?.disabled, true);
  const errors: unknown[] = [];
  context.mock.timers.setTime(NOW.getTime() + 1_000);
  let root: ReturnType<typeof hydrateRoot> | undefined;
  try {
    await act(async () => {
      root = hydrateRoot(container, element, { onRecoverableError: (error) => errors.push(error) });
    });
    await act(async () => {
      input.focus();
      const setter = Object.getOwnPropertyDescriptor(dom.window.HTMLInputElement.prototype, "value")?.set;
      assert.ok(setter);
      setter.call(input, "表示直後に入力した回答本文");
      input.dispatchEvent(new dom.window.Event("input", { bubbles: true }));
    });
    assert.deepEqual(errors, []);
    assert.equal(container.querySelector("input"), input);
    assert.equal(input.value, "表示直後に入力した回答本文");
    assert.equal(container.querySelector<HTMLButtonElement>('button[type="submit"]')?.disabled, false);
    assert.equal(container.querySelector(".wait b")?.textContent, "4秒 待ち");
  } finally {
    await act(async () => root?.unmount());
    container.remove();
  }
});

test("同じ判断待ち画面の再描画で現在時刻を使い、待ち時間順と滞留の境界を更新する", async (context) => {
  context.mock.timers.enable({ apis: ["Date"], now: NOW.getTime() });
  const state = stateAt(new Date(NOW.getTime() - 59_999));
  const question = state.pendingDecisions[0]!;
  state.pendingDecisions = [
    { ...question, inquiryId: "missing", body: "時刻のない質問", createdAt: undefined },
    question,
    { ...question, inquiryId: "older", body: "先に届いた質問", createdAt: new Date(NOW.getTime() - 120_000) },
  ];
  state.thresholds = { ...state.thresholds!, pendingDecisionStaleMinutes: 1 };
  const element = provide(state, createElement(LiveDecisions, { initialNowMs: NOW.getTime() }));
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  try {
    await act(async () => root.render(element));
    const items = container.querySelectorAll<HTMLLIElement>(".tray-list > li");
    assert.match(items[0]!.textContent!, /先に届いた質問/);
    assert.match(items[1]!.textContent!, /補足を入力/);
    assert.equal(items[1]!.classList.contains("stale"), false);
    assert.equal(items[2]!.querySelector(".wait b")?.textContent, "0秒 待ち");
    context.mock.timers.setTime(NOW.getTime() + 1);
    const source = FakeEventSource.instances.at(-1)!;
    await act(async () => source.onmessage?.({ data: JSON.stringify(DashboardState.toJSON(state)) }));
    const updated = container.querySelectorAll<HTMLLIElement>(".tray-list > li");
    assert.equal(updated[1], items[1]);
    assert.equal(updated[1]!.classList.contains("stale"), true);
    assert.equal(updated[1]!.querySelector(".wait b")?.textContent, "1分 待ち");
    assert.equal(updated[2]!.querySelector(".wait b")?.textContent, "0秒 待ち");
  } finally {
    await act(async () => root.unmount());
    container.remove();
  }
});

test("同じ切断中の画面で60秒の通知境界を更新し、最終読取時刻を保持する", async (context) => {
  context.mock.timers.enable({ apis: ["Date"], now: NOW.getTime() });
  const state = stateAt(new Date(NOW.getTime() - 3_000));
  const children = createElement(LiveDecisions, { initialNowMs: NOW.getTime() });
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  try {
    await act(async () => root.render(provide(state, children)));
    const source = FakeEventSource.instances.at(-1)!;
    await act(async () => source.onerror?.({}));
    const readAtNote = () => Array.from(container.querySelectorAll(".page-head .stale-note"))
      .find((note) => note.textContent?.includes("最終読み取り時刻"))?.textContent;
    const readAt = readAtNote();
    assert.match(readAt!, /最終読み取り時刻/);
    context.mock.timers.setTime(NOW.getTime() + 59_999);
    await act(async () => root.render(provide(state, createElement(LiveDecisions, { initialNowMs: NOW.getTime() }))));
    assert.equal(container.querySelector('[role="alert"]')?.textContent, BACKEND_UNREACHABLE_MESSAGE);
    context.mock.timers.setTime(NOW.getTime() + 60_000);
    await act(async () => root.render(provide(state, createElement(LiveDecisions, { initialNowMs: NOW.getTime() }))));
    assert.equal(container.querySelector('[role="alert"]')?.textContent, BACKEND_UNREACHABLE_LONG_MESSAGE);
    assert.equal(readAtNote(), readAt);
  } finally {
    await act(async () => root.unmount());
    container.remove();
  }
});

import test from "node:test";
import assert from "node:assert/strict";
import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import { JSDOM } from "jsdom";
import { LiveRunLog } from "../../src/components/live-run-log.tsx";

test("ログ更新応答の省略記号をログ行と直近の道具名に表示する", async () => {
  const dom = new JSDOM("<div id='root'></div>");
  const globals = { window: dom.window, document: dom.window.document, IS_REACT_ACT_ENVIRONMENT: true };
  const descriptors = new Map(Object.keys(globals).map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(globals)) {
    Object.defineProperty(globalThis, key, { value, configurable: true, writable: true });
  }
  const originalFetch = globalThis.fetch;
  const tool = `${"道".repeat(511)}…`;
  const line = `step_complete ${"あ".repeat(511)}…`;
  globalThis.fetch = async () => Response.json({
    epoch: 1, reset: true, lines: [{ sequence: 1, text: line }],
    activity: { latestTool: tool, toolUseCount: 1 }, logsRemoved: false,
  });
  const container = dom.window.document.getElementById("root");
  assert.ok(container);
  const root = createRoot(container);
  try {
    await act(async () => {
      root.render(createElement(LiveRunLog, { runSlug: "r1", logTail: "", logsRemoved: false }));
    });
    assert.equal(container.querySelector("ol li")?.textContent, line);
    assert.ok(container.querySelector("p.note")?.textContent?.includes(`直近の道具は ${tool} です。`));
  } finally {
    await act(async () => root.unmount());
    globalThis.fetch = originalFetch;
    for (const [key, descriptor] of descriptors) {
      if (descriptor === undefined) Reflect.deleteProperty(globalThis, key);
      else Object.defineProperty(globalThis, key, descriptor);
    }
    dom.window.close();
  }
});

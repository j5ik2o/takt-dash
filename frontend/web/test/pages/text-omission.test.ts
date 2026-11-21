import test from "node:test";
import assert from "node:assert/strict";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { JSDOM } from "jsdom";
import { ActivityContent } from "../../src/components/activity-content.tsx";
import { DecisionsContent } from "../../src/components/decisions-content.tsx";
import { RunDetailContent } from "../../src/components/run-detail-content.tsx";
import { ActivityKind, PendingDecisionType } from "../../src/proto/taktdash.ts";
import { createActivityResponse, createActionableDashboardState, createDashboardState, createRunDetailResponse, NOW } from "../support/fixtures.ts";

test("保存済み活動の本文の省略記号を表示する", () => {
  const body = "問い合わせ「確認。…」を受け付けました。";
  const response = { ...createActivityResponse(), events: [{
    occurredAt: NOW, serverId: "mini", project: "corporate-site", kind: ActivityKind.ACTIVITY_KIND_INQUIRY, body,
  }] };
  const html = renderToStaticMarkup(createElement(ActivityContent, { state: createDashboardState(), response, now: NOW }));
  assert.ok(new JSDOM(html).window.document.body.textContent?.includes(body));
});

test("判断待ちの質問本文に渡された省略記号を保持する", () => {
  const base = createActionableDashboardState();
  const inquiry = base.pendingDecisions.find((item) => item.type === PendingDecisionType.PENDING_DECISION_TYPE_INQUIRY);
  assert.ok(inquiry);
  const state = { ...base, pendingDecisions: [{ ...inquiry, body: "確認。…" }] };
  const html = renderToStaticMarkup(createElement(DecisionsContent, { state, snapshotLoaded: true, now: NOW, disconnected: false }));
  assert.ok(new JSDOM(html).window.document.body.textContent?.includes("確認。…"));
});

test("run詳細は省略された要約とタスク全文を別々に表示する", () => {
  const taskText = `確認する。\n${"あ".repeat(3000)}本文末尾。`;
  const detail = { ...createRunDetailResponse(), summary: "確認する。…", taskText };
  const html = renderToStaticMarkup(createElement(RunDetailContent, { detail, now: NOW }));
  const text = new JSDOM(html).window.document.body.textContent;
  assert.ok(text?.includes("確認する。…"));
  assert.ok(text?.includes(taskText));
});

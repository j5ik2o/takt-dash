import test from "node:test";
import assert from "node:assert/strict";
import { createElement, type ComponentProps } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { JSDOM } from "jsdom";
import { OverviewContent } from "../../src/components/overview-content.tsx";
import { DecisionsContent } from "../../src/components/decisions-content.tsx";
import { GoalsContent } from "../../src/components/goals-content.tsx";
import { Sidebar } from "../../src/components/sidebar.tsx";
import { DashboardState, Project, Server } from "../../src/proto/taktdash.ts";
import { buildOverviewView } from "../../src/state/dashboard.ts";
import { dashboardStateFromProto } from "../../src/state/restore-state.ts";
import { createDashboardState, createHealthyDashboardState, NOW } from "../support/fixtures.ts";

const documentOf = (element: Parameters<typeof renderToStaticMarkup>[0]): Document =>
  new JSDOM(renderToStaticMarkup(element)).window.document;
const required = (root: ParentNode, selector: string): Element => {
  const element = root.querySelector(selector);
  assert.ok(element, selector);
  return element;
};
const text = (element: Element): string => element.textContent?.replace(/\s+/g, " ").trim() ?? "";

const renderGoals = (state: DashboardState, props: Omit<ComponentProps<typeof GoalsContent>, "state">): Document =>
  documentOf(createElement(GoalsContent, { state: dashboardStateFromProto(state), ...props }));

for (const populated of [false, true]) {
  test(`配下のない表示用登録を追加しても${populated ? "非空" : "空"}の全体件数は既読の供給元で決まる`, () => {
    const base = createHealthyDashboardState();
    const state = DashboardState.create({ ...base,
      servers: base.servers.filter((server) => server.serverId === "mini"),
      runs: populated ? base.runs : [], goals: populated ? base.goals : [],
      pendingDecisions: populated ? createDashboardState().pendingDecisions.slice(0, 1) : [],
    });
    const extra = Server.create({ serverId: "metadata-only", name: "表示用サーバー", connected: false });
    for (const candidate of [state, DashboardState.create({ ...state, servers: [...state.servers, extra] })]) {
      const view = buildOverviewView(candidate, NOW, true);
      assert.equal(view.coverage, "complete");
      const overview = documentOf(createElement(OverviewContent, { view, disconnected: false }));
      const decisions = documentOf(createElement(DecisionsContent, { snapshotLoaded: true, state: candidate, now: NOW, disconnected: false }));
      const goals = renderGoals(candidate, { snapshotLoaded: true, disconnected: false });
      for (const page of [overview, decisions, goals]) {
        assert.doesNotMatch(text(required(page, ".page-head")), /まだ読めていない|読めた範囲|読めた分/);
      }
      if (populated) {
        assert.equal(overview.querySelectorAll(".run-row").length, 1);
        assert.match(text(required(decisions, ".page-head")), /1件/);
        assert.match(text(required(goals, ".page-head")), /2件/);
        assert.ok(goals.querySelector('a[href="/goals/goal-1"]'));
      } else {
        assert.match(text(required(overview, '[aria-labelledby="h-runs"]')), /走っている run はありません/);
        assert.equal(text(required(decisions, "main")).match(/判断待ちはありません/g)?.length, 1);
        assert.equal(text(required(goals, "main")).match(/ゴールはありません/g)?.length, 1);
      }
    }
  });
}

test("サーバー行は配下なし、未読、既読ゼロを区別して接続表示を保つ", () => {
  const server = Server.create({ serverId: "mini", name: "Mac mini", connected: false });
  const unread = Project.create({ name: "esa", serverId: "mini" });
  const build = { commit: "test", branch: "test" };
  for (const [projects, countPattern] of [
    [[], /run\s*対象なし/],
    [[unread], /run\s*不明/],
    [[{ ...unread, lastReadAt: NOW }], /run\s*0件/],
  ] satisfies Array<[Project[], RegExp]>) {
    const page = documentOf(createElement(Sidebar, {
      snapshotLoaded: true,
      servers: [server], projects, runs: [], pendingCount: 0, frontendBuild: build, backendBuild: build,
    }));
    const row = required(page, ".srv-list li");
    assert.match(text(required(row, ".cnt")), countPattern);
    assert.equal(text(required(row, ".srv-state")), "応答なし");
    assert.ok(row.querySelector('a[href="/#server-mini"]'));
  }
  const omitted = documentOf(createElement(Sidebar, {
    snapshotLoaded: true,
    servers: [server], runs: [], pendingCount: 0, frontendBuild: build, backendBuild: build,
  }));
  assert.match(text(required(omitted, ".cnt")), /run\s*不明/);
});

test("削除前のゴールURLは正常応答の対象なしと実在する未読対象を区別する", () => {
  const base = createHealthyDashboardState();
  const removed = DashboardState.create({ ...base,
    projects: base.projects.filter((project) => project.name !== "esa"),
    goals: base.goals.filter((goal) => goal.project !== "esa"),
  });
  const page = renderGoals(removed, { snapshotLoaded: true, project: "esa", disconnected: false });
  assert.equal(text(required(page, "main")).match(/ゴールはありません/g)?.length, 1);
  assert.doesNotMatch(text(required(page, "main")), /まだ読めていない|分かりません/);
  const unread = DashboardState.create({ ...removed,
    projects: [...removed.projects, Project.create({ name: "esa", serverId: "mini" })],
  });
  const unreadPage = renderGoals(unread, { snapshotLoaded: true, project: "esa", disconnected: false });
  assert.match(text(required(unreadPage, "main")), /分かりません/);
  assert.doesNotMatch(text(required(unreadPage, "main")), /ゴールはありません/);
});

test("初回概要取得失敗の空配列は他のRPCの応答履歴があっても対象なしと断定しない", () => {
  const state = DashboardState.create({});
  for (const respondedBefore of [false, true]) {
    const failed = renderGoals(state, { snapshotLoaded: false, project: "esa", disconnected: true,
      unavailableSituation: { failure: "unreachable", respondedBefore },
    });
    assert.ok(failed.querySelector('[role="alert"]'));
    assert.match(text(required(failed, "main")), /分かりません/);
    assert.doesNotMatch(text(required(failed, "main")), /ゴールはありません/);
  }
});

test("概要ヘッダーは未取得の空snapshotと正常取得したサーバー0台を区別する", () => {
  const state = DashboardState.create({});
  for (const snapshotLoaded of [false, true]) {
    const page = documentOf(createElement(OverviewContent, {
      view: buildOverviewView(state, NOW, snapshotLoaded), disconnected: !snapshotLoaded,
      unavailableSituation: { failure: "unreachable", respondedBefore: true },
    }));
    const head = text(required(page, ".page-head"));
    const runs = text(required(page, '[aria-labelledby="h-runs"]'));
    if (snapshotLoaded) {
      assert.match(head, /0台のサーバーを集約/);
      assert.equal(runs.match(/走っている run はありません/g)?.length, 1);
      assert.doesNotMatch(head, /不明|0件の run/);
    } else {
      assert.match(head, /サーバーの台数は不明.*run は不明/);
      assert.doesNotMatch(head, /0台|0件/);
      assert.match(runs, /分かりません/);
      assert.doesNotMatch(runs, /run はありません/);
    }
  }
});

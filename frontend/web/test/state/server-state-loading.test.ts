import test from "node:test";
import assert from "node:assert/strict";
import * as grpc from "@grpc/grpc-js";
import { createElement, type ComponentProps } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { JSDOM } from "jsdom";
import { closeBackendClient, getBackendClient, hasBackendResponded, resetBackendResponded } from "../../src/grpc/client.ts";
import { DashboardState } from "../../src/proto/taktdash.ts";
import { loadServerState, type ServerState } from "../../src/state/server-state.ts";
import { formatClock } from "../../src/state/format.ts";
import { StateStreamProvider } from "../../src/components/state-provider.tsx";
import { LiveOverview } from "../../src/components/live-overview.tsx";
import { LiveDecisions } from "../../src/components/live-decisions.tsx";
import { LiveGoals } from "../../src/components/live-goals.tsx";
import { createDashboardState, createHealthyDashboardState, createProjectsResponse, NOW } from "../support/fixtures.ts";
import { startTaktDashServer, type ServerBehavior } from "../support/grpc-server.ts";

const renderLoaded = (loaded: ServerState): Document => {
  const props: ComponentProps<typeof StateStreamProvider> = {
    initialState: loaded.state,
    initialFailure: loaded.error?.failure,
    initialSnapshotLoaded: loaded.snapshotLoaded,
    initialRespondedBefore: hasBackendResponded(),
    children: [
      createElement("div", { id: "overview", key: "overview" }, createElement(LiveOverview, { initialNowMs: NOW.getTime() })),
      createElement("div", { id: "decisions", key: "decisions" }, createElement(LiveDecisions)),
      createElement("div", { id: "goals", key: "goals" }, createElement(LiveGoals)),
      createElement("div", { id: "removed-goals", key: "removed-goals" }, createElement(LiveGoals, { project: "esa" })),
    ],
  };
  return new JSDOM(renderToStaticMarkup(createElement(StateStreamProvider, props))).window.document;
};

const required = (root: ParentNode, selector: string): Element => {
  const element = root.querySelector(selector);
  assert.ok(element, selector);
  return element;
};

test("概要ローダーは初回未取得と取得済み空・非空の再訪キャッシュを3画面まで伝える", async () => {
  const unavailable = { code: grpc.status.UNAVAILABLE, details: "backend unavailable" };
  const calls: string[] = [];
  const behavior: ServerBehavior = { overview: unavailable, projects: createProjectsResponse(), calls };
  const server = await startTaktDashServer(behavior);
  const previousAddress = process.env.TAKTDASH_BACKEND_ADDR;
  await closeBackendClient();
  resetBackendResponded();
  process.env.TAKTDASH_BACKEND_ADDR = server.address;
  try {
    const first = await loadServerState();
    assert.equal(first.snapshotLoaded, false);
    assert.ok(first.error);
    const firstPage = renderLoaded(first);
    for (const id of ["overview", "decisions", "goals", "removed-goals"]) {
      const page = required(firstPage, `#${id}`);
      assert.equal(page.querySelectorAll('[role="alert"]').length, 1);
      assert.match(page.textContent ?? "", /不明|分かりません/);
    }

    await getBackendClient().getProjects();
    assert.equal(hasBackendResponded(), true);
    const otherRpcOnly = await loadServerState();
    assert.equal(otherRpcOnly.snapshotLoaded, false);
    const otherRpcPage = renderLoaded(otherRpcOnly);
    assert.doesNotMatch(required(otherRpcPage, '#overview [aria-labelledby="h-runs"]').textContent ?? "", /run\s*はありません/);
    assert.doesNotMatch(required(otherRpcPage, "#decisions").textContent ?? "", /判断待ちはありません/);
    assert.doesNotMatch(required(otherRpcPage, "#goals").textContent ?? "", /ゴールはありません/);
    assert.doesNotMatch(required(otherRpcPage, "#removed-goals").textContent ?? "", /ゴールはありません/);

    behavior.overview = DashboardState.create({});
    const empty = await loadServerState();
    assert.equal(empty.snapshotLoaded, true);
    assert.equal(empty.error, undefined);
    const assertEmpty = (document: Document, disconnected: boolean): void => {
      assert.match(required(document, '#overview [aria-labelledby="h-runs"]').textContent ?? "", /走っている run はありません/);
      assert.match(required(document, "#decisions").textContent ?? "", /判断待ちはありません/);
      assert.match(required(document, "#goals").textContent ?? "", /ゴールはありません/);
      assert.match(required(document, "#removed-goals").textContent ?? "", /ゴールはありません/);
      for (const id of ["overview", "decisions", "goals", "removed-goals"]) {
        assert.equal(required(document, `#${id}`).querySelectorAll('[role="alert"]').length, disconnected ? 1 : 0);
      }
    };
    assertEmpty(renderLoaded(empty), false);

    behavior.overview = unavailable;
    const cachedEmpty = await loadServerState();
    assert.equal(cachedEmpty.snapshotLoaded, true);
    assert.ok(cachedEmpty.error);
    assertEmpty(renderLoaded(cachedEmpty), true);

    behavior.overview = DashboardState.create({
      ...createHealthyDashboardState(), pendingDecisions: createDashboardState().pendingDecisions,
    });
    const populated = await loadServerState();
    assert.equal(populated.snapshotLoaded, true);
    behavior.overview = unavailable;
    const cachedPopulated = await loadServerState();
    assert.equal(cachedPopulated.snapshotLoaded, true);
    assert.ok(cachedPopulated.error);
    const populatedPage = renderLoaded(cachedPopulated);
    assert.ok(populatedPage.querySelector('#overview a[href="/runs/run-aaa"]'));
    assert.ok(populatedPage.querySelector('#goals a[href="/goals/goal-1"]'));
    assert.equal(populatedPage.querySelectorAll("#decisions .tray-item").length, 2);
    for (const id of ["overview", "decisions", "goals"]) {
      assert.equal(required(populatedPage, `#${id}`).querySelectorAll('[role="alert"]').length, 1);
    }
    assert.equal(calls.filter((call) => call === "GetOverview").length, 6, "Nodeの要求外ではcacheが各呼び出しを実行し、再訪の取得失敗を通る");
    assert.equal(calls.filter((call) => call === "GetProjects").length, 1);
  } finally {
    await closeBackendClient();
    resetBackendResponded();
    if (previousAddress === undefined) delete process.env.TAKTDASH_BACKEND_ADDR;
    else process.env.TAKTDASH_BACKEND_ADDR = previousAddress;
    await server.shutdown();
  }
});

test("成功時刻のある空キャッシュで再訪すると一覧内に元の観測時刻と過去の空結果を表示する", async () => {
  const observed = new Date("2026-11-13T12:00:00Z");
  const base = createHealthyDashboardState();
  const project = base.projects[0];
  assert.ok(project);
  const empty = DashboardState.create({ ...base, readAt: observed, runs: [], goals: [], pendingDecisions: [],
    servers: base.servers.filter((server) => server.serverId === project.serverId)
      .map((server) => ({ ...server, connected: true, lastReadAt: observed })),
    projects: [{ ...project, connected: true, lastReadAt: observed }],
  });
  const behavior: ServerBehavior = { overview: empty };
  const server = await startTaktDashServer(behavior);
  const previousAddress = process.env.TAKTDASH_BACKEND_ADDR;
  await closeBackendClient();
  resetBackendResponded();
  process.env.TAKTDASH_BACKEND_ADDR = server.address;
  try {
    const success = renderLoaded(await loadServerState());
    const normal = required(success, '#overview [aria-labelledby="h-runs"]').textContent ?? "";
    assert.match(normal, /走っている run はありません。/);
    assert.doesNotMatch(normal, /ありませんでした/);
    behavior.overview = { code: grpc.status.UNAVAILABLE, details: "backend unavailable" };
    const cached = await loadServerState();
    assert.equal(cached.snapshotLoaded, true);
    const document = renderLoaded(cached);
    assert.equal(required(document, "#overview").querySelectorAll('[role="alert"]').length, 1);
    const content = required(document, '#overview [aria-labelledby="h-runs"]').textContent ?? "";
    assert.ok(content.includes(formatClock(observed)), content);
    assert.match(content, /走っている run はありませんでした。/);
    assert.equal(content.match(/走っている run はありません/g)?.length, 1);
  } finally {
    await closeBackendClient();
    resetBackendResponded();
    if (previousAddress === undefined) delete process.env.TAKTDASH_BACKEND_ADDR;
    else process.env.TAKTDASH_BACKEND_ADDR = previousAddress;
    await server.shutdown();
  }
});

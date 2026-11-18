import test from "node:test";
import assert from "node:assert/strict";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { JSDOM } from "jsdom";
import { OverviewContent } from "../../src/components/overview-content.tsx";
import { ConnectionState, DashboardState, Server } from "../../src/proto/taktdash.ts";
import { buildOverviewView } from "../../src/state/dashboard.ts";
import { formatClock } from "../../src/state/format.ts";
import { createHealthyDashboardState, NOW } from "../support/fixtures.ts";

const required = (root: ParentNode, selector: string): Element => {
  const element = root.querySelector(selector);
  assert.ok(element, selector);
  return element;
};
const text = (element: Element): string => element.textContent?.replace(/\s+/g, " ").trim() ?? "";
const render = (state: DashboardState, disconnected = false): Document => new JSDOM(renderToStaticMarkup(
  createElement(OverviewContent, { view: buildOverviewView(state, NOW, true), disconnected }),
)).window.document;
const healthy = (populated = true): DashboardState => {
  const base = createHealthyDashboardState();
  return DashboardState.create({ ...base,
    servers: base.servers.filter((server) => server.serverId === "mini"),
    runs: populated ? base.runs : [],
  });
};

for (const populated of [false, true]) {
  test(`供給元のない登録を加えても${populated ? "非空" : "空"}のrun件数の鮮度は変わらない`, () => {
    const base = healthy(populated);
    const added = DashboardState.create({ ...base, servers: [...base.servers,
      Server.create({ serverId: "metadata-only", name: "表示用サーバー", connected: false }),
    ] });
    for (const state of [base, added]) {
      const page = render(state);
      const head = required(page, ".page-head > div");
      const tile = required(page, ".tiles .tile");
      assert.doesNotMatch(text(head), /応答のない|最後に読めた|時刻は不明/);
      assert.doesNotMatch(text(tile), /応答のない|最後に読めた|時刻は不明/);
      if (populated) {
        assert.match(text(head), /1件の run が動いています/);
        assert.ok(page.querySelector('a[href="/runs/run-aaa"]'));
      } else {
        assert.equal(text(required(page, '[aria-labelledby="h-runs"]')).match(/走っている run はありません/g)?.length, 1);
      }
    }
    const page = render(added);
    assert.match(text(required(page, '[role="status"]')), /表示用サーバー から応答がありません/);
    assert.match(text(required(page, "#server-metadata-only")), /応答なし/);
  });
}

for (const failure of ["server", "project"] as const) {
  test(`供給${failure === "server" ? "サーバー" : "プロジェクト"}の応答停止は件数の取得時刻と主語を示す`, () => {
    const base = healthy();
    const observed = base.projects[0]!.lastReadAt!;
    const state = DashboardState.create({ ...base,
      servers: base.servers.map((server) => ({ ...server, connected: failure !== "server" })),
      projects: base.projects.map((project, index) => ({ ...project,
        connectionState: failure === "project" && index === 0
          ? ConnectionState.CONNECTION_STATE_UNRESPONSIVE : project.connectionState,
      })),
    });
    const page = render(state);
    for (const selector of [".page-head > div", ".tiles .tile"]) {
      const notes = text(required(page, selector));
      assert.match(notes, new RegExp(`応答のない${failure === "server" ? "サーバー" : "プロジェクト"}が`));
      assert.match(notes, /最後に読めた時点で 1 件/);
      assert.ok(notes.includes(`${formatClock(observed)} 時点`));
    }
    assert.equal(page.querySelectorAll(".run-row").length, 1);
  });
}

for (const failure of ["server", "project"] as const) {
  test(`応答なし${failure === "server" ? "サーバー" : "プロジェクト"}の空run一覧は一つの空表示で取得した時点を示す`, () => {
    const base = healthy(false);
    const observed = base.projects[0]!.lastReadAt!;
    const connected = render(base);
    assert.match(text(required(connected, '[aria-labelledby="h-runs"]')), /走っている run はありません。/);
    const failed = render(DashboardState.create({ ...base,
      servers: base.servers.map((server) => ({ ...server, connected: failure !== "server" })),
      projects: base.projects.map((project, index) => ({ ...project,
        connectionState: failure === "project" && index === 0
          ? ConnectionState.CONNECTION_STATE_UNRESPONSIVE : project.connectionState,
      })),
    }));
    const list = text(required(failed, '[aria-labelledby="h-runs"]'));
    assert.ok(list.includes(`${formatClock(observed)} 時点`));
    assert.match(list, /走っている run はありませんでした。/);
    assert.equal(list.match(/走っている run はありません/g)?.length, 1);
    for (const selector of [".page-head > div", ".tiles .tile"]) {
      assert.doesNotMatch(text(required(failed, selector)), /run.*0\s*件|0\s*\/|走っている run はありません/);
    }
  });
}

test("本番投影と同じサーバーと配下の同時停止でも空run一覧は成功時刻を一度だけ示す", () => {
  const base = healthy(false);
  const project = base.projects[0];
  assert.ok(project);
  const server = base.servers.find((item) => item.serverId === project.serverId);
  assert.ok(server);
  const observed = new Date("2026-11-13T12:00:00Z");
  const failedAt = new Date("2026-11-13T12:02:00Z");
  const connected = DashboardState.create({
    ...base, readAt: observed, goals: [], pendingDecisions: [],
    servers: [{ ...server, connected: true, lastReadAt: observed }],
    projects: [{ ...project, connected: true, lastReadAt: observed,
      connectionState: ConnectionState.CONNECTION_STATE_CONNECTED }],
  });
  const normalPage = render(connected);
  const normalList = text(required(normalPage, '[aria-labelledby="h-runs"]'));
  assert.match(normalList, /走っている run はありません。/);
  assert.doesNotMatch(normalList, /ありませんでした/);
  assert.equal(normalList.match(/走っている run はありません/g)?.length, 1);

  const unresponsive = DashboardState.create({ ...connected,
    servers: connected.servers.map((item) => ({ ...item, connected: false })),
    projects: connected.projects.map((item) => ({ ...item, connected: false,
      connectionState: ConnectionState.CONNECTION_STATE_UNRESPONSIVE })),
  });
  const page = render(unresponsive);
  const list = text(required(page, '[aria-labelledby="h-runs"]'));
  assert.ok(list.includes(`${formatClock(observed)} 時点`));
  assert.ok(!list.includes(`${formatClock(failedAt)} 時点`));
  assert.match(list, /走っている run はありませんでした。/);
  assert.equal(list.match(/走っている run はありません/g)?.length, 1);
  for (const selector of [".page-head > div", ".tiles .tile"]) {
    assert.doesNotMatch(text(required(page, selector)), /run.*0\s*件|0\s*\/|走っている run はありません/);
  }
});

for (const failure of ["server", "project", "web"] as const) {
  for (const timing of ["common", "different", "unread"] as const) {
    test(`${failure}切断時の空一覧は${timing}のプロジェクト取得範囲だけを示す`, () => {
      const base = healthy(false);
      const first = new Date("2026-11-13T12:00:00Z");
      const second = new Date("2026-11-13T12:05:00Z");
      const state = DashboardState.create({ ...base, readAt: second,
        servers: base.servers.map((server) => ({ ...server, connected: failure !== "server", lastReadAt: second })),
        projects: base.projects.map((project, index) => ({ ...project,
          lastReadAt: index === 0 ? first : timing === "common" ? new Date(first) : timing === "different" ? second : undefined,
          connectionState: failure === "project" && index === 0
            ? ConnectionState.CONNECTION_STATE_UNRESPONSIVE : project.connectionState,
        })),
      });
      const page = render(state, failure === "web");
      const list = text(required(page, '[aria-labelledby="h-runs"]'));
      if (timing === "unread") {
        assert.match(list, /あるかどうかは分かりません/);
        assert.doesNotMatch(list, /ありません/);
      } else {
        assert.equal(list.match(/走っている run はありません/g)?.length, 1);
        assert.match(list, /ありませんでした/);
        if (timing === "common") {
          assert.ok(list.includes(`${formatClock(first)} 時点`), list);
        } else {
          assert.match(list, /各プロジェクト.*最後に取得した結果/);
          assert.doesNotMatch(list, /時点/);
          for (const time of [first, second]) assert.ok(!list.includes(formatClock(time)), list);
        }
      }
      for (const selector of [".page-head > div", ".tiles .tile"]) {
        const content = text(required(page, selector));
        if (timing === "unread") assert.match(content, /読めた分/);
        else assert.doesNotMatch(content, /run.*0\s*件|0\s*\//);
        assert.doesNotMatch(content, /走っている run はありません/);
      }
    });
  }
}

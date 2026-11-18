// 状態取得の一本化とクライアント共有の契約（AC72F-1/AC72F-2）。
//
// layout の GetOverview を要求内で共有し、プロジェクトページはページ側 RPC を
// 持たないこと、アクティビティページは状態と GetActivity を 1 回ずつ呼ぶことを、
// ダミーサーバーへの呼び出し記録で観測する。連続実行ではプロセス共有クライアントを
// 作り直さないことも同じサーバーで確かめる。
import test from "node:test";
import assert from "node:assert/strict";
import { renderToStaticMarkup } from "react-dom/server";
import { createActivityResponse, createDashboardState, createProjectsResponse } from "../support/fixtures.ts";
import { startTaktDashServer } from "../support/grpc-server.ts";
import { closeBackendClient, getBackendClient } from "../../src/grpc/client.ts";
import { ProjectsLoader } from "../../src/components/projects-loader.tsx";
import { ActivityLoader } from "../../src/components/activity-loader.tsx";

const withServerEnv = async <T>(address: string, run: () => Promise<T>): Promise<T> => {
  await closeBackendClient();
  const previous = process.env.TAKTDASH_BACKEND_ADDR;
  process.env.TAKTDASH_BACKEND_ADDR = address;
  try {
    return await run();
  } finally {
    await closeBackendClient();
    if (previous === undefined) {
      delete process.env.TAKTDASH_BACKEND_ADDR;
    } else {
      process.env.TAKTDASH_BACKEND_ADDR = previous;
    }
  }
};

test("プロジェクトのローダーは GetOverview を1回だけ呼び、GetProjects を呼ばない", async () => {
  const calls: string[] = [];
  const server = await startTaktDashServer({
    overview: createDashboardState(),
    projects: createProjectsResponse(),
    calls,
  });
  try {
    await withServerEnv(server.address, async () => {
      const html = renderToStaticMarkup(await ProjectsLoader());
      assert.match(html, /corporate-site/);
      assert.match(html, /run-aaa/);
      assert.match(html, /判断待ち/);
    });
    assert.deepEqual(calls, ["GetOverview"], "状態は GetOverview に一本化します");
  } finally {
    await server.shutdown();
  }
});

test("アクティビティのローダーは GetOverview と GetActivity を1回ずつ呼ぶ", async () => {
  const calls: string[] = [];
  const server = await startTaktDashServer({
    overview: createDashboardState(),
    activity: createActivityResponse(),
    calls,
  });
  try {
    await withServerEnv(server.address, async () => {
      const html = renderToStaticMarkup(await ActivityLoader());
      assert.match(html, /人のマージ待ち/);
      assert.match(html, /Mac mini/, "サーバー名は共有した状態から解決します");
    });
    assert.deepEqual(calls, ["GetOverview", "GetActivity"]);
  } finally {
    await server.shutdown();
  }
});

test("同じローダーを2回実行してもバックエンドクライアントを作り直さない", async () => {
  const calls: string[] = [];
  const server = await startTaktDashServer({
    overview: createDashboardState(),
    projects: createProjectsResponse(),
    calls,
  });
  try {
    await withServerEnv(server.address, async () => {
      const first = getBackendClient();
      await ProjectsLoader();
      await ProjectsLoader();
      assert.equal(getBackendClient(), first, "2回目の要求でも同じクライアントを使います");
    });
    assert.deepEqual(calls, ["GetOverview", "GetOverview"], "どちらの要求も同じクライアントで処理します");
  } finally {
    await server.shutdown();
  }
});

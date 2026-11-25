// 切断しても、前に読めたページは見出しと内容を残し、その上に切断のバナーを出す
// （issue #309）。概要と判断待ちと同じ動きを、プロジェクト・アクティビティ・
// run・ゴール・タスク・会話のローダーでも確かめる。一度も読めていないページは
// unavailable.test.ts のとおり、バナーだけのままにする。
import test from "node:test";
import assert from "node:assert/strict";
import type { ReactElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { startTaktDashServer, type ServerBehavior } from "../support/grpc-server.ts";
import {
  createActivityResponse,
  createConversationResponse,
  createDashboardState,
  createGoalDetailResponse,
  createRunDetailResponse,
  createTaskDetailResponse,
} from "../support/fixtures.ts";
import { closeBackendClient } from "../../src/grpc/client.ts";
import { ActivityLoader } from "../../src/components/activity-loader.tsx";
import { ConversationsLoader } from "../../src/components/conversations-loader.tsx";
import { GoalDetailLoader } from "../../src/components/goal-detail-loader.tsx";
import { ProjectsLoader } from "../../src/components/projects-loader.tsx";
import { RunDetailLoader } from "../../src/components/run-detail-loader.tsx";
import { TaskDetailLoader } from "../../src/components/task-detail-loader.tsx";

const withBackendAddress = async <T>(address: string, run: () => Promise<T>): Promise<T> => {
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

// 1回目は backend が応答する状態で描画し、backend を止めてから同じページを描画し直す。
const renderBeforeAndAfterDisconnect = async (
  behavior: ServerBehavior,
  render: () => Promise<ReactElement>,
): Promise<{ before: string; after: string }> => {
  const server = await startTaktDashServer(behavior);
  let before: string;
  try {
    before = await withBackendAddress(server.address, async () => renderToStaticMarkup(await render()));
  } finally {
    await server.shutdown();
  }
  const after = await withBackendAddress(server.address, async () => renderToStaticMarkup(await render()));
  return { before, after };
};

const DISCONNECTED_BANNER = /<p class="banner warn" role="alert">/;

// 切断のバナーは、前に読めた内容より上に出す。
const assertKeptWithBanner = (
  { before, after }: { before: string; after: string },
  heading: RegExp,
  content: RegExp,
  banner: RegExp = DISCONNECTED_BANNER,
): void => {
  assert.doesNotMatch(before, banner, "接続中はバナーを出さない");
  assert.match(before, heading);
  assert.match(after, banner, "切断のバナーを出す");
  assert.match(after, heading, "見出しを残す");
  assert.match(after, content, "前に読めた内容を残す");
  assert.ok(after.search(banner) < after.search(content), "バナーは前の内容より上に出す");
};

test("切断してもプロジェクトは見出しと前の内容を残し、その上にバナーを出す", async () => {
  const result = await renderBeforeAndAfterDisconnect({ overview: createDashboardState() }, () => ProjectsLoader());
  assertKeptWithBanner(result, /<h1>プロジェクト<\/h1>/, /corporate-site/);
});

test("切断してもアクティビティは見出しと前の内容を残し、その上にバナーを出す", async () => {
  const result = await renderBeforeAndAfterDisconnect(
    { overview: createDashboardState(), activity: createActivityResponse() },
    () => ActivityLoader(),
  );
  assertKeptWithBanner(result, /<h1>アクティビティ<\/h1>/, /問い合わせが届きました。/);
});

test("切断しても run 詳細は見出しと前の内容を残し、その上にバナーを出す", async () => {
  const result = await renderBeforeAndAfterDisconnect({ runDetail: createRunDetailResponse("run-aaa") }, () =>
    RunDetailLoader({ params: Promise.resolve({ slug: "run-aaa" }) }),
  );
  assertKeptWithBanner(result, /<h1>採用ページのレイアウトを直す<\/h1>/, /レイアウトを確認しています。/);
});

test("切断してもゴール詳細は見出しと前の内容を残し、その上にバナーを出す", async () => {
  const result = await renderBeforeAndAfterDisconnect({ goalDetail: createGoalDetailResponse() }, () =>
    GoalDetailLoader({ params: Promise.resolve({ goalId: "goal-1" }) }),
  );
  assertKeptWithBanner(result, /<h1>ゴール goal-1<\/h1>/, /採用ページのレイアウトを直す目的です。/);
});

test("切断してもタスク詳細は見出しと前の内容を残し、その上にバナーを出す", async () => {
  const result = await renderBeforeAndAfterDisconnect({ taskDetail: createTaskDetailResponse() }, () =>
    TaskDetailLoader({
      params: Promise.resolve({ project: "corporate-site" }),
      searchParams: Promise.resolve({ task: "採用ページのレイアウトを直す", goal: "goal-1" }),
    }),
  );
  assertKeptWithBanner(result, /<h1>採用ページのレイアウトを直す<\/h1>/, /レイアウトを直して、確認を待っています。/);
});

test("切断しても会話は見出しと前の内容を残し、その上にバナーを出す", async () => {
  const result = await renderBeforeAndAfterDisconnect(
    { overview: createDashboardState(), conversation: createConversationResponse() },
    () => ConversationsLoader(),
  );
  // 会話は 1 秒更新の失敗と同じ取得失敗の通知を、見出しの下、発言の上に出す。
  assertKeptWithBanner(
    result,
    /<h1>会話<\/h1>/,
    /採用ページの見出しを直してください。/,
    /会話を取得できません: backend に接続できません。/,
  );
});

test("前に読めた run と違う run は、切断中は前の内容を出さずバナーだけにする", async () => {
  const server = await startTaktDashServer({ runDetail: createRunDetailResponse("run-ccc") });
  try {
    await withBackendAddress(server.address, async () =>
      renderToStaticMarkup(await RunDetailLoader({ params: Promise.resolve({ slug: "run-ccc" }) })),
    );
  } finally {
    await server.shutdown();
  }
  const html = await withBackendAddress(server.address, async () =>
    renderToStaticMarkup(await RunDetailLoader({ params: Promise.resolve({ slug: "run-ddd" }) })),
  );
  assert.match(html, /role="alert"/);
  assert.doesNotMatch(html, /<h1>/);
});

// 切断しても、前に読めたページは見出しと内容を残し、その上に切断のバナーを出す
// （issue #309）。概要と判断待ちと同じ動きを、プロジェクト・アクティビティ・
// run・ゴール・タスク・会話のローダーでも確かめる。一度も読めていないページは
// unavailable.test.ts のとおり、バナーだけのままにする。
import test, { mock } from "node:test";
import assert from "node:assert/strict";
import type { ReactElement } from "react";
import { NextRequest } from "next/server";
import { renderToStaticMarkup } from "react-dom/server";
import { startTaktDashServer, type ServerBehavior } from "../support/grpc-server.ts";
import {
  createActivityResponse,
  createConversationMessages,
  createConversationResponse,
  createDashboardState,
  createGoalDetailResponse,
  createRunDetailResponse,
  createTaskDetailResponse,
} from "../support/fixtures.ts";
import { closeBackendClient } from "../../src/grpc/client.ts";
import { formatClock } from "../../src/state/format.ts";
import { GET as getConversationUpdate } from "../../app/api/conversations/route.ts";
import { GET as getGoalUpdate } from "../../app/api/goals/[goalId]/route.ts";
import { GET as getTaskUpdate } from "../../app/api/tasks/[project]/route.ts";
import { ActivityLoader } from "../../src/components/activity-loader.tsx";
import { ConversationsLoader } from "../../src/components/conversations-loader.tsx";
import { GoalDetailLoader } from "../../src/components/goal-detail-loader.tsx";
import { ProjectsLoader } from "../../src/components/projects-loader.tsx";
import { RunDetailLoader } from "../../src/components/run-detail-loader.tsx";
import { RunDetailContent } from "../../src/components/run-detail-content.tsx";
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

// 1 秒更新（Route Handler）で読めた内容も覚え、切断中に開き直したときはそれを出す。
const renderAfterUpdateAndDisconnect = async (
  initial: ServerBehavior,
  updated: ServerBehavior,
  render: () => Promise<ReactElement>,
  update: () => Promise<Response>,
): Promise<string> => {
  const first = await startTaktDashServer(initial);
  try {
    await withBackendAddress(first.address, async () => renderToStaticMarkup(await render()));
  } finally {
    await first.shutdown();
  }
  const second = await startTaktDashServer(updated);
  try {
    await withBackendAddress(second.address, async () => {
      assert.equal((await update()).status, 200);
    });
  } finally {
    await second.shutdown();
  }
  return withBackendAddress(second.address, async () => renderToStaticMarkup(await render()));
};

test("切断中に開き直したゴール詳細は、1 秒更新で最後に読めた内容を出す", async () => {
  const html = await renderAfterUpdateAndDisconnect(
    { goalDetail: createGoalDetailResponse({ goalId: "goal-7", objective: "最初に読めた目的です。" }) },
    { goalDetail: createGoalDetailResponse({ goalId: "goal-7", objective: "更新で読めた目的です。" }) },
    () => GoalDetailLoader({ params: Promise.resolve({ goalId: "goal-7" }) }),
    () =>
      getGoalUpdate(new NextRequest("http://localhost/api/goals/goal-7"), {
        params: Promise.resolve({ goalId: "goal-7" }),
      }),
  );
  assert.match(html, DISCONNECTED_BANNER);
  assert.match(html, /更新で読めた目的です。/);
});

test("切断中に開き直したタスク詳細は、1 秒更新で最後に読めた内容を出す", async () => {
  const taskName = "求人一覧を直す";
  const html = await renderAfterUpdateAndDisconnect(
    { taskDetail: createTaskDetailResponse({ taskName, summary: "最初に読めた要約です。" }) },
    { taskDetail: createTaskDetailResponse({ taskName, summary: "更新で読めた要約です。" }) },
    () =>
      TaskDetailLoader({
        params: Promise.resolve({ project: "corporate-site" }),
        searchParams: Promise.resolve({ task: taskName, goal: "goal-1" }),
      }),
    () =>
      getTaskUpdate(
        new NextRequest(`http://localhost/api/tasks/corporate-site?task=${encodeURIComponent(taskName)}&goal=goal-1`),
        { params: Promise.resolve({ project: "corporate-site" }) },
      ),
  );
  assert.match(html, DISCONNECTED_BANNER);
  assert.match(html, /更新で読めた要約です。/);
});

test("切断中に開き直した会話は、1 秒更新で届いた発言も出す", async () => {
  const added = {
    seq: 4,
    occurredAt: new Date("2026-11-13T07:29:00Z"),
    speaker: createConversationMessages()[0]!.speaker,
    text: "更新で届いた発言です。",
  };
  const html = await renderAfterUpdateAndDisconnect(
    { overview: createDashboardState(), conversation: createConversationResponse() },
    { conversation: createConversationResponse({ messages: [added], nextAfterSeq: 4 }) },
    () => ConversationsLoader(),
    () =>
      getConversationUpdate(
        new NextRequest(
          "http://localhost/api/conversations?project=corporate-site&session_id=session-conv-1&after_seq=3&limit=200",
        ),
      ),
  );
  assert.match(html, /会話を取得できません/);
  assert.match(html, /採用ページの見出しを直してください。/);
  assert.match(html, /更新で届いた発言です。/);
});

test("切断中のアクティビティは、最終更新に描画した時刻ではなく読めた時刻を出す", async () => {
  const readAt = new Date("2026-11-13T01:00:00Z");
  const renderedAt = new Date("2026-11-13T03:30:00Z");
  mock.timers.enable({ apis: ["Date"], now: readAt });
  try {
    const server = await startTaktDashServer({ overview: createDashboardState(), activity: createActivityResponse() });
    try {
      await withBackendAddress(server.address, async () => renderToStaticMarkup(await ActivityLoader()));
    } finally {
      await server.shutdown();
    }
    mock.timers.setTime(renderedAt.getTime());
    const html = await withBackendAddress(server.address, async () => renderToStaticMarkup(await ActivityLoader()));
    assert.match(html, DISCONNECTED_BANNER);
    assert.ok(html.includes(`最終更新は ${formatClock(readAt)} です。`), "読めた時刻を出す");
    assert.ok(!html.includes(formatClock(renderedAt)), "描画した時刻を出さない");
  } finally {
    mock.timers.reset();
  }
});

// React は同じ位置に同じ型の要素があれば状態を保つ。接続中と切断中で run 詳細の
// 内容の位置と型をそろえ、切断から戻ったときにログの追従を作り直させない。
test("run 詳細は接続中と切断中で、内容を同じ位置に同じ型で置く", async () => {
  const contentOf = (element: ReactElement): unknown => {
    const props = element.props as { children?: ReactElement };
    return props.children?.type;
  };
  const server = await startTaktDashServer({ runDetail: createRunDetailResponse("run-eee") });
  let connected: ReactElement;
  try {
    connected = await withBackendAddress(server.address, () =>
      RunDetailLoader({ params: Promise.resolve({ slug: "run-eee" }) }),
    );
  } finally {
    await server.shutdown();
  }
  const disconnected = await withBackendAddress(server.address, () =>
    RunDetailLoader({ params: Promise.resolve({ slug: "run-eee" }) }),
  );
  assert.equal(connected.type, disconnected.type);
  assert.equal(contentOf(connected), RunDetailContent);
  assert.equal(contentOf(disconnected), RunDetailContent);
});

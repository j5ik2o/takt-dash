// ゴール系 Route Handler の契約（BFF の取得境界）。
//
// ブラウザは backend へ直接接続せず、Next.js の Route Handler が gRPC の応答を
// proto の JSON 表現で返す。見つからない対象は 404、backend 不通は 503 にする。
import test from "node:test";
import assert from "node:assert/strict";
import * as grpc from "@grpc/grpc-js";
import { NextRequest } from "next/server";
import {
  createGoalDetailResponse,
  createTaskDetailResponse,
} from "../support/fixtures.ts";
import { reserveClosedAddress, startTaktDashServer } from "../support/grpc-server.ts";
import { GET as getGoalDetail } from "../../app/api/goals/[goalId]/route.ts";
import { GET as getTaskDetail } from "../../app/api/tasks/[project]/route.ts";
import type { GetTaskDetailRequest } from "../../src/proto/taktdash.ts";
import { closeBackendClient } from "../../src/grpc/client.ts";

const TASK_NAME = "採用ページのレイアウトを直す";

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

const goalContext = (goalId: string) => ({ params: Promise.resolve({ goalId }) });
const taskContext = (project: string) => ({ params: Promise.resolve({ project }) });

test("ゴール詳細の Route Handler は backend の GetGoalDetail の JSON を返す", async () => {
  const server = await startTaktDashServer({ goalDetail: createGoalDetailResponse() });
  try {
    await withServerEnv(server.address, async () => {
      const response = await getGoalDetail(
        new NextRequest("http://localhost/api/goals/goal-1"),
        goalContext("goal-1"),
      );
      assert.equal(response.status, 200);
      const body = (await response.json()) as { goalId: string; objective: string };
      assert.equal(body.goalId, "goal-1");
      assert.equal(body.objective, "採用ページのレイアウトを直す目的です。");
    });
  } finally {
    await server.shutdown();
  }
});

test("ゴール詳細が NOT_FOUND のとき Route Handler は 404 を返す", async () => {
  const server = await startTaktDashServer({
    goalDetail: { code: grpc.status.NOT_FOUND, details: "missing" },
  });
  try {
    await withServerEnv(server.address, async () => {
      const response = await getGoalDetail(
        new NextRequest("http://localhost/api/goals/no-such-goal"),
        goalContext("no-such-goal"),
      );
      assert.equal(response.status, 404);
      assert.deepEqual(await response.json(), { error: "ゴールが見つかりません。" });
    });
  } finally {
    await server.shutdown();
  }
});

test("ゴール詳細の応答が要求と違う goalId のとき Route Handler は 404 を返す", async () => {
  const server = await startTaktDashServer({
    goalDetail: createGoalDetailResponse({ goalId: "goal-other" }),
  });
  try {
    await withServerEnv(server.address, async () => {
      const response = await getGoalDetail(
        new NextRequest("http://localhost/api/goals/goal-1"),
        goalContext("goal-1"),
      );
      assert.equal(response.status, 404);
      assert.deepEqual(await response.json(), { error: "ゴールが見つかりません。" });
    });
  } finally {
    await server.shutdown();
  }
});

test("タスク詳細の Route Handler はクエリを復号して backend へ渡し、JSON を返す", async () => {
  let received: GetTaskDetailRequest | undefined;
  const server = await startTaktDashServer({
    taskDetail: createTaskDetailResponse(),
    onGetTaskDetail: (request) => {
      received = request;
    },
  });
  try {
    await withServerEnv(server.address, async () => {
      const url = `http://localhost/api/tasks/corporate-site?task=${encodeURIComponent(TASK_NAME)}&goal=goal-1`;
      const response = await getTaskDetail(new NextRequest(url), taskContext("corporate-site"));
      assert.equal(response.status, 200);
      const body = (await response.json()) as { project: string; summary: string };
      assert.equal(body.project, "corporate-site");
      assert.equal(body.summary, "レイアウトを直して、確認を待っています。");
    });
    assert.deepEqual(received, {
      project: "corporate-site",
      taskName: TASK_NAME,
      goalId: "goal-1",
    });
  } finally {
    await server.shutdown();
  }
});

test("タスク詳細が NOT_FOUND のとき Route Handler は 404 を返す", async () => {
  const server = await startTaktDashServer({
    taskDetail: { code: grpc.status.NOT_FOUND, details: "missing" },
  });
  try {
    await withServerEnv(server.address, async () => {
      const url = `http://localhost/api/tasks/corporate-site?task=${encodeURIComponent(TASK_NAME)}`;
      const response = await getTaskDetail(new NextRequest(url), taskContext("corporate-site"));
      assert.equal(response.status, 404);
      assert.deepEqual(await response.json(), { error: "タスクが見つかりません。" });
    });
  } finally {
    await server.shutdown();
  }
});

test("タスク詳細の Route Handler は ?task が無いとき 400 を返す", async () => {
  const response = await getTaskDetail(
    new NextRequest("http://localhost/api/tasks/corporate-site"),
    taskContext("corporate-site"),
  );
  assert.equal(response.status, 400);
  assert.deepEqual(await response.json(), { error: "タスク名が指定されていません。" });
});

test("タスク詳細の Route Handler は ?task= が空のとき 400 を返す", async () => {
  const response = await getTaskDetail(
    new NextRequest("http://localhost/api/tasks/corporate-site?task="),
    taskContext("corporate-site"),
  );
  assert.equal(response.status, 400);
  assert.deepEqual(await response.json(), { error: "タスク名が指定されていません。" });
});

test("backend に接続できないときゴール系 Route Handler は 503 を返す", async () => {
  const closedAddress = await reserveClosedAddress();
  await withServerEnv(closedAddress, async () => {
    const detail = await getGoalDetail(
      new NextRequest("http://localhost/api/goals/goal-1"),
      goalContext("goal-1"),
    );
    assert.equal(detail.status, 503);
    assert.deepEqual(await detail.json(), { error: "backend に接続できません。" });

    const url = `http://localhost/api/tasks/corporate-site?task=${encodeURIComponent(TASK_NAME)}`;
    const task = await getTaskDetail(new NextRequest(url), taskContext("corporate-site"));
    assert.equal(task.status, 503);
    assert.deepEqual(await task.json(), { error: "backend に接続できません。" });
  });
});

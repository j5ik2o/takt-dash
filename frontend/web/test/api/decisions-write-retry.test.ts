// 書き込みの応答が期限内に届かないときの Route Handler の契約（issue #305）。
//
// backend（takt 側）では書き込みが済んでいることがあるため、「接続できません」ではなく
// 結果が不明であることを伝える。やり直しで書き込みを重ねないための鍵は、受け取った値を
// そのまま gRPC の書き込み API へ渡す。backend はダミー gRPC サーバーで代替する。
import test from "node:test";
import assert from "node:assert/strict";
import * as grpc from "@grpc/grpc-js";
import { NextRequest } from "next/server";
import { startTaktDashServer, type RunningServer } from "../support/grpc-server.ts";
import { runWrite } from "../../app/api/decisions/_shared.ts";
import { POST as postAnswer } from "../../app/api/decisions/answer/route.ts";
import { POST as postMerge } from "../../app/api/decisions/merge/route.ts";
import { POST as postApproveRegistration } from "../../app/api/decisions/goal-registration/approve/route.ts";
import { POST as postRejectRegistration } from "../../app/api/decisions/goal-registration/reject/route.ts";
import { closeBackendClient, createBackendClient } from "../../src/grpc/client.ts";

type RouteHandler = (request: NextRequest) => Promise<Response>;
type Behavior = Parameters<typeof startTaktDashServer>[0];

const withServer = async (behavior: Behavior, run: (server: RunningServer) => Promise<void>): Promise<void> => {
  const server = await startTaktDashServer(behavior);
  await closeBackendClient();
  const previous = process.env.TAKTDASH_BACKEND_ADDR;
  process.env.TAKTDASH_BACKEND_ADDR = server.address;
  try {
    await run(server);
  } finally {
    await closeBackendClient();
    if (previous === undefined) {
      delete process.env.TAKTDASH_BACKEND_ADDR;
    } else {
      process.env.TAKTDASH_BACKEND_ADDR = previous;
    }
    await server.shutdown();
  }
};

const postJson = (handler: RouteHandler, path: string, body: unknown): Promise<Response> =>
  handler(
    new NextRequest(`http://localhost${path}`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
    }),
  );

const ANSWER_BODY = { serverId: "mini", project: "esa", goalId: "goal-2", inquiryId: "inquiry-text", text: "回答" };
const MERGE_BODY = { serverId: "mini", project: "corporate-site", goalId: "goal-1", prNumber: 418 };
const REGISTRATION_BODY = { serverId: "mini", project: "takt", revision: 7 };

const writeRoutes: Array<[string, RouteHandler, string, Record<string, unknown>, Behavior]> = [
  ["回答", postAnswer, "/api/decisions/answer", ANSWER_BODY, { answerInquiry: { inquiryId: "inquiry-text" } }],
  [
    "マージ承認",
    postMerge,
    "/api/decisions/merge",
    MERGE_BODY,
    { approveGoalMerge: { taskName: "マージ", workflow: "simple" } },
  ],
  [
    "ゴール登録の承認",
    postApproveRegistration,
    "/api/decisions/goal-registration/approve",
    REGISTRATION_BODY,
    { approveGoalRegistration: { revision: 7, goalId: "goal-new" } },
  ],
  [
    "ゴール登録の却下",
    postRejectRegistration,
    "/api/decisions/goal-registration/reject",
    REGISTRATION_BODY,
    { rejectGoalRegistration: { revision: 7 } },
  ],
];

test("書き込みの応答が期限内に届かないときは 504 を返し、結果が不明であることを伝える", async () => {
  await withServer({ hangUnary: true }, async (server) => {
    const client = createBackendClient({ env: { TAKTDASH_BACKEND_ADDR: server.address }, writeDeadlineMs: 50 });
    try {
      const response = await runWrite(() => client.approveGoalMerge({ ...MERGE_BODY, until: "merge" }));

      assert.equal(response.status, 504);
      assert.deepEqual(await response.json(), {
        ok: false,
        error: "操作の結果は不明です。期限内に応答がありませんでした。状態を確かめてから、必要ならやり直してください。",
      });
      assert.equal(server.requests.length, 1, "要求は backend に届いている");
    } finally {
      await client.close();
    }
  });
});

test("backend が takt の時間切れを返したときは 504 を返し、backend の説明を伝える", async () => {
  const details = "takt-mcp から 15 秒以内に応答がなく、操作の結果は不明です。";
  await withServer({ answerInquiry: { code: grpc.status.DEADLINE_EXCEEDED, details } }, async () => {
    const response = await postJson(postAnswer, "/api/decisions/answer", ANSWER_BODY);

    assert.equal(response.status, 504);
    assert.deepEqual(await response.json(), { ok: false, error: details });
  });
});

test("idempotencyKey は gRPC の書き込み API へそのまま渡す", async () => {
  for (const [label, handler, path, body, behavior] of writeRoutes) {
    await withServer(behavior, async (server) => {
      const response = await postJson(handler, path, { ...body, idempotencyKey: "0123456789abcdef" });

      assert.equal(response.status, 200, label);
      assert.equal(
        (server.requests[0]!.request as { idempotencyKey?: string }).idempotencyKey,
        "0123456789abcdef",
        label,
      );
    });
  }
});

test("idempotencyKey の無い既存の要求は、鍵を付けずに gRPC へ渡す", async () => {
  for (const [label, handler, path, body, behavior] of writeRoutes) {
    await withServer(behavior, async (server) => {
      const response = await postJson(handler, path, body);

      assert.equal(response.status, 200, label);
      assert.equal((server.requests[0]!.request as { idempotencyKey?: string }).idempotencyKey, undefined, label);
    });
  }
});

test("idempotencyKey が正しくない書き込みは 400 で gRPC を呼ばない", async () => {
  for (const [label, handler, path, body, behavior] of writeRoutes) {
    for (const idempotencyKey of [1, "", "空白 を含む", "k".repeat(129), null]) {
      await withServer(behavior, async (server) => {
        const response = await postJson(handler, path, { ...body, idempotencyKey });

        assert.equal(response.status, 400, `${label}: ${JSON.stringify(idempotencyKey)}`);
        assert.equal(server.requests.length, 0, `${label} は gRPC を呼ばない`);
      });
    }
  }
});

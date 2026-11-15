// gRPC クライアント層の契約（C1/C2/C3/C10/C11）。
//
// backend の gRPC はダミーサーバーで置き換え、本番と同じ生成クライアントで通信する。
import test from "node:test";
import assert from "node:assert/strict";
import * as grpc from "@grpc/grpc-js";
import {
  createActivityResponse,
  createDashboardState,
  createProjectsResponse,
  createRunDetailResponse,
} from "../support/fixtures.ts";
import { reserveClosedAddress, startTaktDashServer } from "../support/grpc-server.ts";
import { createBackendClient, resolveBackendAddress, type BackendClient } from "../../src/grpc/client.ts";

const withServer = async (
  behavior: Parameters<typeof startTaktDashServer>[0],
  run: (ctx: { address: string }) => Promise<void>,
) => {
  const server = await startTaktDashServer(behavior);
  try {
    await run({ address: server.address });
  } finally {
    await server.shutdown();
  }
};

test("接続先は環境変数 TAKTDASH_BACKEND_ADDR で指定できる", async () => {
  await withServer({ overview: createDashboardState() }, async ({ address }) => {
    const client = createBackendClient({ env: { TAKTDASH_BACKEND_ADDR: address } });
    try {
      const state = await client.getOverview();
      assert.equal(state.servers.length, 2);
    } finally {
      await client.close();
    }
  });
});

test("接続先の環境変数が未設定なら既定の 127.0.0.1:50051 を使う", () => {
  assert.equal(resolveBackendAddress({}), "127.0.0.1:50051");
});

test("6つの読み取り API をダミーサーバーから取得できる", async () => {
  await withServer(
    {
      overview: createDashboardState(),
      projects: createProjectsResponse(),
      pendingDecisions: { state: createDashboardState() },
      activity: createActivityResponse(),
      runDetail: createRunDetailResponse("run-aaa"),
    },
    async ({ address }) => {
      const client = createBackendClient({ env: { TAKTDASH_BACKEND_ADDR: address } });
      try {
        assert.equal((await client.getOverview()).projects.length, 2);
        assert.equal((await client.getProjects()).recentEvents.length, 3);
        assert.equal((await client.getPendingDecisions()).pendingDecisions.length, 2);
        assert.equal((await client.getActivity()).dailySummaries.length, 7);
        assert.equal((await client.getRunDetail("run-aaa")).currentStep, "implement");
      } finally {
        await client.close();
      }
    },
  );
});

test("run 詳細が NOT_FOUND を返すとき RunNotFoundError として扱う", async () => {
  await withServer({ runDetail: { code: grpc.status.NOT_FOUND, details: "missing" } }, async ({ address }) => {
    const client = createBackendClient({ env: { TAKTDASH_BACKEND_ADDR: address } });
    try {
      await assert.rejects(client.getRunDetail("no-such-run"), (error: Error) => error.name === "RunNotFoundError");
    } finally {
      await client.close();
    }
  });
});

test("応答の runSlug が要求と違うときも RunNotFoundError として扱う", async () => {
  await withServer({ runDetail: createRunDetailResponse("run-other") }, async ({ address }) => {
    const client = createBackendClient({ env: { TAKTDASH_BACKEND_ADDR: address } });
    try {
      await assert.rejects(client.getRunDetail("run-aaa"), (error: Error) => error.name === "RunNotFoundError");
    } finally {
      await client.close();
    }
  });
});

test("backend に接続できないとき BackendUnavailableError として扱い、ページを落とさない", async () => {
  const closedAddress = await reserveClosedAddress();
  const client = createBackendClient({ env: { TAKTDASH_BACKEND_ADDR: closedAddress } });
  try {
    await assert.rejects(
      client.getOverview(),
      (error: Error) => error.name === "BackendUnavailableError",
    );
  } finally {
    await client.close();
  }
});

test("NOT_FOUND は BackendUnavailableError と取り違えない", async () => {
  await withServer({ runDetail: { code: grpc.status.NOT_FOUND, details: "missing" } }, async ({ address }) => {
    const client = createBackendClient({ env: { TAKTDASH_BACKEND_ADDR: address } });
    try {
      await assert.rejects(client.getRunDetail("x"), (error: Error) => error.name !== "BackendUnavailableError");
    } finally {
      await client.close();
    }
  });
});

test("書き込みの要求は期限を過ぎると BackendUnavailableError として扱う", async () => {
  const cases: Array<
    [string, Parameters<typeof startTaktDashServer>[0], (client: BackendClient) => Promise<unknown>]
  > = [
    [
      "回答",
      { answerInquiry: { inquiryId: "inquiry-text" } },
      (client) =>
        client.answerInquiry({
          serverId: "mini",
          project: "esa",
          goalId: "goal-2",
          inquiryId: "inquiry-text",
          text: "回答",
        }),
    ],
    [
      "マージ承認",
      { approveGoalMerge: { taskName: "マージ", workflow: "simple" } },
      (client) =>
        client.approveGoalMerge({
          serverId: "mini",
          project: "corporate-site",
          goalId: "goal-1",
          prNumber: 418,
          until: "merge",
        }),
    ],
    [
      "ゴール登録の承認",
      { approveGoalRegistration: { revision: 7, goalId: "goal-new" } },
      (client) => client.approveGoalRegistration({ serverId: "mini", project: "takt", revision: 7 }),
    ],
    [
      "ゴール登録の却下",
      { rejectGoalRegistration: { revision: 7 } },
      (client) => client.rejectGoalRegistration({ serverId: "mini", project: "takt", revision: 7 }),
    ],
  ];

  for (const [label, behavior, invoke] of cases) {
    await withServer({ ...behavior, writeDelayMs: 300 }, async ({ address }) => {
      const client = createBackendClient({
        env: { TAKTDASH_BACKEND_ADDR: address },
        writeDeadlineMs: 25,
      });
      try {
        await assert.rejects(invoke(client), (error: Error) => {
          assert.equal(error.name, "BackendUnavailableError", label);
          return true;
        });
      } finally {
        await client.close();
      }
    });
  }
});

test("読み取りの要求には書き込みの期限を適用しない", async () => {
  await withServer({ overview: createDashboardState(), readDelayMs: 300 }, async ({ address }) => {
    const client = createBackendClient({
      env: { TAKTDASH_BACKEND_ADDR: address },
      writeDeadlineMs: 25,
    });
    try {
      const state = await client.getOverview();
      assert.equal(state.servers.length, 2);
    } finally {
      await client.close();
    }
  });
});

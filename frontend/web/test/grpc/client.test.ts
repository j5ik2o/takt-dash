// gRPC クライアント層の契約（C1/C2/C3/C10/C11）。
//
// backend の gRPC はダミーサーバーで置き換え、本番と同じ生成クライアントで通信する。
import test from "node:test";
import assert from "node:assert/strict";
import * as grpc from "@grpc/grpc-js";
import {
  createActivityResponse,
  createConversationResponse,
  createDashboardState,
  createProjectsResponse,
  createRunDetailResponse,
} from "../support/fixtures.ts";
import { reserveClosedAddress, startTaktDashServer, type RunningServer } from "../support/grpc-server.ts";
import { createBackendClient, resolveBackendAddress, type BackendClient } from "../../src/grpc/client.ts";

const withServer = async (
  behavior: Parameters<typeof startTaktDashServer>[0],
  run: (ctx: { address: string; conversationRequests: RunningServer["conversationRequests"] }) => Promise<void>,
) => {
  const server = await startTaktDashServer(behavior);
  try {
    await run({ address: server.address, conversationRequests: server.conversationRequests });
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

test("GetConversation を取得し、project・session_id・after_seq・limit を backend へ渡す", async () => {
  await withServer({ conversation: createConversationResponse() }, async ({ address, conversationRequests }) => {
    const client = createBackendClient({ env: { TAKTDASH_BACKEND_ADDR: address } });
    try {
      const conversation = await client.getConversation({
        project: "corporate-site",
        sessionId: "session-conv-1",
        afterSeq: 2,
        limit: 50,
      });

      assert.equal(conversation.messages.length, 3);
      assert.equal(conversation.sessionId, "session-conv-1");
      assert.equal(conversation.nextAfterSeq, 3);
      assert.equal(conversation.hasMore, false);
      assert.equal(conversationRequests.length, 1);
      assert.equal(conversationRequests[0]?.project, "corporate-site");
      assert.equal(conversationRequests[0]?.sessionId, "session-conv-1");
      assert.equal(conversationRequests[0]?.afterSeq, 2);
      assert.equal(conversationRequests[0]?.limit, 50);
    } finally {
      await client.close();
    }
  });
});

// 応答しない unary の待ちを、テスト自身の上限時間で打ち切る。
// deadline が設定されていない実装では "timeout" が返り、assertion が失敗する。
const settleWithin = async (promise: Promise<unknown>, ms: number): Promise<string> => {
  let timer: NodeJS.Timeout | undefined;
  const timeout = new Promise<string>((resolve) => {
    timer = setTimeout(() => resolve("timeout"), ms);
  });
  try {
    return await Promise.race([promise.then(() => "resolved", (error: Error) => error.name), timeout]);
  } finally {
    if (timer !== undefined) {
      clearTimeout(timer);
    }
  }
};

test("応答を返さない 5 つの unary RPC は有限の deadline で終了する", async () => {
  await withServer({ hangUnary: true }, async ({ address }) => {
    // 既定の deadline を待たずに観測できるよう、テストから短い deadline を渡す。
    const options = { env: { TAKTDASH_BACKEND_ADDR: address }, deadlineMs: 50 };
    const client = createBackendClient(options);
    try {
      const calls = {
        getOverview: () => client.getOverview(),
        getProjects: () => client.getProjects(),
        getPendingDecisions: () => client.getPendingDecisions(),
        getActivity: () => client.getActivity(),
        getRunDetail: () => client.getRunDetail("run-aaa"),
      };
      for (const [name, call] of Object.entries(calls)) {
        assert.equal(
          await settleWithin(call(), 1_000),
          "BackendUnavailableError",
          `${name} は deadline の超過を BackendUnavailableError として返す必要があります`,
        );
      }
    } finally {
      await client.close();
    }
  });
});

test("deadlineMs を指定しない既定の unary は有限の deadline を送る", async () => {
  // 既定値の生成そのものを観測するため、サーバー側で受け取った deadline を記録する。
  // 実時間で 30 秒は待たず、有限で観測時刻より未来かつ 60 秒以内であることだけを確かめる。
  let observedDeadlineMs: number | undefined;
  const before = Date.now();
  await withServer(
    {
      overview: createDashboardState(),
      onOverviewDeadline: (deadline) => {
        observedDeadlineMs = typeof deadline === "number" ? deadline : deadline.getTime();
      },
    },
    async ({ address }) => {
      const client = createBackendClient({ env: { TAKTDASH_BACKEND_ADDR: address } });
      try {
        await client.getOverview();
      } finally {
        await client.close();
      }
    },
  );
  const after = Date.now();
  assert.ok(observedDeadlineMs !== undefined, "既定の unary はサーバーへ deadline を送る必要があります");
  assert.ok(Number.isFinite(observedDeadlineMs), "既定の deadline は有限値である必要があります");
  assert.ok(observedDeadlineMs > before, "既定の deadline は観測時刻より未来である必要があります");
  assert.ok(observedDeadlineMs <= after + 60_000, "既定の deadline は 60 秒以内である必要があります");
});

test("deadline の超過は BackendUnavailableError として扱う", async () => {
  await withServer({ overview: { code: grpc.status.DEADLINE_EXCEEDED, details: "期限切れ" } }, async ({ address }) => {
    const client = createBackendClient({ env: { TAKTDASH_BACKEND_ADDR: address } });
    try {
      await assert.rejects(client.getOverview(), (error: Error) => error.name === "BackendUnavailableError");
    } finally {
      await client.close();
    }
  });
});

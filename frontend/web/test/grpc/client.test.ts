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
import { createBackendClient, resolveBackendAddress } from "../../src/grpc/client.ts";

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

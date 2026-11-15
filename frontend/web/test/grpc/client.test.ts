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
import {
  closeBackendClient,
  createBackendClient,
  getBackendClient,
  resolveBackendAddress,
} from "../../src/grpc/client.ts";

const withServer = async (
  behavior: Parameters<typeof startTaktDashServer>[0],
  run: (ctx: { address: string; calls: string[] }) => Promise<void>,
) => {
  const calls: string[] = [];
  const server = await startTaktDashServer({ ...behavior, calls });
  try {
    await run({ address: server.address, calls });
  } finally {
    await server.shutdown();
  }
};

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

test("unary RPC の所要時間を呼び出しごとに計測先へ名前つきで渡す", async () => {
  await withServer(
    {
      overview: createDashboardState(),
      projects: createProjectsResponse(),
      pendingDecisions: { state: createDashboardState() },
      activity: createActivityResponse(),
      runDetail: createRunDetailResponse("run-aaa"),
    },
    async ({ address }) => {
      const recorded: { rpc: string; durationMs: number }[] = [];
      const timing = { recordRpc: (value: { rpc: string; durationMs: number }) => recorded.push(value) };
      const client = createBackendClient({ env: { TAKTDASH_BACKEND_ADDR: address } });
      try {
        await client.getOverview(timing);
        await client.getProjects(timing);
        await client.getPendingDecisions(timing);
        await client.getActivity(timing);
        await client.getRunDetail("run-aaa", timing);
      } finally {
        await client.close();
      }
      assert.deepEqual(
        recorded.map((value) => value.rpc),
        ["GetOverview", "GetProjects", "GetPendingDecisions", "GetActivity", "GetRunDetail"],
      );
      for (const value of recorded) {
        assert.ok(value.durationMs >= 0, `${value.rpc} の所要時間は 0 以上です`);
      }
    },
  );
});

test("unary RPC が失敗しても所要時間を計測先へ渡す", async () => {
  await withServer({}, async ({ address }) => {
    const recorded: { rpc: string; durationMs: number }[] = [];
    const timing = { recordRpc: (value: { rpc: string; durationMs: number }) => recorded.push(value) };
    const client = createBackendClient({ env: { TAKTDASH_BACKEND_ADDR: address } });
    try {
      await assert.rejects(client.getOverview(timing));
    } finally {
      await client.close();
    }
    assert.equal(recorded.length, 1, "失敗した RPC も1回として記録します");
    assert.equal(recorded[0].rpc, "GetOverview");
  });
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

test("共有クライアントは同じインスタンスを返し、明示的な破棄で作り直す", async () => {
  await closeBackendClient();
  try {
    const first = getBackendClient();
    assert.equal(getBackendClient(), first, "同じプロセスでは同じインスタンスを返します");
    await closeBackendClient();
    const second = getBackendClient();
    assert.notEqual(second, first, "明示的に破棄した後は新しいインスタンスを作ります");
  } finally {
    await closeBackendClient();
  }
});

test("共有クライアントは要求をまたいで使い続けられる", async () => {
  await withServer({ overview: createDashboardState() }, async ({ address, calls }) => {
    await withServerEnv(address, async () => {
      assert.equal((await getBackendClient().getOverview()).servers.length, 2);
      assert.equal((await getBackendClient().getOverview()).servers.length, 2);
    });
    assert.deepEqual(calls, ["GetOverview", "GetOverview"], "同じクライアントで 2 回の要求を処理します");
  });
});

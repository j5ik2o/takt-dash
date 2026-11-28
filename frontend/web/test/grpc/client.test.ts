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
  createGoalDetailResponse,
  createProjectsResponse,
  createRunDetailResponse,
  createTaskDetailResponse,
} from "../support/fixtures.ts";
import { reserveClosedAddress, startTaktDashServer, type RunningServer } from "../support/grpc-server.ts";
import {
  BackendUnavailableError,
  backendUnavailableSituation,
  closeBackendClient,
  createBackendClient,
  getBackendClient,
  hasBackendResponded,
  resetBackendResponded,
  resolveBackendAddress,
  RUN_DETAIL_DEADLINE_MS,
  type BackendClient,
} from "../../src/grpc/client.ts";
import type { GetGoalDetailRequest, GetTaskDetailRequest } from "../../src/proto/taktdash.ts";

const withServer = async (
  behavior: Parameters<typeof startTaktDashServer>[0],
  run: (ctx: { address: string; calls: string[]; conversationRequests: RunningServer["conversationRequests"] }) => Promise<void>,
) => {
  const calls: string[] = [];
  const server = await startTaktDashServer({ ...behavior, calls });
  try {
    await run({ address: server.address, calls, conversationRequests: server.conversationRequests });
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

test("ゴール詳細・タスク詳細をダミーサーバーから取得できる", async () => {
  await withServer(
    {
      goalDetail: createGoalDetailResponse(),
      taskDetail: createTaskDetailResponse(),
    },
    async ({ address, calls }) => {
      const client = createBackendClient({ env: { TAKTDASH_BACKEND_ADDR: address } });
      try {
        const detail = await client.getGoalDetail("goal-1");
        assert.equal(detail.objective, "採用ページのレイアウトを直す目的です。");

        const task = await client.getTaskDetail("corporate-site", "採用ページのレイアウトを直す", "goal-1");
        assert.equal(task.summary, "レイアウトを直して、確認を待っています。");
        assert.deepEqual(calls, ["GetGoalDetail", "GetTaskDetail"], "詳細の RPC も呼ばれた名前を記録する");
      } finally {
        await client.close();
      }
    },
  );
});

test("ゴール詳細の要求は goal ID をそのまま渡す", async () => {
  let received: GetGoalDetailRequest | undefined;
  await withServer(
    {
      goalDetail: createGoalDetailResponse(),
      onGetGoalDetail: (request) => {
        received = request;
      },
    },
    async ({ address }) => {
      const client = createBackendClient({ env: { TAKTDASH_BACKEND_ADDR: address } });
      try {
        await client.getGoalDetail("goal-1");
        assert.deepEqual(received, { goalId: "goal-1" });
      } finally {
        await client.close();
      }
    },
  );
});

test("タスク詳細の要求はプロジェクト・タスク名・任意のゴール ID をそのまま渡す", async () => {
  let received: GetTaskDetailRequest | undefined;
  await withServer(
    {
      taskDetail: createTaskDetailResponse(),
      onGetTaskDetail: (request) => {
        received = request;
      },
    },
    async ({ address }) => {
      const client = createBackendClient({ env: { TAKTDASH_BACKEND_ADDR: address } });
      try {
        await client.getTaskDetail("corporate-site", "採用ページのレイアウトを直す", "goal-1");
        assert.deepEqual(received, {
          project: "corporate-site",
          taskName: "採用ページのレイアウトを直す",
          goalId: "goal-1",
        });

        received = undefined;
        await client.getTaskDetail("corporate-site", "採用ページのレイアウトを直す");
        assert.deepEqual(received, {
          project: "corporate-site",
          taskName: "採用ページのレイアウトを直す",
          goalId: undefined,
        });
      } finally {
        await client.close();
      }
    },
  );
});

test("ゴール詳細が NOT_FOUND を返すとき GoalNotFoundError として扱う", async () => {
  await withServer({ goalDetail: { code: grpc.status.NOT_FOUND, details: "missing" } }, async ({ address }) => {
    const client = createBackendClient({ env: { TAKTDASH_BACKEND_ADDR: address } });
    try {
      await assert.rejects(client.getGoalDetail("no-such-goal"), (error: Error) => error.name === "GoalNotFoundError");
    } finally {
      await client.close();
    }
  });
});

test("応答の goalId が要求と違うときも GoalNotFoundError として扱う", async () => {
  await withServer(
    { goalDetail: createGoalDetailResponse({ goalId: "goal-other" }) },
    async ({ address }) => {
      const client = createBackendClient({ env: { TAKTDASH_BACKEND_ADDR: address } });
      try {
        await assert.rejects(client.getGoalDetail("goal-1"), (error: Error) => error.name === "GoalNotFoundError");
      } finally {
        await client.close();
      }
    },
  );
});

test("タスク詳細が NOT_FOUND を返すとき TaskNotFoundError として扱う", async () => {
  await withServer({ taskDetail: { code: grpc.status.NOT_FOUND, details: "missing" } }, async ({ address }) => {
    const client = createBackendClient({ env: { TAKTDASH_BACKEND_ADDR: address } });
    try {
      await assert.rejects(
        client.getTaskDetail("corporate-site", "no-such-task"),
        (error: Error) => error.name === "TaskNotFoundError",
      );
    } finally {
      await client.close();
    }
  });
});

test("応答のタスク名・プロジェクト・ゴール ID が要求と違うときも TaskNotFoundError として扱う", async () => {
  for (const response of [
    createTaskDetailResponse({ taskName: "別のタスク" }),
    createTaskDetailResponse({ project: "esa" }),
    createTaskDetailResponse({ goalId: "goal-2" }),
  ]) {
    await withServer({ taskDetail: response }, async ({ address }) => {
      const client = createBackendClient({ env: { TAKTDASH_BACKEND_ADDR: address } });
      try {
        await assert.rejects(
          client.getTaskDetail("corporate-site", "採用ページのレイアウトを直す", "goal-1"),
          (error: Error) => error.name === "TaskNotFoundError",
        );
      } finally {
        await client.close();
      }
    });
  }
});

test("バックエンドの接続失敗はゴール詳細・タスク詳細でも BackendUnavailableError として扱う", async () => {
  const closedAddress = await reserveClosedAddress();
  const client = createBackendClient({ env: { TAKTDASH_BACKEND_ADDR: closedAddress } });
  try {
    await assert.rejects(
      client.getGoalDetail("goal-1"),
      (error: Error) => error.name === "BackendUnavailableError",
    );
    await assert.rejects(
      client.getTaskDetail("corporate-site", "採用ページのレイアウトを直す"),
      (error: Error) => error.name === "BackendUnavailableError",
    );
  } finally {
    await client.close();
  }
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
    const options = { env: { TAKTDASH_BACKEND_ADDR: address }, deadlineMs: 50, runDetailDeadlineMs: 50 };
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

test("既定の getRunDetail は backend の総上限 45 秒より長く、50 秒以内の deadline を送る", async () => {
  // issue #59：回り道の run 詳細は takt-mcp を順に何回も呼ぶので、ほかの unary の 30 秒では足りない。
  // backend の総上限（45 秒）より先に Web が打ち切らないことを、送られた deadline で確かめる。
  let observedDeadlineMs: number | undefined;
  const before = Date.now();
  await withServer(
    {
      runDetail: createRunDetailResponse("run-aaa"),
      onRunDetailDeadline: (deadline) => {
        observedDeadlineMs = typeof deadline === "number" ? deadline : deadline.getTime();
      },
    },
    async ({ address }) => {
      const client = createBackendClient({ env: { TAKTDASH_BACKEND_ADDR: address } });
      try {
        await client.getRunDetail("run-aaa");
      } finally {
        await client.close();
      }
    },
  );
  const after = Date.now();
  assert.equal(RUN_DETAIL_DEADLINE_MS, 50_000);
  assert.ok(observedDeadlineMs !== undefined, "getRunDetail はサーバーへ deadline を送る必要があります");
  assert.ok(observedDeadlineMs > after + 45_000, "deadline は backend の総上限 45 秒より後である必要があります");
  // サーバーは受け取った時刻に grpc-timeout を足して deadline を作るので、上の端は呼び出しの後の時刻で見る。
  assert.ok(observedDeadlineMs <= after + RUN_DETAIL_DEADLINE_MS, "deadline は 50 秒以内である必要があります");
  assert.ok(observedDeadlineMs > before, "deadline は呼び出しの時刻より後である必要があります");
});

test("既定の getOverview の deadline は 30 秒以内のまま", async () => {
  let observedDeadlineMs: number | undefined;
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
  assert.ok(observedDeadlineMs !== undefined, "getOverview はサーバーへ deadline を送る必要があります");
  assert.ok(observedDeadlineMs <= after + 30_000, "getRunDetail 以外の unary の deadline は 30 秒以内のままです");
});

test("backend の総上限の DEADLINE_EXCEEDED は、getRunDetail で応答なしとして扱う", async () => {
  await withServer(
    { runDetail: { code: grpc.status.DEADLINE_EXCEEDED, details: "run run-aaa の詳細を 45 秒以内に読めませんでした。" } },
    async ({ address }) => {
      const client = createBackendClient({ env: { TAKTDASH_BACKEND_ADDR: address } });
      try {
        await assert.rejects(client.getRunDetail("run-aaa"), (error: unknown) => {
          assert.ok(error instanceof BackendUnavailableError);
          assert.equal(error.failure, "no-response");
          return true;
        });
      } finally {
        await client.close();
      }
    },
  );
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

test("deadline の超過は接続不可と区別して応答なしとして扱う", async () => {
  await withServer({ overview: { code: grpc.status.DEADLINE_EXCEEDED, details: "期限切れ" } }, async ({ address }) => {
    const client = createBackendClient({ env: { TAKTDASH_BACKEND_ADDR: address } });
    try {
      await assert.rejects(client.getOverview(), (error: unknown) => {
        assert.ok(error instanceof BackendUnavailableError);
        assert.equal(error.failure, "no-response");
        return true;
      });
    } finally {
      await client.close();
    }
  });
});

test("応答を返さない backend は deadline の超過で応答なしとして扱う", async () => {
  await withServer({ hangUnary: true }, async ({ address }) => {
    const client = createBackendClient({ env: { TAKTDASH_BACKEND_ADDR: address }, deadlineMs: 50 });
    try {
      await assert.rejects(client.getOverview(), (error: unknown) => {
        assert.ok(error instanceof BackendUnavailableError);
        assert.equal(error.failure, "no-response");
        return true;
      });
    } finally {
      await client.close();
    }
  });
});

test("接続できない場合は応答なしと区別して接続不可として扱う", async () => {
  const closedAddress = await reserveClosedAddress();
  const client = createBackendClient({ env: { TAKTDASH_BACKEND_ADDR: closedAddress } });
  try {
    await assert.rejects(client.getOverview(), (error: unknown) => {
      assert.ok(error instanceof BackendUnavailableError);
      assert.equal(error.failure, "unreachable");
      return true;
    });
  } finally {
    await client.close();
  }
});

test("backend の応答を受けると接続履歴が残り、表示用の状況に反映される", async () => {
  resetBackendResponded();
  assert.equal(hasBackendResponded(), false, "履歴を消した直後は応答を受けていない");
  await withServer({ overview: createDashboardState() }, async ({ address }) => {
    const client = createBackendClient({ env: { TAKTDASH_BACKEND_ADDR: address } });
    try {
      await client.getOverview();
      assert.equal(hasBackendResponded(), true, "応答を受けると履歴が残る");
      const situation = backendUnavailableSituation(new BackendUnavailableError(new Error("切断")));
      assert.equal(situation.respondedBefore, true, "状況には接続履歴が反映される");
      assert.equal(situation.failure, "unreachable", "状況には失敗の種類が反映される");
    } finally {
      await client.close();
    }
  });
});

// 判断待ちの操作を受け付ける Route Handler の契約。
//
// ブラウザは backend の gRPC を直接呼ばない。Route Handler が入力を検証し、
// 受け取った値をそのまま gRPC の書き込み API へ渡す。ここではダミー gRPC サーバーが
// 実際に受け取ったリクエストを観測する。
import test from "node:test";
import assert from "node:assert/strict";
import * as grpc from "@grpc/grpc-js";
import { NextRequest } from "next/server";
import { reserveClosedAddress, startTaktDashServer, type RunningServer } from "../support/grpc-server.ts";
import { POST as postAnswer } from "../../app/api/decisions/answer/route.ts";
import { POST as postMerge } from "../../app/api/decisions/merge/route.ts";
import { POST as postApproveRegistration } from "../../app/api/decisions/goal-registration/approve/route.ts";
import { POST as postRejectRegistration } from "../../app/api/decisions/goal-registration/reject/route.ts";
import { closeBackendClient } from "../../src/grpc/client.ts";

type RouteHandler = (request: NextRequest) => Promise<Response>;

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

const postWithHeaders = (
  handler: RouteHandler,
  path: string,
  body: unknown,
  headers: Record<string, string>,
): Promise<Response> =>
  handler(
    new NextRequest(`http://localhost${path}`, {
      method: "POST",
      headers,
      body: JSON.stringify(body),
    }),
  );

const postJson = (handler: RouteHandler, path: string, body: unknown): Promise<Response> =>
  postWithHeaders(handler, path, body, { "content-type": "application/json" });

const withServer = async (
  behavior: Parameters<typeof startTaktDashServer>[0],
  run: (server: RunningServer) => Promise<void>,
): Promise<void> => {
  const server = await startTaktDashServer(behavior);
  try {
    await withServerEnv(server.address, () => run(server));
  } finally {
    await server.shutdown();
  }
};

const ANSWER_PATH = "/api/decisions/answer";
const MERGE_PATH = "/api/decisions/merge";
const APPROVE_PATH = "/api/decisions/goal-registration/approve";
const REJECT_PATH = "/api/decisions/goal-registration/reject";

type AnswerRequest = {
  serverId: string;
  project: string;
  goalId?: string;
  inquiryId: string;
  text?: string;
  report?: number;
};

type MergeRequest = {
  serverId: string;
  project: string;
  goalId: string;
  prNumber: number;
  until: string;
};

type RegistrationRequest = {
  serverId: string;
  project: string;
  revision: number;
};

test("回答（自由記述）は text だけを gRPC の AnswerInquiry へ渡す", async () => {
  await withServer({ answerInquiry: { inquiryId: "inquiry-text" } }, async (server) => {
    const response = await postJson(postAnswer, ANSWER_PATH, {
      serverId: "mini",
      project: "esa",
      goalId: "goal-2",
      inquiryId: "inquiry-text",
      text: " 補足します ",
    });

    assert.equal(response.status, 200);
    assert.equal((await response.json()).ok, true);
    assert.equal(server.requests.length, 1);
    const call = server.requests[0]!;
    assert.equal(call.method, "answerInquiry");
    const request = call.request as AnswerRequest;
    assert.equal(request.serverId, "mini");
    assert.equal(request.project, "esa");
    assert.equal(request.goalId, "goal-2");
    assert.equal(request.inquiryId, "inquiry-text");
    assert.equal(request.text, "補足します");
    assert.equal(request.report, undefined);
  });
});

test("回答（作業の依頼）は report だけを gRPC の AnswerInquiry へ渡す", async () => {
  await withServer({ answerInquiry: { inquiryId: "inquiry-operation" } }, async (server) => {
    const response = await postJson(postAnswer, ANSWER_PATH, {
      serverId: "mini",
      project: "esa",
      goalId: "goal-2",
      inquiryId: "inquiry-operation",
      report: 2,
    });

    assert.equal(response.status, 200);
    const request = server.requests[0]!.request as AnswerRequest;
    assert.equal(request.report, 2);
    assert.equal(request.text, undefined);
  });
});

test("マージ承認は until を merge 固定で gRPC の ApproveGoalMerge へ渡す", async () => {
  await withServer({ approveGoalMerge: { taskName: "マージ", workflow: "simple" } }, async (server) => {
    const response = await postJson(postMerge, MERGE_PATH, {
      serverId: "mini",
      project: "corporate-site",
      goalId: "goal-1",
      prNumber: 418,
    });

    assert.equal(response.status, 200);
    assert.equal((await response.json()).ok, true);
    const call = server.requests[0]!;
    assert.equal(call.method, "approveGoalMerge");
    const request = call.request as MergeRequest;
    assert.equal(request.serverId, "mini");
    assert.equal(request.project, "corporate-site");
    assert.equal(request.goalId, "goal-1");
    assert.equal(request.prNumber, 418);
    assert.equal(request.until, "merge");
  });
});

test("ゴール登録の承認は revision を gRPC の ApproveGoalRegistration へ渡す", async () => {
  await withServer(
    { approveGoalRegistration: { revision: 7, goalId: "goal-new" } },
    async (server) => {
      const response = await postJson(postApproveRegistration, APPROVE_PATH, {
        serverId: "mini",
        project: "takt",
        revision: 7,
      });

      assert.equal(response.status, 200);
      assert.equal((await response.json()).ok, true);
      const call = server.requests[0]!;
      assert.equal(call.method, "approveGoalRegistration");
      const request = call.request as RegistrationRequest;
      assert.equal(request.serverId, "mini");
      assert.equal(request.project, "takt");
      assert.equal(request.revision, 7);
    },
  );
});

test("ゴール登録の却下は revision を gRPC の RejectGoalRegistration へ渡す", async () => {
  await withServer({ rejectGoalRegistration: { revision: 7 } }, async (server) => {
    const response = await postJson(postRejectRegistration, REJECT_PATH, {
      serverId: "mini",
      project: "takt",
      revision: 7,
    });

    assert.equal(response.status, 200);
    assert.equal((await response.json()).ok, true);
    const call = server.requests[0]!;
    assert.equal(call.method, "rejectGoalRegistration");
    const request = call.request as RegistrationRequest;
    assert.equal(request.serverId, "mini");
    assert.equal(request.project, "takt");
    assert.equal(request.revision, 7);
  });
});

test("問い合わせの識別子が無い回答は 400 で gRPC を呼ばない", async () => {
  await withServer({ answerInquiry: { inquiryId: "x" } }, async (server) => {
    const response = await postJson(postAnswer, ANSWER_PATH, {
      serverId: "mini",
      project: "esa",
      goalId: "goal-2",
      text: "回答",
    });

    assert.equal(response.status, 400);
    assert.equal((await response.json()).ok, false);
    assert.equal(server.requests.length, 0);
  });
});

test("text と report を同時に送る回答は 400 で gRPC を呼ばない", async () => {
  await withServer({ answerInquiry: { inquiryId: "x" } }, async (server) => {
    const response = await postJson(postAnswer, ANSWER_PATH, {
      serverId: "mini",
      project: "esa",
      goalId: "goal-2",
      inquiryId: "inquiry-text",
      text: "回答",
      report: 1,
    });

    assert.equal(response.status, 400);
    assert.equal(server.requests.length, 0);
  });
});

test("プルリクエスト番号が正の整数でないマージ承認は 400 で gRPC を呼ばない", async () => {
  await withServer({ approveGoalMerge: { taskName: "マージ", workflow: "simple" } }, async (server) => {
    const response = await postJson(postMerge, MERGE_PATH, {
      serverId: "mini",
      project: "corporate-site",
      goalId: "goal-1",
      prNumber: 0,
    });

    assert.equal(response.status, 400);
    assert.equal(server.requests.length, 0);
  });
});

test("revision が正の整数でないゴール登録の承認は 400 で gRPC を呼ばない", async () => {
  await withServer({ approveGoalRegistration: { revision: 7, goalId: "goal-new" } }, async (server) => {
    const response = await postJson(postApproveRegistration, APPROVE_PATH, {
      serverId: "mini",
      project: "takt",
      revision: 0,
    });

    assert.equal(response.status, 400);
    assert.equal(server.requests.length, 0);
  });
});

test("backend が INVALID_ARGUMENT を返すと 400 と失敗の文言を返す", async () => {
  await withServer(
    {
      answerInquiry: {
        code: grpc.status.INVALID_ARGUMENT,
        details: "問い合わせはゴールに属していません。",
      },
    },
    async () => {
      const response = await postJson(postAnswer, ANSWER_PATH, {
        serverId: "mini",
        project: "esa",
        goalId: "goal-2",
        inquiryId: "inquiry-without-goal",
        text: "回答",
      });

      assert.equal(response.status, 400);
      assert.deepEqual(await response.json(), {
        ok: false,
        error: "問い合わせはゴールに属していません。",
      });
    },
  );
});

test("backend が UNAVAILABLE を返すと 502 と backend の理由を返し、接続不能として扱わない", async () => {
  await withServer(
    {
      answerInquiry: {
        code: grpc.status.UNAVAILABLE,
        details: "道具の実行に失敗しました。",
      },
    },
    async () => {
      const response = await postJson(postAnswer, ANSWER_PATH, {
        serverId: "mini",
        project: "esa",
        goalId: "goal-2",
        inquiryId: "inquiry-text",
        text: "回答",
      });

      assert.equal(response.status, 502);
      assert.deepEqual(await response.json(), {
        ok: false,
        error: "道具の実行に失敗しました。",
      });
    },
  );
});

test("backend に接続できないときは 503 を返し、失敗を画面に伝える", async () => {
  const address = await reserveClosedAddress();
  await withServerEnv(address, async () => {
    const response = await postJson(postAnswer, ANSWER_PATH, {
      serverId: "mini",
      project: "esa",
      goalId: "goal-2",
      inquiryId: "inquiry-text",
      text: "回答",
    });

    assert.equal(response.status, 503);
    assert.deepEqual(await response.json(), {
      ok: false,
      error: "backend に接続できません。",
    });
  });
});

const ANSWER_BODY = {
  serverId: "mini",
  project: "esa",
  goalId: "goal-2",
  inquiryId: "inquiry-text",
  text: "回答",
} as const;

const MERGE_BODY = {
  serverId: "mini",
  project: "corporate-site",
  goalId: "goal-1",
  prNumber: 418,
} as const;

const REGISTRATION_BODY = {
  serverId: "mini",
  project: "takt",
  revision: 7,
} as const;

const writeRoutes: Array<
  [string, RouteHandler, string, unknown, Parameters<typeof startTaktDashServer>[0]]
> = [
  ["回答", postAnswer, ANSWER_PATH, ANSWER_BODY, { answerInquiry: { inquiryId: "inquiry-text" } }],
  [
    "マージ承認",
    postMerge,
    MERGE_PATH,
    MERGE_BODY,
    { approveGoalMerge: { taskName: "マージ", workflow: "simple" } },
  ],
  [
    "ゴール登録の承認",
    postApproveRegistration,
    APPROVE_PATH,
    REGISTRATION_BODY,
    { approveGoalRegistration: { revision: 7, goalId: "goal-new" } },
  ],
  [
    "ゴール登録の却下",
    postRejectRegistration,
    REJECT_PATH,
    REGISTRATION_BODY,
    { rejectGoalRegistration: { revision: 7 } },
  ],
];

test("Content-Type が application/json でない書き込みは 415 で gRPC を呼ばない", async () => {
  for (const [label, handler, path, body, behavior] of writeRoutes) {
    await withServer(behavior, async (server) => {
      const response = await postWithHeaders(handler, path, body, {
        "content-type": "text/plain;charset=UTF-8",
      });

      assert.equal(response.status, 415, label);
      assert.equal(server.requests.length, 0, `${label} は gRPC を呼ばない`);
    });
  }
});

test("charset 付きの application/json は既存どおり許可する", async () => {
  await withServer({ approveGoalMerge: { taskName: "マージ", workflow: "simple" } }, async (server) => {
    const response = await postWithHeaders(postMerge, MERGE_PATH, MERGE_BODY, {
      "content-type": "application/json; charset=utf-8",
    });

    assert.equal(response.status, 200);
    assert.equal(server.requests.length, 1);
  });
});

test("外部オリジンからの書き込みは 403 で gRPC を呼ばない", async () => {
  for (const [label, handler, path, body, behavior] of writeRoutes) {
    await withServer(behavior, async (server) => {
      const response = await postWithHeaders(handler, path, body, {
        "content-type": "application/json",
        origin: "https://evil.example",
      });

      assert.equal(response.status, 403, label);
      assert.equal(server.requests.length, 0, `${label} は gRPC を呼ばない`);
    });
  }
});

test("同一オリジンからの application/json は許可する", async () => {
  for (const [label, handler, path, body, behavior] of writeRoutes) {
    await withServer(behavior, async (server) => {
      const response = await postWithHeaders(handler, path, body, {
        "content-type": "application/json",
        origin: "http://localhost",
        "sec-fetch-site": "same-origin",
      });

      assert.equal(response.status, 200, label);
      assert.equal(server.requests.length, 1, `${label} は gRPC を呼ぶ`);
    });
  }
});

test("Origin の無い CLI の要求は許可する", async () => {
  for (const [label, handler, path, body, behavior] of writeRoutes) {
    await withServer(behavior, async (server) => {
      const response = await postJson(handler, path, body);

      assert.equal(response.status, 200, label);
      assert.equal(server.requests.length, 1, `${label} は gRPC を呼ぶ`);
    });
  }
});

test("Sec-Fetch-Site が cross-site の書き込みは 403 で gRPC を呼ばない", async () => {
  for (const [label, handler, path, body, behavior] of writeRoutes) {
    await withServer(behavior, async (server) => {
      const response = await postWithHeaders(handler, path, body, {
        "content-type": "application/json",
        "sec-fetch-site": "cross-site",
      });

      assert.equal(response.status, 403, label);
      assert.equal(server.requests.length, 0, `${label} は gRPC を呼ばない`);
    });
  }
});

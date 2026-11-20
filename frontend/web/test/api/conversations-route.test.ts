// 会話の BFF Route Handler の契約（AC38B-1/AC38B-2/AC38B-4）。
//
// Next.js のサーバー側が backend の GetConversation を呼び、proto の JSON 表現を
// そのまま返す。ブラウザは backend へ直接つながず、この Route Handler を経由する。
// project は必須で、after_seq と limit は数値として検証する。
import test from "node:test";
import assert from "node:assert/strict";
import { NextRequest } from "next/server";
import { createConversationMessages, createConversationResponse } from "../support/fixtures.ts";
import { reserveClosedAddress, startTaktDashServer } from "../support/grpc-server.ts";
import { GET } from "../../app/api/conversations/route.ts";
import { closeBackendClient } from "../../src/grpc/client.ts";

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

const request = (query: string): NextRequest =>
  new NextRequest(`http://localhost/api/conversations${query}`);

test("project・session_id・after_seq・limit を backend へ渡し、proto の JSON を返す", async () => {
  const server = await startTaktDashServer({ conversation: createConversationResponse() });
  try {
    await withServerEnv(server.address, async () => {
      const response = await GET(request("?project=corporate-site&session_id=session-conv-1&after_seq=2&limit=50"));

      assert.equal(response.status, 200);
      assert.equal(server.conversationRequests.length, 1, "backend の GetConversation を1回呼ぶ");
      const seen = server.conversationRequests[0]!;
      assert.equal(seen.project, "corporate-site");
      assert.equal(seen.sessionId, "session-conv-1");
      assert.equal(seen.afterSeq, 2);
      assert.equal(seen.limit, 50);

      const body = await response.json();
      assert.equal(body.project, "corporate-site");
      assert.equal(body.sessionId, "session-conv-1");
      assert.equal(body.provider, "opencode");
      assert.equal(body.messages.length, 3);
      const message = createConversationMessages()[0]!;
      assert.equal(body.messages[0].seq, 1);
      assert.equal(body.messages[0].speaker, "CONVERSATION_SPEAKER_USER");
      assert.equal(body.messages[0].text, message.text);
      assert.equal(body.messages[0].occurredAt, message.occurredAt!.toISOString());
      assert.equal(body.nextAfterSeq, 3);
      assert.equal(body.hasMore, false);
    });
  } finally {
    await server.shutdown();
  }
});

test("session_id を省略すると backend にも渡さない", async () => {
  const server = await startTaktDashServer({ conversation: createConversationResponse() });
  try {
    await withServerEnv(server.address, async () => {
      const response = await GET(request("?project=corporate-site&after_seq=0&limit=200"));

      assert.equal(response.status, 200);
      assert.equal(server.conversationRequests[0]?.sessionId, undefined);
      assert.equal(server.conversationRequests[0]?.afterSeq, 0);
    });
  } finally {
    await server.shutdown();
  }
});

test("project が無い要求は 400 を返し、backend を呼ばない", async () => {
  const server = await startTaktDashServer({ conversation: createConversationResponse() });
  try {
    await withServerEnv(server.address, async () => {
      const missing = await GET(request(""));
      const blank = await GET(request("?project="));

      assert.equal(missing.status, 400);
      assert.equal(blank.status, 400);
      assert.equal(server.conversationRequests.length, 0);
    });
  } finally {
    await server.shutdown();
  }
});

test("after_seq と limit が数値でない要求は 400 を返す", async () => {
  const server = await startTaktDashServer({ conversation: createConversationResponse() });
  try {
    await withServerEnv(server.address, async () => {
      const badAfter = await GET(request("?project=corporate-site&after_seq=abc&limit=200"));
      const badLimit = await GET(request("?project=corporate-site&after_seq=0&limit=-1"));

      assert.equal(badAfter.status, 400);
      assert.equal(badLimit.status, 400);
      assert.equal(server.conversationRequests.length, 0);
    });
  } finally {
    await server.shutdown();
  }
});

test("backend に接続できないときは 503 を返す", async () => {
  const closedAddress = await reserveClosedAddress();
  await withServerEnv(closedAddress, async () => {
    const response = await GET(request("?project=corporate-site&after_seq=0&limit=200"));

    assert.equal(response.status, 503);
    const body = await response.json();
    assert.match(String(body.error ?? ""), /backend に接続できません/);
  });
});

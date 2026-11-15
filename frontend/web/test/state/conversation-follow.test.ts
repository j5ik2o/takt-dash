// 会話ページの差分追随の契約。
//
// 初期表示で取得済みのページを引き継いだ初回のポーリング、プロジェクト切替後の再取得、
// 前回の完了まで次のポーリングを開始しないこと、失敗してもカーソルとセッションを保つことを、
// 実際に送られた要求の内容と表示用のページで検証する。
import test from "node:test";
import assert from "node:assert/strict";
import { GetConversationResponse } from "../../src/proto/taktdash.ts";
import { createConversationMessages, createConversationResponse } from "../support/fixtures.ts";
import {
  restoreConversationResponse,
  type ConversationPage,
  type ConversationRequest,
} from "../../src/state/conversation.ts";
import { createConversationFollow } from "../../src/state/conversation-follow.ts";

const restore = (overrides: Parameters<typeof createConversationResponse>[0] = {}): ConversationPage =>
  restoreConversationResponse(GetConversationResponse.toJSON(createConversationResponse(overrides)));

const totalSkipped = (page: ConversationPage): number =>
  page.skipped.reduce((total, item) => total + item.count, 0);

type Deferred = {
  promise: Promise<ConversationPage>;
  resolve: (page: ConversationPage) => void;
};

const deferred = (): Deferred => {
  let resolve!: (page: ConversationPage) => void;
  const promise = new Promise<ConversationPage>((settle) => {
    resolve = settle;
  });
  return { promise, resolve };
};

test("初回のポーリングは初期ページのセッションとカーソルを引き継ぎ、先頭から再取得しない", async () => {
  const initialPage = restore({
    messages: createConversationMessages().slice(0, 2),
    sessionId: "session-conv-1",
    nextAfterSeq: 200,
  });
  const appended = restore({
    messages: [{ ...createConversationMessages()[2]!, seq: 201, text: "201件目の発言" }],
    sessionId: "session-conv-1",
    nextAfterSeq: 201,
  });
  const calls: ConversationRequest[] = [];
  const follow = createConversationFollow({
    initialProject: "corporate-site",
    initialPage,
    requestPage: async (request) => {
      calls.push(request);
      return appended;
    },
  });

  const result = await follow.followerFor("corporate-site").poll();

  assert.equal(calls.length, 1, "初回のポーリングは1回だけ取得する");
  assert.equal(calls[0]?.project, "corporate-site");
  assert.equal(calls[0]?.sessionId, "session-conv-1", "初期ページのセッションを引き継ぐ");
  assert.equal(calls[0]?.afterSeq, 200, "初期ページのカーソルから差分を取る");
  assert.equal(calls[0]?.limit, 200);
  assert.equal(result?.kind, "loaded", "初回のポーリングは表示用のページを返す");
  const page = result?.kind === "loaded" ? result.page : undefined;
  assert.deepEqual(
    page?.messages.map((message) => message.seq),
    [1, 2, 201],
    "応答の発言を既存の会話へ重複なく足す",
  );
});

test("プロジェクト切替後と初期プロジェクトへ戻った後は、初期ページを引き継がず先頭から取得する", async () => {
  const initialPage = restore({ sessionId: "session-conv-1", nextAfterSeq: 200 });
  const calls: ConversationRequest[] = [];
  const follow = createConversationFollow({
    initialProject: "corporate-site",
    initialPage,
    requestPage: async (request) => {
      calls.push(request);
      return restore({ project: request.project, sessionId: `session-${request.project}`, messages: [], nextAfterSeq: 250 });
    },
  });

  await follow.followerFor("corporate-site").poll();
  await follow.followerFor("esa").poll();
  await follow.followerFor("corporate-site").poll();

  assert.equal(calls.length, 3);
  assert.equal(calls[0]?.afterSeq, 200, "初回だけ初期ページのカーソルを引き継ぐ");
  assert.equal(calls[0]?.sessionId, "session-conv-1");
  assert.equal(calls[1]?.project, "esa");
  assert.equal(calls[1]?.afterSeq, 0, "切替後は先頭から取得する");
  assert.equal(calls[1]?.sessionId, undefined, "切替後は直前のセッションを引き継がない");
  assert.equal(calls[2]?.project, "corporate-site");
  assert.equal(calls[2]?.afterSeq, 0, "初期プロジェクトへ戻った後も先頭から取得する");
  assert.equal(calls[2]?.sessionId, undefined, "初期ページは一度だけ引き継ぐ");
});

test("前回のポーリングが終わるまで次を開始せず、重なった応答で skipped を二重に加算しない", async () => {
  const initialPage = restore({
    messages: createConversationMessages(),
    sessionId: "session-conv-1",
    nextAfterSeq: 3,
    skipped: [{ reason: "本文が空の行", count: 1 }],
  });
  const delayed = deferred();
  const calls: ConversationRequest[] = [];
  const follow = createConversationFollow({
    initialProject: "corporate-site",
    initialPage,
    requestPage: async (request) => {
      calls.push(request);
      if (calls.length === 1) {
        return delayed.promise;
      }
      return restore({ sessionId: "session-conv-1", nextAfterSeq: 3, messages: [] });
    },
  });
  const follower = follow.followerFor("corporate-site");

  const firstPoll = follower.poll();
  const overlapping = await follower.poll();

  assert.equal(overlapping, undefined, "前回の完了まで次のポーリングは開始しない");
  assert.equal(calls.length, 1, "重なったポーリングは要求を送らない");

  delayed.resolve(
    restore({
      sessionId: "session-conv-1",
      nextAfterSeq: 3,
      messages: [],
      skipped: [{ reason: "本文が空の行", count: 1 }],
    }),
  );
  const firstResult = await firstPoll;
  const firstPage = firstResult?.kind === "loaded" ? firstResult.page : undefined;
  assert.ok(firstPage, "初回のポーリングは表示用のページを返す");
  assert.equal(totalSkipped(firstPage), 2, "skipped は初期値に応答の1件だけを足す");
  assert.deepEqual(
    firstPage?.messages.map((message) => message.seq),
    [1, 2, 3],
  );

  const secondResult = await follower.poll();

  assert.equal(calls.length, 2, "前回の完了後は次のポーリングを開始する");
  assert.equal(calls[1]?.sessionId, "session-conv-1");
  assert.equal(calls[1]?.afterSeq, 3, "進んだカーソルから差分を取る");
  assert.equal(secondResult?.kind, "loaded");
});

test("更新の失敗ではカーソルとセッションを保ち、次のポーリングで同じ位置から再試行する", async () => {
  const initialPage = restore({
    messages: createConversationMessages(),
    sessionId: "session-conv-1",
    nextAfterSeq: 3,
  });
  const calls: ConversationRequest[] = [];
  const follow = createConversationFollow({
    initialProject: "corporate-site",
    initialPage,
    requestPage: async (request) => {
      calls.push(request);
      if (calls.length === 1) {
        throw new Error("接続先に接続できません。");
      }
      return restore({
        sessionId: "session-conv-1",
        nextAfterSeq: 4,
        messages: [{ ...createConversationMessages()[2]!, seq: 4, text: "4件目の発言" }],
      });
    },
  });
  const follower = follow.followerFor("corporate-site");

  const failed = await follower.poll();

  assert.equal(failed?.kind, "failed", "失敗を結果として返す");
  assert.equal(failed?.kind === "failed" ? failed.error : undefined, "接続先に接続できません。");

  const recovered = await follower.poll();

  assert.equal(calls[1]?.afterSeq, 3, "失敗してもカーソルを巻き戻さない");
  assert.equal(calls[1]?.sessionId, "session-conv-1", "失敗してもセッションを保つ");
  const page = recovered?.kind === "loaded" ? recovered.page : undefined;
  assert.deepEqual(
    page?.messages.map((message) => message.seq),
    [1, 2, 3, 4],
    "失敗前の本文を保ったまま、次の取得の発言を足す",
  );
});

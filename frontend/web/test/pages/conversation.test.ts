// 会話ページの初期表示の契約（AC38B-1）。
//
// サーバー側の ConversationsLoader がプロジェクト一覧を取得し、先頭のプロジェクトの
// 会話を GetConversation で取得して表示する。プロジェクトを選べる選択要素を持つ。
import test from "node:test";
import assert from "node:assert/strict";
import { renderToStaticMarkup } from "react-dom/server";
import {
  createConversationMessages,
  createConversationResponse,
  createDashboardState,
} from "../support/fixtures.ts";
import { startTaktDashServer } from "../support/grpc-server.ts";
import { ConversationsLoader } from "../../src/components/conversations-loader.tsx";

const withServerEnv = async <T>(address: string, run: () => Promise<T>): Promise<T> => {
  const previous = process.env.TAKTDASH_BACKEND_ADDR;
  process.env.TAKTDASH_BACKEND_ADDR = address;
  try {
    return await run();
  } finally {
    if (previous === undefined) {
      delete process.env.TAKTDASH_BACKEND_ADDR;
    } else {
      process.env.TAKTDASH_BACKEND_ADDR = previous;
    }
  }
};

test("会話ページはプロジェクトを選べ、先頭のプロジェクトの会話を時系列で表示する", async () => {
  const server = await startTaktDashServer({
    overview: createDashboardState(),
    conversation: createConversationResponse(),
  });
  try {
    await withServerEnv(server.address, async () => {
      const html = renderToStaticMarkup(await ConversationsLoader());

      assert.match(html, /<select/i, "対象プロジェクトを選べる");
      assert.match(html, /corporate-site/);
      assert.match(html, /esa/);
      assert.match(html, /会話/);

      const messages = createConversationMessages();
      const firstIndex = html.indexOf(messages[0]!.text);
      const secondIndex = html.indexOf(messages[1]!.text);
      const thirdIndex = html.indexOf(messages[2]!.text);
      assert.ok(firstIndex >= 0 && secondIndex >= 0 && thirdIndex >= 0, "初期表示に会話を出す");
      assert.ok(firstIndex < secondIndex && secondIndex < thirdIndex, "会話は seq の昇順に並ぶ");
      assert.match(html, /ユーザー/);
      assert.match(html, /manager/);
      assert.match(html, /システム/);

      const request = server.conversationRequests[0]!;
      assert.equal(request.project, "corporate-site");
      assert.equal(request.afterSeq, 0);
      assert.equal(request.limit, 200);

      assert.doesNotMatch(html, /<button/i, "会話ページは送信の操作を持たない");
      assert.doesNotMatch(html, /<textarea/i, "会話ページは返信の入力を持たない");
    });
  } finally {
    await server.shutdown();
  }
});

test("プロジェクトが無いときは会話を取得せず、その旨を表示する", async () => {
  const state = createDashboardState();
  state.projects = [];
  const server = await startTaktDashServer({ overview: state, conversation: createConversationResponse() });
  try {
    await withServerEnv(server.address, async () => {
      const html = renderToStaticMarkup(await ConversationsLoader());

      assert.match(html, /プロジェクトがありません/);
      assert.equal(server.conversationRequests.length, 0, "対象が無いときは GetConversation を呼ばない");
    });
  } finally {
    await server.shutdown();
  }
});

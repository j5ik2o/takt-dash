// 会話の復元・並び替え・差分取得の契約（AC38B-1/AC38B-2/AC38B-4）。
//
// backend が返す GetConversationResponse.toJSON の JSON は camelCase のフィールドと
// ISO 文字列の時刻を持つ。ブラウザ側は生成コードの fromJSON を値として使えないため、
// restoreConversationResponse が内部の型へ復元する。差分取得は after_seq と
// next_after_seq を使い、最初の応答の session_id を固定して同一の会話を追う。
import test from "node:test";
import assert from "node:assert/strict";
import {
  ConversationSpeaker,
  GetConversationResponse,
} from "../../src/proto/taktdash.ts";
import {
  createConversationResponse,
  createConversationMessages,
  createGarbledConversationMessage,
} from "../support/fixtures.ts";
import {
  beginConversationSwitch,
  conversationFailed,
  conversationLoaded,
  conversationSpeakerLabel,
  fetchConversationPages,
  mergeConversationPages,
  restoreConversationResponse,
  sortConversationMessages,
  type ConversationPage,
  type ConversationRequest,
  type ConversationView,
} from "../../src/state/conversation.ts";

const restore = (overrides: Parameters<typeof createConversationResponse>[0] = {}): ConversationPage =>
  restoreConversationResponse(GetConversationResponse.toJSON(createConversationResponse(overrides)));

const firstMessage = createConversationMessages()[0]!;

test("proto の JSON から会話を復元し、時刻・発話者・本文・続きの位置を保つ", () => {
  const message = firstMessage;
  const page = restore();

  assert.equal(page.project, "corporate-site");
  assert.equal(page.sessionId, "session-conv-1");
  assert.equal(page.provider, "opencode");
  assert.equal(page.messages.length, 3);

  const restored = page.messages[0]!;
  assert.equal(restored.seq, 1);
  assert.equal(restored.text, message.text);
  assert.equal(restored.speaker, ConversationSpeaker.CONVERSATION_SPEAKER_USER);
  assert.ok(restored.occurredAt instanceof Date, "時刻は Date へ復元する");
  assert.equal(restored.occurredAt?.toISOString(), message.occurredAt?.toISOString());

  assert.equal(page.nextAfterSeq, 3);
  assert.equal(page.hasMore, false);
  assert.deepEqual(page.skipped, []);
  assert.equal(page.unavailableReason, undefined);
});

test("unavailable_reason と skipped を応答の JSON から復元する", () => {
  const page = restore({
    messages: [],
    skipped: [{ reason: "解釈できない行", count: 2 }],
    unavailableReason: "会話の保存先がありません。",
  });

  assert.equal(page.unavailableReason, "会話の保存先がありません。");
  assert.equal(page.skipped.length, 1);
  assert.equal(page.skipped[0]?.reason, "解釈できない行");
  assert.equal(page.skipped[0]?.count, 2);
  assert.deepEqual(page.messages, []);
});

test("発話者はユーザー・manager・システムの表示名へ変換する", () => {
  assert.equal(conversationSpeakerLabel(ConversationSpeaker.CONVERSATION_SPEAKER_USER), "ユーザー");
  assert.equal(conversationSpeakerLabel(ConversationSpeaker.CONVERSATION_SPEAKER_MANAGER), "manager");
  assert.equal(conversationSpeakerLabel(ConversationSpeaker.CONVERSATION_SPEAKER_SYSTEM), "システム");
  assert.equal(conversationSpeakerLabel(ConversationSpeaker.CONVERSATION_SPEAKER_UNSPECIFIED), "不明");
  assert.equal(conversationSpeakerLabel(99), "不明");
});

test("会話の並びは seq の昇順に整える", () => {
  const page = restore();
  const shuffled = [page.messages[2]!, page.messages[0]!, page.messages[1]!];

  assert.deepEqual(
    sortConversationMessages(shuffled).map((message) => message.seq),
    [1, 2, 3],
  );
});

test("差分のページを足すと seq で重複を除き、skipped を理由ごとに合算する", () => {
  const base = restore({
    messages: createConversationMessages().slice(0, 2),
    nextAfterSeq: 2,
    skipped: [{ reason: "本文が空の行", count: 1 }],
  });
  const next = restore({
    messages: createConversationMessages().slice(1),
    nextAfterSeq: 3,
    skipped: [
      { reason: "本文が空の行", count: 2 },
      { reason: "解釈できない行", count: 1 },
    ],
  });

  const merged = mergeConversationPages(base, next);

  assert.deepEqual(
    merged.messages.map((message) => message.seq),
    [1, 2, 3],
    "既に表示した seq を重複させない",
  );
  assert.equal(merged.nextAfterSeq, 3);
  assert.equal(merged.skipped.find((item) => item.reason === "本文が空の行")?.count, 3);
  assert.equal(merged.skipped.find((item) => item.reason === "解釈できない行")?.count, 1);
});

test("unavailable の後に成功ページをマージすると理由を消す", () => {
  const base = restore({ messages: [], unavailableReason: "会話の保存先がありません。" });
  const next = restore({ messages: [firstMessage], unavailableReason: undefined });

  const merged = mergeConversationPages(base, next);

  assert.equal(merged.unavailableReason, undefined, "会話が取得できるようになったら理由を残さない");
  assert.deepEqual(
    merged.messages.map((message) => message.seq),
    [1],
    "復旧後の会話を表示する",
  );
});

test("成功の後に unavailable ページをマージすると理由を表示する", () => {
  const base = restore({ messages: createConversationMessages().slice(0, 2) });
  const next = restore({ messages: [], unavailableReason: "会話の保存先がありません。" });

  const merged = mergeConversationPages(base, next);

  assert.equal(merged.unavailableReason, "会話の保存先がありません。", "unavailable の理由は従来どおり表示する");
});

test("has_more の間は next_after_seq を次の after_seq にして続きを取り、最初の session_id を固定する", async () => {
  const fourth = { ...createConversationMessages()[2]!, seq: 4, text: "4件目の発言" };
  const first = restore({
    messages: createConversationMessages().slice(0, 2),
    nextAfterSeq: 4,
    hasMore: true,
    sessionId: "session-conv-1",
    skipped: [{ reason: "解釈できない行", count: 1 }],
  });
  const second = restore({
    messages: [fourth],
    nextAfterSeq: 4,
    hasMore: false,
    sessionId: "session-other",
  });
  const calls: ConversationRequest[] = [];
  const fetchPage = async (request: ConversationRequest): Promise<ConversationPage> => {
    calls.push(request);
    return calls.length === 1 ? first : second;
  };

  const result = await fetchConversationPages(fetchPage, "corporate-site");

  assert.deepEqual(
    calls.map((call) => call.afterSeq),
    [0, 4],
    "続きは末尾メッセージの seq ではなく応答の next_after_seq から取る",
  );
  assert.equal(calls[0]?.sessionId, undefined, "初回は session_id を省略する");
  assert.equal(calls[1]?.sessionId, "session-conv-1", "以降は最初の応答の session_id を固定する");
  assert.equal(calls[0]?.limit, 200, "初回の limit は 200 を使う");
  assert.equal(result.sessionId, "session-conv-1", "応答の session_id が変わっても最初の値を使い続ける");
  assert.equal(result.nextAfterSeq, 4);
  assert.deepEqual(
    result.page.messages.map((message) => message.seq),
    [1, 2, 4],
  );
});

test("ページの境目で同じ seq が返っても重複させない", async () => {
  const first = restore({
    messages: createConversationMessages().slice(0, 2),
    nextAfterSeq: 2,
    hasMore: true,
  });
  const second = restore({
    messages: createConversationMessages().slice(1),
    nextAfterSeq: 3,
    hasMore: false,
  });
  let calls = 0;

  const result = await fetchConversationPages(
    async () => {
      calls += 1;
      return calls === 1 ? first : second;
    },
    "corporate-site",
  );

  assert.deepEqual(
    result.page.messages.map((message) => message.seq),
    [1, 2, 3],
  );
});

test("has_more でもカーソルが進まないときは無限に取得しない", { timeout: 2_000 }, async () => {
  const stuck = restore({ messages: [], nextAfterSeq: 0, hasMore: true });
  let calls = 0;

  const result = await fetchConversationPages(
    async () => {
      calls += 1;
      return stuck;
    },
    "corporate-site",
  );

  assert.equal(calls, 1, "カーソルが進まない応答では取得を止める");
  assert.equal(result.nextAfterSeq, 0);
});

test("更新後も同じ会話の状態に新しい発言だけを差分で足す", async () => {
  const fourth = { ...createConversationMessages()[2]!, seq: 4, text: "4件目の発言" };
  const initialPage = restore({
    messages: createConversationMessages().slice(0, 2),
    nextAfterSeq: 4,
    skipped: [{ reason: "解釈できない行", count: 1 }],
  });
  const nextPage = restore({
    messages: [fourth],
    nextAfterSeq: 4,
  });
  const calls: ConversationRequest[] = [];
  const fetchPage = async (request: ConversationRequest): Promise<ConversationPage> => {
    calls.push(request);
    return calls.length === 1 ? initialPage : nextPage;
  };

  const initial = await fetchConversationPages(fetchPage, "corporate-site");
  const update = await fetchConversationPages(fetchPage, "corporate-site", {
    sessionId: initial.sessionId,
    afterSeq: initial.nextAfterSeq,
  });
  const displayedAfterUpdate = mergeConversationPages(initial.page, update.page);

  assert.deepEqual(
    initial.page.messages.map((message) => message.seq),
    [1, 2],
    "更新前の表示は取得済みの2件",
  );
  assert.deepEqual(
    displayedAfterUpdate.messages.map((message) => message.seq),
    [1, 2, 4],
    "同じ会話の表示に新しい発言だけが加わる",
  );
  assert.equal(calls[1]?.afterSeq, 4, "更新は末尾メッセージの seq ではなく応答の next_after_seq から差分を取る");
  assert.equal(calls[1]?.sessionId, "session-conv-1", "更新でも同じセッションを使う");
});

test("プロジェクト切替の開始は旧本文を破棄し、成功で同じプロジェクトの本文を表示する", () => {
  const loaded: ConversationView = {
    project: "corporate-site",
    page: restore({ messages: createConversationMessages() }),
    loading: false,
  };
  assert.match(loaded.page!.messages[0]!.text, /採用ページの見出し/, "切替前は旧プロジェクトの本文を持つ");

  const switching = beginConversationSwitch("esa");
  assert.equal(switching.project, "esa");
  assert.equal(switching.page, undefined, "切替中に旧プロジェクトの本文を残さない");
  assert.equal(switching.loading, true, "切替中は読み込み状態にする");

  const esaPage = restore({ project: "esa", sessionId: "session-esa" });
  const ready = conversationLoaded(switching, esaPage);
  assert.equal(ready.project, "esa");
  assert.deepEqual(
    ready.page?.messages.map((message) => message.seq),
    [1, 2, 3],
  );
  assert.equal(ready.loading, false);
  assert.equal(ready.error, undefined);
});

test("切替先の取得失敗では新しいプロジェクト名だけを残し、旧本文を表示しない", () => {
  const failed = conversationFailed(beginConversationSwitch("esa"), "接続先に接続できません。");

  assert.equal(failed.project, "esa");
  assert.equal(failed.page, undefined, "切替失敗時に旧プロジェクトの本文を新しいプロジェクトの本文として残さない");
  assert.equal(failed.loading, false);
  assert.equal(failed.error, "接続先に接続できません。", "失敗を警告として残す");
});

test("切替後の再取得は after_seq 0・session 未指定から始まる", async () => {
  const loaded: ConversationView = {
    project: "corporate-site",
    page: restore({ messages: createConversationMessages().slice(0, 2), nextAfterSeq: 4 }),
    loading: false,
  };
  assert.equal(loaded.page?.nextAfterSeq, 4, "切替前は続きの位置とセッションを持つ");

  const switching = beginConversationSwitch("esa");
  const calls: ConversationRequest[] = [];
  await fetchConversationPages(async (request) => {
    calls.push(request);
    return restore({ project: "esa", sessionId: "session-esa", messages: [] });
  }, switching.project);

  assert.equal(calls[0]?.project, "esa");
  assert.equal(calls[0]?.afterSeq, 0, "切替時は直前のカーソルを引き継がない");
  assert.equal(calls[0]?.sessionId, undefined, "切替時は直前のセッションを引き継がない");
});

test("更新の失敗は本文を保って警告し、成功で警告を解除する", () => {
  const base: ConversationView = {
    project: "corporate-site",
    page: restore({ messages: createConversationMessages().slice(0, 2) }),
    loading: false,
  };

  const failed = conversationFailed(base, "接続先に接続できません。");
  assert.equal(failed.page, base.page, "失敗しても表示済みの本文を保つ");
  assert.equal(failed.error, "接続先に接続できません。");
  assert.equal(failed.loading, false);

  const recovered = conversationLoaded(failed, restore({ messages: createConversationMessages() }));
  assert.equal(recovered.error, undefined, "成功で警告を解除する");
  assert.equal(recovered.loading, false);
  assert.deepEqual(
    recovered.page?.messages.map((message) => message.seq),
    [1, 2, 3],
  );
});

test("元記録の文字化けの印を notice として復元する", () => {
  const garbled = createGarbledConversationMessage();
  const page = restore({ messages: [garbled] });

  assert.equal(page.messages[0]?.notice, garbled.notice, "印を本文とは別の値として復元する");
  assert.equal(page.messages[0]?.text, garbled.text, "本文は変えずに復元する");
});

test("印のないメッセージは notice を持たない", () => {
  const page = restore();

  for (const message of page.messages) {
    assert.equal(message.notice, undefined, "印のないメッセージに印を付けない");
  }
});

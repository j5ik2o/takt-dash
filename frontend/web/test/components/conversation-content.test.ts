// 会話表示コンポーネントの契約（AC38B-1/AC38B-4）。
//
// 会話の表示は読み取り専用とし、発話者・時刻・本文を seq の昇順で出す。
// unavailable_reason は会話が無い理由として、skipped は理由と件数として補足表示する。
// 送信・返信の操作要素は持たない。
import test from "node:test";
import assert from "node:assert/strict";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { GetConversationResponse } from "../../src/proto/taktdash.ts";
import { createConversationMessages, createConversationResponse } from "../support/fixtures.ts";
import { formatDateTime } from "../../src/state/format.ts";
import { restoreConversationResponse, type ConversationPage } from "../../src/state/conversation.ts";
import { ConversationContent } from "../../src/components/conversation-content.tsx";

const restore = (overrides: Parameters<typeof createConversationResponse>[0] = {}): ConversationPage =>
  restoreConversationResponse(GetConversationResponse.toJSON(createConversationResponse(overrides)));

const render = (page: ConversationPage, project = "corporate-site"): string =>
  renderToStaticMarkup(createElement(ConversationContent, { project, page }));

const renderView = (
  project: string,
  view: { page?: ConversationPage; loading?: boolean; error?: string } = {},
): string => renderToStaticMarkup(createElement(ConversationContent, { project, ...view }));

test("会話は発話者・時刻・本文を seq の昇順で描画する", () => {
  const messages = createConversationMessages();
  const page = restore({ messages: [messages[2]!, messages[0]!, messages[1]!] });
  const html = render(page);

  assert.match(html, /会話/);
  assert.match(html, /corporate-site/);
  assert.match(html, /opencode/);

  const firstIndex = html.indexOf(messages[0]!.text);
  const secondIndex = html.indexOf(messages[1]!.text);
  const thirdIndex = html.indexOf(messages[2]!.text);
  assert.ok(firstIndex >= 0 && secondIndex >= 0 && thirdIndex >= 0, "すべての発言を描画する");
  assert.ok(firstIndex < secondIndex && secondIndex < thirdIndex, "発言は seq の昇順に並ぶ");

  assert.match(html, /ユーザー/);
  assert.match(html, /manager/);
  assert.match(html, /システム/);
  assert.ok(html.includes(formatDateTime(messages[0]!.occurredAt!)), "発言の時刻を描画する");
  assert.match(html, /seq\s*1/, "seq を描画する");
});

test("unavailable_reason を会話が無い理由として描画する", () => {
  const html = render(
    restore({
      messages: [],
      unavailableReason: "会話の保存先がありません。",
    }),
  );

  assert.match(html, /会話の保存先がありません。/);
});

test("skipped は理由と件数を補足表示する", () => {
  const html = render(
    restore({
      messages: [],
      skipped: [{ reason: "解釈できない行", count: 2 }],
    }),
  );

  assert.match(html, /解釈できない行/);
  assert.match(html, /2件/);
});

test("会話が空のときは空であることを描画する", () => {
  const html = render(restore({ messages: [] }));

  assert.match(html, /会話はありません/);
});

test("切替中は新しいプロジェクト名と読み込み状態を描画し、旧会話を出さない", () => {
  const oldPage = restore({ messages: createConversationMessages() });
  assert.match(render(oldPage), /採用ページの見出しを直してください。/, "切替前は旧プロジェクトの本文を出す");

  const html = renderView("esa", { loading: true });

  assert.match(html, /esa/);
  assert.match(html, /会話を読み込んでいます/);
  assert.doesNotMatch(html, /採用ページの見出しを直してください。/, "旧プロジェクトの本文を新しいプロジェクト名で出さない");
  assert.doesNotMatch(html, /opencode/, "取得前は旧プロジェクトのプロバイダを出さない");
});

test("切替先の取得失敗は新しいプロジェクト名と警告を描画し、旧会話を出さない", () => {
  const html = renderView("esa", { error: "接続先に接続できません。" });

  assert.match(html, /esa/);
  assert.match(html, /class="banner warn"/);
  assert.match(html, /会話を取得できません: 接続先に接続できません。/);
  assert.doesNotMatch(html, /採用ページの見出しを直してください。/);
});

test("更新失敗中も表示済みの会話を残したまま警告を描画する", () => {
  const page = restore({ messages: createConversationMessages().slice(0, 2) });
  const html = renderView("corporate-site", { page, error: "接続先に接続できません。" });

  assert.match(html, /採用ページの見出しを直してください。/, "失敗しても表示済みの本文を残す");
  assert.match(html, /class="banner warn"/);
  assert.match(html, /会話を取得できません: 接続先に接続できません。/);
});

test("会話の表示は送信・返信の操作要素を持たない", () => {
  const html = render(restore());

  assert.doesNotMatch(html, /<form/i);
  assert.doesNotMatch(html, /<button/i);
  assert.doesNotMatch(html, /<textarea/i);
  assert.doesNotMatch(html, /送信/);
});

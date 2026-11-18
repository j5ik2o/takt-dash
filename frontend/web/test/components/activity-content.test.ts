// アクティビティ表示コンポーネントの契約（issue #268）。
//
// 通知の本文は、改行が「\n」という文字の並び（エスケープされた形）で入っている
// 場合も、実際の改行で入っている場合も、改行として描画する。
// 改行の生の「\n」は画面に出ない。
import test from "node:test";
import assert from "node:assert/strict";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { ActivityKind, type GetActivityResponse } from "../../src/proto/taktdash.ts";
import { createActivityResponse, createDashboardState, NOW } from "../support/fixtures.ts";
import { ActivityContent } from "../../src/components/activity-content.tsx";

const renderWithBody = (body: string): string => {
  const response: GetActivityResponse = {
    ...createActivityResponse(),
    events: [
      {
        occurredAt: NOW,
        serverId: "mini",
        project: "takt",
        kind: ActivityKind.ACTIVITY_KIND_BLOCKED,
        body,
      },
    ],
  };
  return renderToStaticMarkup(createElement(ActivityContent, { state: createDashboardState(), response, now: NOW }));
};

test("blocked 通知の本文の改行の生の「\\n\\n」を改行として描画する", () => {
  const html = renderWithBody("ゴールブランチは無変更です。\\n\\n窓口で対応してください。");

  assert.ok(!html.includes("\\n"), "改行の生の「\\n」を画面に出さない");
  assert.ok(
    html.includes("ゴールブランチは無変更です。<br/><br/>窓口で対応してください。"),
    "段落の区切りを改行で描画する",
  );
});

test("blocked 通知の本文の実際の改行を改行として描画する", () => {
  const html = renderWithBody("1行目です。\n2行目です。");

  assert.ok(html.includes("1行目です。<br/>2行目です。"), "実際の改行を改行で描画する");
});

test("改行の無い本文はそのまま描画する", () => {
  const html = renderWithBody("問い合わせが届きました。");

  assert.match(html, /問い合わせが届きました。/);
});

// アクティビティ表示コンポーネントの契約（issue #268、#151）。
//
// タスクと run の失敗は、種類ごとの名前で、異常の行として出す。backend の起動は、
// サーバーの欄を空にせず taktDash と出す。
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

const renderEvent = (kind: ActivityKind, serverId: string, project: string | undefined, body: string): string => {
  const response: GetActivityResponse = {
    ...createActivityResponse(),
    events: [{ occurredAt: NOW, serverId, project, kind, body }],
  };
  return renderToStaticMarkup(createElement(ActivityContent, { state: createDashboardState(), response, now: NOW }));
};

const failureKinds: [ActivityKind, string][] = [
  [ActivityKind.ACTIVITY_KIND_TASK_FAILED, "タスクの失敗"],
  [ActivityKind.ACTIVITY_KIND_TASK_EXCEEDED, "タスクの上限超過"],
  [ActivityKind.ACTIVITY_KIND_TASK_PR_FAILED, "PR 失敗"],
  [ActivityKind.ACTIVITY_KIND_RUN_FAILED, "run の失敗"],
  [ActivityKind.ACTIVITY_KIND_RUN_ABORTED, "run の中断"],
];

for (const [kind, label] of failureKinds) {
  test(`${label}を、種類の名前と異常の行で出す`, () => {
    const html = renderEvent(kind, "mini", "takt", "タスク「build」が失敗しました。");

    assert.ok(html.includes(label), "種類の名前を出す");
    assert.ok(!html.includes("その他"), "種類を「その他」にしない");
    assert.match(html, /<li class="ev warn"/, "異常の行として出す");
    assert.match(html, /<span class="dot warn"/, "状態の点を警告色にする");
  });
}

test("backend の起動を、種類の名前と taktDash のサーバー欄で出す", () => {
  const html = renderEvent(ActivityKind.ACTIVITY_KIND_BACKEND_STARTED, "", undefined, "taktDash の backend が起動しました。");

  assert.ok(html.includes("backend の起動"), "種類の名前を出す");
  assert.match(html, /<span class="sv">taktDash<\/span>/, "サーバーの欄を空にしない");
  assert.match(html, /<li class="ev "/, "異常の行にしない");
  assert.match(html, /<span class="dot idle"/, "状態の点を待機の色にする");
});

// backend 不通の文面が、失敗の状態に応じて変わる契約（issue #152）。
//
// 応答なし（DEADLINE_EXCEEDED）と接続不可（UNAVAILABLE）を別の文面にし、
// 設定の確認を促すのは一度も応答を受けていないときだけに限る。接続できない
// 状態が60秒を超えたら、設定ではなく起動の確認を促す。
import test from "node:test";
import assert from "node:assert/strict";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import {
  BACKEND_UNAVAILABLE_MESSAGE,
  BackendUnavailableNotice,
  backendUnavailableMessage,
} from "../../src/components/backend-unavailable-notice.tsx";

test("応答なしの場合は接続不可と違う文面を返す", () => {
  const message = backendUnavailableMessage({ failure: "no-response", respondedBefore: true });
  assert.match(message, /backend が応答しません/);
  assert.doesNotMatch(message, /接続できません/);
});

test("応答なしの場合は一度も応答を受けていなくても応答なしの文面を返す", () => {
  const message = backendUnavailableMessage({ failure: "no-response", respondedBefore: false });
  assert.match(message, /backend が応答しません/);
  assert.doesNotMatch(message, /TAKTDASH_BACKEND_ADDR/);
});

test("一度も応答を受けていない接続不可では設定の確認を促す", () => {
  const message = backendUnavailableMessage({ failure: "unreachable", respondedBefore: false });
  assert.equal(message, BACKEND_UNAVAILABLE_MESSAGE);
  assert.match(message, /TAKTDASH_BACKEND_ADDR/);
});

test("一度応答を受けた後の接続不可では設定の確認を促さない", () => {
  const message = backendUnavailableMessage({
    failure: "unreachable",
    respondedBefore: true,
    durationMs: 10_000,
  });
  assert.match(message, /backend に接続できません/);
  assert.doesNotMatch(message, /TAKTDASH_BACKEND_ADDR/);
  assert.doesNotMatch(message, /起動を確認/);
});

test("接続できない状態が60秒を超えたら起動の確認を促す", () => {
  const message = backendUnavailableMessage({
    failure: "unreachable",
    respondedBefore: true,
    durationMs: 60_000,
  });
  assert.match(message, /backend の起動を確認してください/);
  assert.doesNotMatch(message, /TAKTDASH_BACKEND_ADDR/);
});

test("通知は渡された状況に応じた文面を描画する", () => {
  const html = renderToStaticMarkup(
    createElement(BackendUnavailableNotice, {
      situation: { failure: "no-response", respondedBefore: true },
    }),
  );
  assert.match(html, /backend が応答しません/);
});

test("通知は状況が無ければ従来の文面を描画する", () => {
  const html = renderToStaticMarkup(createElement(BackendUnavailableNotice));
  assert.ok(html.includes(BACKEND_UNAVAILABLE_MESSAGE));
});

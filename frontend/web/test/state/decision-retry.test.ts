// 判断待ちの操作のやり直しが、同じ操作として backend へ届くこと（issue #305）。
//
// 応答を受け取れなかった操作は、backend では済んでいることがある。やり直しは同じ鍵を
// 付けて送り、backend が書き込みを重ねないようにする。
import test from "node:test";
import assert from "node:assert/strict";
import { createActionableDashboardState } from "../support/fixtures.ts";
import {
  buildAnswerAction,
  IDEMPOTENCY_KEY_REUSE_MS,
  buildMergeAction,
  decisionAttempt,
  newIdempotencyKey,
  submitDecisionAction,
  type DecisionAction,
} from "../../src/state/decision-actions.ts";

const decisions = createActionableDashboardState().pendingDecisions;

const answerAction = (text: string): DecisionAction => {
  const decision = decisions.find((candidate) => candidate.inquiryId === "inquiry-text");
  assert.ok(decision);
  const action = buildAnswerAction(decision, { text });
  assert.ok(action);
  return action;
};

const mergeAction = (): DecisionAction => {
  const decision = decisions.find((candidate) => candidate.pullRequest !== undefined);
  assert.ok(decision);
  const action = buildMergeAction(decision);
  assert.ok(action);
  return action;
};

const sequentialKeys = (): (() => string) => {
  let next = 0;
  return () => `key-${++next}`;
};

test("鍵を付けた送信は、payload に idempotencyKey を加えて POST する", async () => {
  const bodies: unknown[] = [];
  const fetchImpl = (async (_input: RequestInfo | URL, init?: RequestInit) => {
    bodies.push(JSON.parse(String(init?.body)));
    return { ok: true, json: async () => ({ ok: true }) } as Response;
  }) as typeof fetch;
  const action = answerAction("補足します");

  await submitDecisionAction(action, fetchImpl, "key-1");

  assert.deepEqual(bodies, [{ ...action.payload, idempotencyKey: "key-1" }]);
});

const T0 = 1_800_000_000_000;

test("間を置かないやり直しは、同じ鍵を使う", () => {
  const newKey = sequentialKeys();
  const first = decisionAttempt(answerAction("補足します"), undefined, T0, newKey);

  const retried = decisionAttempt(answerAction("補足します"), first, T0 + 60_000, newKey);
  const retriedAgain = decisionAttempt(answerAction("補足します"), retried, T0 + 60_000 + IDEMPOTENCY_KEY_REUSE_MS - 1, newKey);

  assert.deepEqual(
    [first.idempotencyKey, retried.idempotencyKey, retriedAgain.idempotencyKey],
    ["key-1", "key-1", "key-1"],
  );
});

test("時間を置いたやり直しは、新しい操作として新しい鍵を使う", () => {
  const newKey = sequentialKeys();
  const first = decisionAttempt(mergeAction(), undefined, T0, newKey);

  const later = decisionAttempt(mergeAction(), first, T0 + IDEMPOTENCY_KEY_REUSE_MS, newKey);

  assert.deepEqual([first.idempotencyKey, later.idempotencyKey], ["key-1", "key-2"]);
});

test("内容や種類の違う操作は、別の鍵を使う", () => {
  const newKey = sequentialKeys();
  const first = decisionAttempt(answerAction("補足します"), undefined, T0, newKey);

  const edited = decisionAttempt(answerAction("書き直しました"), first, T0 + 1, newKey);
  const other = decisionAttempt(mergeAction(), edited, T0 + 2, newKey);

  assert.deepEqual(
    [first.idempotencyKey, edited.idempotencyKey, other.idempotencyKey],
    ["key-1", "key-2", "key-3"],
  );
});

test("成功した操作のあとは、同じ内容でも新しい鍵を使う", () => {
  const newKey = sequentialKeys();
  decisionAttempt(mergeAction(), undefined, T0, newKey);

  const next = decisionAttempt(mergeAction(), undefined, T0 + 1, newKey);

  assert.equal(next.idempotencyKey, "key-2");
});

test("鍵は backend が受け付ける形で、毎回異なる", () => {
  const keys = new Set(Array.from({ length: 100 }, () => newIdempotencyKey()));

  assert.equal(keys.size, 100);
  for (const key of keys) {
    assert.match(key, /^[0-9a-f]{32}$/);
  }
});

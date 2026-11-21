// 要素単位の自動更新の契約（LIVE-01）。
//
// 詳細画面は同じページを保ったまま、一定間隔で取得した値だけを差し替える。
// 取得に失敗した間は直前の値を保持して切断表示を出し、次の成功で表示を戻す。
// 前回の取得が終わる前に次の要求を重ねず、unmount（abort）後は取得しない。
import test from "node:test";
import assert from "node:assert/strict";
import { startPolling } from "../../src/components/polling.ts";

const delay = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));

const waitFor = async (predicate: () => boolean, timeoutMs = 1_000): Promise<void> => {
  const deadline = Date.now() + timeoutMs;
  while (!predicate()) {
    if (Date.now() > deadline) {
      throw new Error("待機条件が時間内に成立しませんでした。");
    }
    await delay(2);
  }
};

// 待機条件の時間切れや検証の失敗でテストが途中で終わってもポーリングが残らないよう、
// 各テストは finally で必ず abort する。
test("1秒を基本とする間隔で取得を繰り返し、同じ表示へ値を届ける", async () => {
  const controller = new AbortController();
  const values: number[] = [];
  let sequence = 0;
  try {
    startPolling({
      intervalMs: 5,
      signal: controller.signal,
      load: async () => {
        sequence += 1;
        return sequence;
      },
      onValue: (value: number) => values.push(value),
      onError: () => assert.fail("取得は成功する"),
    });

    await waitFor(() => values.length >= 2);
    controller.abort();
    const callsAtAbort = sequence;
    await delay(30);
    assert.equal(sequence, callsAtAbort, "abort 後は取得しない");
  } finally {
    controller.abort();
  }
});

test("取得に失敗した間は直前の値を保持して切断表示を出し、次の成功で表示を戻す", async () => {
  const controller = new AbortController();
  let current = 0;
  let disconnected = false;
  let failing = false;
  let nextValue = 1;

  try {
    startPolling({
      intervalMs: 5,
      signal: controller.signal,
      load: async () => {
        if (failing) {
          throw new Error("取得に失敗しました。");
        }
        return nextValue;
      },
      onValue: (value: number) => {
        current = value;
        disconnected = false;
      },
      onError: () => {
        disconnected = true;
      },
    });

    await waitFor(() => current === 1);

    failing = true;
    await waitFor(() => disconnected);
    assert.equal(current, 1, "失敗しても直前の値を保持する");

    failing = false;
    nextValue = 2;
    await waitFor(() => current === 2);
    assert.equal(disconnected, false, "次の成功で切断表示を消す");
  } finally {
    controller.abort();
  }
});

test("前回の取得が終わる前に次の要求を重ねない", async () => {
  const controller = new AbortController();
  let calls = 0;
  let resolveFirst!: (value: number) => void;
  const first = new Promise<number>((resolve) => {
    resolveFirst = resolve;
  });

  try {
    startPolling({
      intervalMs: 5,
      signal: controller.signal,
      load: () => {
        calls += 1;
        return first;
      },
      onValue: () => {},
      onError: () => assert.fail("取得は成功する"),
    });

    await waitFor(() => calls === 1);
    await delay(30);
    assert.equal(calls, 1, "未完了の取得がある間は新しい要求を出さない");

    resolveFirst(10);
    await waitFor(() => calls >= 2);
  } finally {
    controller.abort();
  }
});

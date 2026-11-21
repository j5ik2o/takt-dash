// run 詳細のログの追記分を取りに行くポーリングの契約。
//
// 開いている run 詳細の分だけ、一定間隔で前回の続きの位置を送って追記分を取り、画面の状態へ適用する。
// 前回の取得が終わるまで次を始めず、ページを離れたとき（abort）と、ログが残っていないと分かったときに
// 止まる。取得に失敗した間は直前の行を保ったまま失敗を知らせ、次の成功で続きから再開する。
import test from "node:test";
import assert from "node:assert/strict";
import { startRunLogUpdates } from "../../src/components/run-log-updates.ts";
import {
  initialRunLogView,
  type RunLogCursor,
  type RunLogView,
  type RunUpdates,
} from "../../src/state/run-log-stream.ts";

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

const texts = (view: RunLogView | undefined): string[] => (view?.lines ?? []).map((line) => line.text);

const response = (overrides: Partial<RunUpdates>): RunUpdates => ({
  epoch: 7,
  reset: false,
  lines: [],
  activity: { latestTool: undefined, toolUseCount: 0, updatedAt: undefined },
  logsRemoved: false,
  ...overrides,
});

test("最初の取得は位置を送らず、以後は直前に適用した応答の続きの位置を送って行を足す", async () => {
  const controller = new AbortController();
  const cursors: (RunLogCursor | undefined)[] = [];
  const replies = [
    response({ reset: true, lines: [{ sequence: 1, text: "a" }] }),
    response({ lines: [{ sequence: 2, text: "b" }] }),
  ];
  let latest: RunLogView | undefined;

  startRunLogUpdates({
    signal: controller.signal,
    intervalMs: 5,
    initialView: initialRunLogView("初期の行", false),
    fetchUpdates: async (cursor) => {
      cursors.push(cursor);
      return replies.shift() ?? response({});
    },
    onView: (view) => {
      latest = view;
    },
    onError: () => assert.fail("取得は成功する"),
  });

  await waitFor(() => cursors.length >= 3);
  controller.abort();

  assert.equal(cursors[0], undefined, "初期表示は位置を持たないため、最初の取得は位置を送らない");
  assert.deepEqual(cursors[1], { epoch: 7, sequence: 1 });
  assert.deepEqual(cursors[2], { epoch: 7, sequence: 2 }, "行の無い応答の後も前回の位置を送る");
  assert.deepEqual(texts(latest), ["a", "b"], "reset で初期の行を置き換え、差分を末尾へ足す");
});

test("abort の後は取得も適用もしない", async () => {
  const controller = new AbortController();
  let calls = 0;
  let views = 0;
  startRunLogUpdates({
    signal: controller.signal,
    intervalMs: 5,
    initialView: initialRunLogView("", false),
    fetchUpdates: async () => {
      calls += 1;
      return response({ reset: true });
    },
    onView: () => {
      views += 1;
    },
    onError: () => assert.fail("取得は成功する"),
  });

  await waitFor(() => calls >= 2);
  controller.abort();
  const callsAtAbort = calls;
  const viewsAtAbort = views;
  await delay(30);
  assert.equal(calls, callsAtAbort, "ページを離れた後は取得しない");
  assert.equal(views, viewsAtAbort, "ページを離れた後は画面の状態を変えない");
});

test("取得中に abort された応答は画面へ適用しない", async () => {
  const controller = new AbortController();
  let resolveFirst!: (value: RunUpdates) => void;
  let views = 0;
  startRunLogUpdates({
    signal: controller.signal,
    intervalMs: 5,
    initialView: initialRunLogView("", false),
    fetchUpdates: () =>
      new Promise<RunUpdates>((resolve) => {
        resolveFirst = resolve;
      }),
    onView: () => {
      views += 1;
    },
    onError: () => {},
  });

  await waitFor(() => resolveFirst !== undefined);
  controller.abort();
  resolveFirst(response({ reset: true, lines: [{ sequence: 1, text: "late" }] }));
  await delay(20);
  assert.equal(views, 0);
});

test("ログが残っていないと分かったら、その応答を適用して止まる", async () => {
  const controller = new AbortController();
  let calls = 0;
  let latest: RunLogView | undefined;
  startRunLogUpdates({
    signal: controller.signal,
    intervalMs: 5,
    initialView: initialRunLogView("", false),
    fetchUpdates: async () => {
      calls += 1;
      return response({ logsRemoved: true });
    },
    onView: (view) => {
      latest = view;
    },
    onError: () => assert.fail("取得は成功する"),
  });

  await waitFor(() => latest !== undefined);
  await delay(30);
  controller.abort();
  assert.equal(calls, 1, "ログが残っていない run は取得を続けない");
  assert.equal(latest?.logsRemoved, true);
});

test("前回の取得が終わる前に次の取得を重ねない", async () => {
  const controller = new AbortController();
  let calls = 0;
  let resolveFirst!: (value: RunUpdates) => void;
  startRunLogUpdates({
    signal: controller.signal,
    intervalMs: 5,
    initialView: initialRunLogView("", false),
    fetchUpdates: () => {
      calls += 1;
      if (calls === 1) {
        return new Promise<RunUpdates>((resolve) => {
          resolveFirst = resolve;
        });
      }
      return Promise.resolve(response({}));
    },
    onView: () => {},
    onError: () => assert.fail("取得は成功する"),
  });

  await waitFor(() => calls === 1);
  await delay(30);
  assert.equal(calls, 1, "未完了の取得がある間は新しい要求を出さない");
  resolveFirst(response({ reset: true }));
  await waitFor(() => calls >= 2);
  controller.abort();
});

test("取得に失敗した間は直前の行と位置を保ち、次の成功で続きの位置から再開する", async () => {
  const controller = new AbortController();
  const cursors: (RunLogCursor | undefined)[] = [];
  let failing = false;
  let errors = 0;
  let latest: RunLogView | undefined;
  startRunLogUpdates({
    signal: controller.signal,
    intervalMs: 5,
    initialView: initialRunLogView("", false),
    fetchUpdates: async (cursor) => {
      cursors.push(cursor);
      if (cursors.length === 1) {
        return response({ reset: true, lines: [{ sequence: 4, text: "a" }] });
      }
      if (failing) {
        throw new Error("取得に失敗しました。");
      }
      return response({ lines: [{ sequence: 5, text: "b" }] });
    },
    onView: (view) => {
      latest = view;
      if (texts(view).length === 1) {
        failing = true;
      }
    },
    onError: () => {
      errors += 1;
      failing = false;
    },
  });

  await waitFor(() => texts(latest).length === 2);
  controller.abort();
  assert.equal(errors, 1, "失敗を知らせる");
  assert.deepEqual(texts(latest), ["a", "b"], "失敗の間も直前の行を保ち、成功で足す");
  assert.ok(cursors.slice(1).every((cursor) => cursor?.epoch === 7 && cursor.sequence === 4), "失敗の後も同じ続きの位置を送る");
});

test("最初から abort 済みのときは取得しない", async () => {
  const controller = new AbortController();
  controller.abort();
  let calls = 0;
  startRunLogUpdates({
    signal: controller.signal,
    intervalMs: 5,
    initialView: initialRunLogView("", false),
    fetchUpdates: async () => {
      calls += 1;
      return response({});
    },
    onView: () => {},
    onError: () => {},
  });
  await delay(20);
  assert.equal(calls, 0);
});

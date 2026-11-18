// run 詳細のログの追記分を画面の状態へ適用する契約。
//
// 初期表示は RSC が GetRunDetail から得た log_tail の行です。ポーリングで届いた追記分は、
// reset なら行を置き換え、そうでなければ末尾へ足す。保持する行は上限で止め、古い行から捨てる。
// 続きの位置（世代と最後の通し番号）と活動の印は、届いた応答の値へ差し替える。
import test from "node:test";
import assert from "node:assert/strict";
import {
  MAX_RUN_LOG_LINES,
  applyRunUpdates,
  initialRunLogView,
  restoreRunUpdates,
  runUpdatesUrl,
  type RunLogView,
  type RunUpdates,
} from "../../src/state/run-log-stream.ts";

const texts = (view: RunLogView): string[] => view.lines.map((line) => line.text);

const updates = (overrides: Partial<RunUpdates> = {}): RunUpdates => ({
  epoch: 7,
  reset: false,
  lines: [],
  activity: { latestTool: undefined, toolUseCount: 0, updatedAt: undefined },
  logsRemoved: false,
  ...overrides,
});

test("初期表示は log_tail の行で、続きの位置も活動の印もまだ持たない", () => {
  const view = initialRunLogView("2026-11-13T09:01:00Z step_complete plan\n2026-11-13T09:05:00Z step_start implement", false);
  assert.deepEqual(texts(view), ["2026-11-13T09:01:00Z step_complete plan", "2026-11-13T09:05:00Z step_start implement"]);
  assert.equal(view.cursor, undefined, "最初のポーリングは位置を送らず、全体を受け取る");
  assert.equal(view.activity, undefined);
  assert.equal(view.logsRemoved, false);
});

test("空の log_tail は0行で、ログが残っていない run はそのことを保持する", () => {
  assert.deepEqual(texts(initialRunLogView("", false)), []);
  assert.equal(initialRunLogView("", true).logsRemoved, true);
});

test("reset の応答は行を置き換え、続きの位置を最後の行の番号にする", () => {
  const view = initialRunLogView("初期の行", false);
  const next = applyRunUpdates(
    view,
    updates({ reset: true, lines: [{ sequence: 3, text: "a" }, { sequence: 4, text: "b" }] }),
  );
  assert.deepEqual(texts(next), ["a", "b"], "初期の行を重複させず置き換える");
  assert.deepEqual(next.cursor, { epoch: 7, sequence: 4 });
});

test("差分の応答は既存の行を保ったまま末尾へ足す", () => {
  const base = applyRunUpdates(initialRunLogView("", false), updates({ reset: true, lines: [{ sequence: 1, text: "a" }] }));
  const next = applyRunUpdates(base, updates({ lines: [{ sequence: 2, text: "b" }, { sequence: 3, text: "c" }] }));
  assert.deepEqual(texts(next), ["a", "b", "c"]);
  assert.deepEqual(next.cursor, { epoch: 7, sequence: 3 });
});

test("行の無い差分の応答は行を変えず、続きの位置を保つ", () => {
  const base = applyRunUpdates(initialRunLogView("", false), updates({ reset: true, lines: [{ sequence: 5, text: "a" }] }));
  const next = applyRunUpdates(base, updates());
  assert.deepEqual(texts(next), ["a"]);
  assert.deepEqual(next.cursor, { epoch: 7, sequence: 5 }, "行が無い応答の後も前回の位置から続ける");
});

test("行の無い reset の応答は行を空にする", () => {
  const base = applyRunUpdates(initialRunLogView("", false), updates({ reset: true, lines: [{ sequence: 5, text: "a" }] }));
  const next = applyRunUpdates(base, updates({ epoch: 9, reset: true, lines: [] }));
  assert.deepEqual(texts(next), [], "reset は行が無くても前の世代の行を残さない");
});

test("保持する行は上限で止め、古い行から捨てる", () => {
  const many = Array.from({ length: MAX_RUN_LOG_LINES + 5 }, (_, index) => ({ sequence: index + 1, text: `line-${index + 1}` }));
  const next = applyRunUpdates(initialRunLogView("", false), updates({ reset: true, lines: many }));
  assert.equal(next.lines.length, MAX_RUN_LOG_LINES);
  assert.equal(texts(next)[0], "line-6", "古い行から捨てる");
  assert.equal(texts(next).at(-1), `line-${MAX_RUN_LOG_LINES + 5}`);

  const appended = applyRunUpdates(next, updates({ lines: [{ sequence: MAX_RUN_LOG_LINES + 6, text: "newest" }] }));
  assert.equal(appended.lines.length, MAX_RUN_LOG_LINES, "差分で足しても上限を超えない");
  assert.equal(texts(appended).at(-1), "newest");
  assert.equal(texts(appended)[0], "line-7");
});

test("活動の印とログが残っていないことは、届いた応答の値へ差し替える", () => {
  const updatedAt = new Date("2026-11-13T09:12:00Z");
  const next = applyRunUpdates(
    initialRunLogView("", false),
    updates({ activity: { latestTool: "Bash", toolUseCount: 12, updatedAt } }),
  );
  assert.deepEqual(next.activity, { latestTool: "Bash", toolUseCount: 12, updatedAt });

  const removed = applyRunUpdates(next, updates({ logsRemoved: true }));
  assert.equal(removed.logsRemoved, true);
});

test("適用は元の状態を書き換えない", () => {
  const base = applyRunUpdates(initialRunLogView("", false), updates({ reset: true, lines: [{ sequence: 1, text: "a" }] }));
  applyRunUpdates(base, updates({ lines: [{ sequence: 2, text: "b" }] }));
  assert.deepEqual(texts(base), ["a"]);
  assert.deepEqual(base.cursor, { epoch: 7, sequence: 1 });
});

test("Route Handler の URL は、位置が無ければ問い合わせを付けず、あれば世代と番号を付ける", () => {
  assert.equal(runUpdatesUrl("run-aaa", undefined), "/api/runs/run-aaa/updates");
  assert.equal(runUpdatesUrl("run-aaa", { epoch: 7, sequence: 12 }), "/api/runs/run-aaa/updates?epoch=7&after=12");
});

test("応答の JSON（proto の JSON 表現）から追記分を復元する", () => {
  // proto の JSON 表現は既定値（reset=false、空の行、0件）を省き、int64 を文字列で表すことがある。
  assert.deepEqual(
    restoreRunUpdates({
      runSlug: "run-aaa",
      epoch: "1731488400000",
      reset: true,
      lines: [{ sequence: "3", text: "2026-11-13T09:05:00Z step_start implement" }],
      activity: { latestTool: "Bash", toolUseCount: "12", updatedAt: "2026-11-13T09:12:00Z" },
    }),
    {
      epoch: 1731488400000,
      reset: true,
      lines: [{ sequence: 3, text: "2026-11-13T09:05:00Z step_start implement" }],
      activity: { latestTool: "Bash", toolUseCount: 12, updatedAt: new Date("2026-11-13T09:12:00Z") },
      logsRemoved: false,
    },
  );
  assert.deepEqual(restoreRunUpdates({ runSlug: "run-aaa", epoch: 7, activity: {} }), {
    epoch: 7,
    reset: false,
    lines: [],
    activity: { latestTool: undefined, toolUseCount: 0, updatedAt: undefined },
    logsRemoved: false,
  });
  assert.equal(restoreRunUpdates({ runSlug: "run-aaa", epoch: 7, logsRemoved: true }).logsRemoved, true);
});

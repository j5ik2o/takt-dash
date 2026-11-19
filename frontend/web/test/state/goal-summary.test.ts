// ゴール表示の見出しとタスク進捗の契約（issue #78）。
// 一覧名の省略は backend の印を使い、第一文の抽出は括弧の外で行う。
//
// 目的の全文をカードに出すと、引用や括弧の内側の句点でも切れてしまう。見出しは
// 括弧・引用の外側の最初の文末記号までを1文として取り出し、タスクの進み具合は
// 6種類の状態の合計と失敗の合計から作る。表示の判断を持つ関数だけを観測する。
import test from "node:test";
import assert from "node:assert/strict";
import { goalDisplayName, goalListHeading, goalTaskProgress } from "../../src/state/goal-summary.ts";
import { type TaskStatusCounts } from "../../src/proto/taktdash.ts";

test("括弧と引用の外側にある最初の句点で見出しを切る", () => {
  assert.equal(
    goalListHeading("採用ページのレイアウトを直す。対象はトップページだけである。", false),
    "採用ページのレイアウトを直す。",
  );
});

test("句点以外の文末記号でも見出しを切る", () => {
  assert.equal(
    goalListHeading("採用ページのレイアウトを直す！対象はトップページだけである。", false),
    "採用ページのレイアウトを直す！",
  );
  assert.equal(
    goalListHeading("採用ページのレイアウトを直す？対象はトップページだけである。", false),
    "採用ページのレイアウトを直す？",
  );
});

test("引用符と丸括弧の内側の句点と疑問符では見出しを切らない", () => {
  assert.equal(
    goalListHeading("オーナーの指示（2026-11-16「E2Eないだろ？テストピラミッドを健全化してほしい。カバレッジも80%以上」）に基づき、takt-dash のテストピラミッドを健全化する土台を作る。対象は issue #106 である。", false),
    "オーナーの指示（2026-11-16「E2Eないだろ？テストピラミッドを健全化してほしい。カバレッジも80%以上」）に基づき、takt-dash のテストピラミッドを健全化する土台を作る。",
  );
});

test("閉じ忘れた丸括弧の内側の句点では見出しを切らない", () => {
  assert.equal(
    goalListHeading("補足（この括弧は閉じていない。見出しを切ってはいけない", false),
    "補足（この括弧は閉じていない。見出しを切ってはいけない",
  );
});

test("対応する開き括弧の無い閉じ括弧では深さを負にせず、外側の句点で切る", () => {
  assert.equal(goalListHeading("閉じ括弧だけ）の文。次の文。", false), "閉じ括弧だけ）の文。");
});

test("文末記号が無ければ前後の空白を除いた全体を見出しにする", () => {
  assert.equal(goalListHeading("  採用ページのレイアウトを直す  ", false), "採用ページのレイアウトを直す");
});

test("先頭の文の前にある空白は見出しに含めない", () => {
  assert.equal(goalListHeading("  前後の空白を除く。次の文。", false), "前後の空白を除く。");
});

test("改行は文の境界として扱わない", () => {
  assert.equal(goalListHeading("1行目\n2行目。3行目。", false), "1行目\n2行目。");
});

const counts = (overrides: Partial<TaskStatusCounts> = {}): TaskStatusCounts => ({
  pending: 0,
  running: 0,
  completed: 0,
  failed: 0,
  exceeded: 0,
  prFailed: 0,
  ...overrides,
});

test("タスクの全体は6種類の状態の合計で、失敗は失敗・超過・PR失敗の合計にする", () => {
  const progress = goalTaskProgress({
    taskCounts: counts({ pending: 2, running: 1, completed: 3, failed: 1, exceeded: 1, prFailed: 1 }),
  });

  assert.deepEqual(progress, { completed: 3, total: 9, failed: 3, running: 1 });
});

test("失敗だけを数え、完了・待機・実行中を失敗に混ぜない", () => {
  const progress = goalTaskProgress({
    taskCounts: counts({ pending: 5, running: 4, completed: 6, failed: 0, exceeded: 0, prFailed: 0 }),
  });

  assert.deepEqual(progress, { completed: 6, total: 15, failed: 0, running: 4 });
});

test("タスクの件数が無いゴールは全件0として扱う", () => {
  assert.deepEqual(goalTaskProgress({ taskCounts: undefined }), { completed: 0, total: 0, failed: 0, running: 0 });
});

// 2026-11-16 の本番で実際に表示された、256 バイトで切れた preview（issue #153）。
const TRUNCATED_PREVIEW =
  "taktDash の関門と道具の穴4件を解消する。対象は issue #132（https://github.com/j5ik2o/takt-dash/issues/132 、proto-check が index 依存で、proto を変更するタスクは machine_gate を通過できない）、issue #98（https://";

test("切り詰められた preview の表示名には省略記号を付ける", () => {
  assert.equal(goalDisplayName(TRUNCATED_PREVIEW, true), `${TRUNCATED_PREVIEW}…`);
});

test("文で完結する長い名前には省略記号を付けない", () => {
  const complete = `${"あ".repeat(84)}。`;
  assert.equal(goalDisplayName(complete, false), complete);
});

test("閉じ括弧で終わる長い名前には省略記号を付けない", () => {
  const complete = `${"あ".repeat(83)}（補足）`;
  assert.equal(goalDisplayName(complete, false), complete);
});

test("短い名前はそのまま表示する", () => {
  assert.equal(goalDisplayName("採用ページのレイアウトを直す", false), "採用ページのレイアウトを直す");
});

test("見出しが最初の文で完結する場合は省略記号を付けない", () => {
  assert.equal(goalListHeading(TRUNCATED_PREVIEW, true), "taktDash の関門と道具の穴4件を解消する。");
});

test("最初の文自体が preview で切れている見出しには省略記号を付ける", () => {
  const cut = `${"あ".repeat(84)}htt`;
  assert.equal(goalListHeading(cut, true), `${cut}…`);
});

test("文末記号の無い短い名前の見出しには省略記号を付けない", () => {
  assert.equal(goalListHeading("採用ページのレイアウトを直す", false), "採用ページのレイアウトを直す");
});

test("短い句点終わりでも省略の印があれば目的全体に省略記号を付ける", () => {
  assert.equal(goalDisplayName("確認。", true), "確認。…");
  assert.equal(goalListHeading("確認。", true), "確認。");
});

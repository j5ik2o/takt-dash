// ゴール表示の見出しとタスク進捗の契約（issue #78）。
// 一覧の名前は takt-mcp の preview であり、切り詰めの見せ方もここで決める（issue #153）。
//
// 目的の全文をカードに出すと、引用や括弧の内側の句点でも切れてしまう。見出しは
// 括弧・引用の外側の最初の文末記号までを1文として取り出し、タスクの進み具合は
// 6種類の状態の合計と失敗の合計から作る。表示の判断を持つ関数だけを観測する。
import test from "node:test";
import assert from "node:assert/strict";
import { goalDisplayName, goalHeading, goalListHeading, goalTaskProgress } from "../../src/state/goal-summary.ts";
import { type TaskStatusCounts } from "../../src/proto/taktdash.ts";

test("括弧と引用の外側にある最初の句点で見出しを切る", () => {
  assert.equal(
    goalHeading("採用ページのレイアウトを直す。対象はトップページだけである。"),
    "採用ページのレイアウトを直す。",
  );
});

test("句点以外の文末記号でも見出しを切る", () => {
  assert.equal(
    goalHeading("採用ページのレイアウトを直す！対象はトップページだけである。"),
    "採用ページのレイアウトを直す！",
  );
  assert.equal(
    goalHeading("採用ページのレイアウトを直す？対象はトップページだけである。"),
    "採用ページのレイアウトを直す？",
  );
});

test("引用符と丸括弧の内側の句点と疑問符では見出しを切らない", () => {
  assert.equal(
    goalHeading(
      "オーナーの指示（2026-11-16「E2Eないだろ？テストピラミッドを健全化してほしい。カバレッジも80%以上」）に基づき、takt-dash のテストピラミッドを健全化する土台を作る。対象は issue #106 である。",
    ),
    "オーナーの指示（2026-11-16「E2Eないだろ？テストピラミッドを健全化してほしい。カバレッジも80%以上」）に基づき、takt-dash のテストピラミッドを健全化する土台を作る。",
  );
});

test("閉じ忘れた丸括弧の内側の句点では見出しを切らない", () => {
  assert.equal(
    goalHeading("補足（この括弧は閉じていない。見出しを切ってはいけない"),
    "補足（この括弧は閉じていない。見出しを切ってはいけない",
  );
});

test("対応する開き括弧の無い閉じ括弧では深さを負にせず、外側の句点で切る", () => {
  assert.equal(goalHeading("閉じ括弧だけ）の文。次の文。"), "閉じ括弧だけ）の文。");
});

test("文末記号が無ければ前後の空白を除いた全体を見出しにする", () => {
  assert.equal(goalHeading("  採用ページのレイアウトを直す  "), "採用ページのレイアウトを直す");
});

test("先頭の文の前にある空白は見出しに含めない", () => {
  assert.equal(goalHeading("  前後の空白を除く。次の文。"), "前後の空白を除く。");
});

test("改行は文の境界として扱わない", () => {
  assert.equal(goalHeading("1行目\n2行目。3行目。"), "1行目\n2行目。");
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

// 切り詰めの印は backend が takt の応答から判定して nameTruncated に載せる（issue #230）。
// 表示側は文の形やバイト長から推定しない。
const cut = (name: string) => ({ name, nameTruncated: true });
const whole = (name: string) => ({ name, nameTruncated: false });

test("印のある preview の表示名には省略記号を付ける", () => {
  assert.equal(goalDisplayName(cut(TRUNCATED_PREVIEW)), `${TRUNCATED_PREVIEW}…`);
});

test("QA で見つかった、文の途中で切れた短い目的にも印があれば省略記号を付ける", () => {
  assert.equal(goalDisplayName(cut("土台と完成申請の宛先は、プルリク")), "土台と完成申請の宛先は、プルリク…");
});

test("印の無い名前は、256 バイトで文末記号が無くても省略記号を付けない", () => {
  const longWithoutEnd = `${"あ".repeat(84)}htt`;
  assert.equal(goalDisplayName(whole(longWithoutEnd)), longWithoutEnd);
  assert.equal(goalDisplayName(whole(TRUNCATED_PREVIEW)), TRUNCATED_PREVIEW);
});

test("印のある名前は、文末記号で終わっていても続きがあるので省略記号を付ける", () => {
  const endsWithStop = `${"あ".repeat(84)}。`;
  assert.equal(goalDisplayName(cut(endsWithStop)), `${endsWithStop}…`);
});

test("短い名前はそのまま表示する", () => {
  assert.equal(goalDisplayName(whole("採用ページのレイアウトを直す")), "採用ページのレイアウトを直す");
});

test("見出しが最初の文で完結する場合は、印があっても省略記号を付けない", () => {
  assert.equal(goalListHeading(cut(TRUNCATED_PREVIEW)), "taktDash の関門と道具の穴4件を解消する。");
});

test("preview がちょうど最初の文の文末で切れた見出しには省略記号を付けない", () => {
  assert.equal(goalListHeading(cut("最初の文で切れた。")), "最初の文で切れた。");
});

test("最初の文自体が切れている見出しには、印があるときだけ省略記号を付ける", () => {
  const text = `${"あ".repeat(84)}htt`;
  assert.equal(goalListHeading(cut(text)), `${text}…`);
  assert.equal(goalListHeading(whole(text)), text);
});

test("文末記号の無い短い名前の見出しには、印が無ければ省略記号を付けない", () => {
  assert.equal(goalListHeading(whole("採用ページのレイアウトを直す")), "採用ページのレイアウトを直す");
});

// 生成した名前で、印と省略記号の関係を確かめる（設計の P6・P7）。
const PARTS = ["あ", "😀", "。", "！", "「", "」", "（", "）", " ", "\n", "…", "https://example.test/a", "a"];

const generatedNames = (): string[] => {
  let seed = 230;
  const next = () => {
    seed = (seed * 1_103_515_245 + 12_345) % 2_147_483_648;
    return seed;
  };
  return Array.from({ length: 500 }, () =>
    Array.from({ length: next() % 30 }, () => PARTS[next() % PARTS.length]).join(""),
  );
};

test("表示名は印のあるときだけ、前後の空白を除いた名前の末尾に省略記号を1つ足す（生成した名前）", () => {
  for (const name of generatedNames()) {
    assert.equal(goalDisplayName(whole(name)), name, JSON.stringify(name));
    assert.equal(goalDisplayName(cut(name)), `${name.trim()}…`, JSON.stringify(name));
  }
});

test("見出しは印が無ければ最初の文のまま、印があれば最初の文が閉じていないときだけ省略記号を足す（生成した名前）", () => {
  for (const name of generatedNames()) {
    const heading = goalHeading(name);
    assert.equal(goalListHeading(whole(name)), heading, JSON.stringify(name));
    const marked = goalListHeading(cut(name));
    if (heading !== name.trim()) {
      // 最初の文が文末記号で閉じ、その後ろに続きがある。
      assert.equal(marked, heading, JSON.stringify(name));
    } else {
      assert.ok(marked === heading || marked === `${heading}…`, JSON.stringify(name));
    }
  }
});

// Web プロセスのログ行へ UTC ISO 時刻を付ける契約（issue #186）。
//
// ラッパーは Next.js 自身の起動・Ready・エラーを含む行へ backend と同じ
// `YYYY-MM-DDTHH:MM:SS.sssZ` 形式の時刻を1行につき1回だけ前置し、メッセージ本文は
// 変えない。改行で終わらない部分行は、続きが届くか終了の flush が呼ばれるまで保持する。
import test from "node:test";
import assert from "node:assert/strict";
import { createLinePrefixer, formatLine } from "../../scripts/web-log-prefix.mjs";

const STAMP = "2026-11-19T06:31:20.123Z";
const fixedNow = (): Date => new Date(STAMP);

test("時刻を持たない1行には backend と同じ形式の UTC 時刻と空白を前置する", () => {
  const prefixer = createLinePrefixer({ now: fixedNow });

  const out = prefixer.push("Ready in 512ms\n");

  assert.equal(out, `${STAMP} Ready in 512ms\n`);
});

test("複数行のチャンクは行ごとに時刻を付け、末尾の部分行は次のチャンクへ持ち越す", () => {
  const prefixer = createLinePrefixer({ now: fixedNow });

  const first = prefixer.push("line A\nline B\npartial");
  const second = prefixer.push(" continues\n");

  assert.equal(first, `${STAMP} line A\n${STAMP} line B\n`);
  assert.equal(second, `${STAMP} partial continues\n`);
});

test("改行なしの1チャンクは次の改行まで出力しない", () => {
  const prefixer = createLinePrefixer({ now: fixedNow });

  assert.equal(prefixer.push("Ready in "), "");
  assert.equal(prefixer.push("512ms\n"), `${STAMP} Ready in 512ms\n`);
});

test("flush は保持中の部分行へ時刻を1回前置して返し、次の出力は空になる", () => {
  const prefixer = createLinePrefixer({ now: fixedNow });

  assert.equal(prefixer.push("last partial"), "");
  assert.equal(prefixer.flush(), `${STAMP} last partial`);
  assert.equal(prefixer.flush(), "");
});

test("flush は部分行が無いとき何も出さない", () => {
  const prefixer = createLinePrefixer({ now: fixedNow });

  assert.equal(prefixer.push("done\n"), `${STAMP} done\n`);
  assert.equal(prefixer.flush(), "");
});

test("メッセージ本文は変えない", () => {
  const prefixer = createLinePrefixer({ now: fixedNow });
  const message = "⨯ Error: 既存の文言と項目は変えない (code=500)\n";

  assert.equal(prefixer.push(message), `${STAMP} ${message}`);
});

test("改行だけの空行にも時刻を付け、空文字のチャンクは何も出さない", () => {
  const prefixer = createLinePrefixer({ now: fixedNow });

  assert.equal(prefixer.push("\n"), `${STAMP} \n`);
  assert.equal(prefixer.push(""), "");
});

test("formatLine は行1本の時刻付き表現を作る", () => {
  assert.equal(formatLine("GET / 200", fixedNow()), `${STAMP} GET / 200`);
});

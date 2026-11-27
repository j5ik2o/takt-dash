// Web のログの各行に時刻を付ける部分（issue #186、docs/design/issue-186.md）。
//
// createLineStamper は流れ 1 本ぶんの状態を持ち、受け取ったバイト列の行の頭に
// 「UTC の RFC 3339（ミリ秒まで）と空白 1 つ」を差し込んで返す。時計は注入できる。
import test from "node:test";
import assert from "node:assert/strict";
import { createLineStamper, formatStamp } from "../../scripts/line-stamper.mjs";
import { STAMP_LEN, STAMP_RE, unstamp } from "../support/stamped-lines.ts";

const T0 = Date.UTC(2026, 10, 27, 1, 2, 3, 456);

const bytes = (text: string): Buffer => Buffer.from(text, "utf8");

const scriptedClock = (values: number[]): { now: () => number; calls: () => number } => {
  let index = 0;
  return {
    now: () => {
      const value = values[index];
      if (value === undefined) {
        throw new Error(`時計の呼び出しが想定より多くなりました（${index + 1} 回目）。`);
      }
      index += 1;
      return value;
    },
    calls: () => index,
  };
};

const feed = (stamp: (chunk: Uint8Array) => Buffer, chunks: Uint8Array[]): Buffer =>
  Buffer.concat(chunks.map((chunk) => stamp(chunk)));

test("formatStamp は UTC の ISO 8601 をミリ秒まで、末尾 Z で書く", () => {
  assert.equal(formatStamp(T0), "2026-11-27T01:02:03.456Z");
  assert.equal(formatStamp(0), "1970-01-01T00:00:00.000Z");
  assert.equal(formatStamp(T0 + 1), "2026-11-27T01:02:03.457Z");
  assert.match(formatStamp(T0), STAMP_RE);
});

test("1 行の頭に時刻と空白 1 つを付け、中身と改行は変えない", () => {
  const clock = scriptedClock([T0]);
  const out = feed(createLineStamper(clock.now), [bytes("✓ Ready in 512ms\n")]);
  assert.equal(out.toString("utf8"), "2026-11-27T01:02:03.456Z ✓ Ready in 512ms\n");
  assert.equal(clock.calls(), 1);
});

test("1 つの塊に複数行があれば、各行に付け、行ごとに時計を 1 回読む", () => {
  const clock = scriptedClock([T0, T0 + 1, T0 + 2]);
  const stack = "Error: boom\n    at a (/app/x.js:1:1)\n    at b (/app/y.js:2:2)\n";
  const out = feed(createLineStamper(clock.now), [bytes(stack)]);
  assert.equal(
    out.toString("utf8"),
    [
      "2026-11-27T01:02:03.456Z Error: boom\n",
      "2026-11-27T01:02:03.457Z     at a (/app/x.js:1:1)\n",
      "2026-11-27T01:02:03.458Z     at b (/app/y.js:2:2)\n",
    ].join(""),
  );
  assert.equal(clock.calls(), 3);
});

test("空の行にも時刻を付ける", () => {
  const clock = scriptedClock([T0, T0, T0]);
  const out = feed(createLineStamper(clock.now), [bytes("a\n\n\n")]);
  assert.equal(
    out.toString("utf8"),
    "2026-11-27T01:02:03.456Z a\n2026-11-27T01:02:03.456Z \n2026-11-27T01:02:03.456Z \n",
  );
});

test("改行で終わった塊のあとは、次のバイトが来るまで時刻を出さない", () => {
  const clock = scriptedClock([T0, T0 + 1000]);
  const stamp = createLineStamper(clock.now);
  assert.equal(stamp(bytes("first\n")).toString("utf8"), "2026-11-27T01:02:03.456Z first\n");
  assert.equal(clock.calls(), 1, "次の行の時刻は、その行の最初のバイトが来たときに決める");
  assert.equal(stamp(bytes("second\n")).toString("utf8"), "2026-11-27T01:02:04.456Z second\n");
});

test("改行の無い最後の行は、時刻を付けて中身を返し、改行を足さない。続きの塊には時刻を付けない", () => {
  const clock = scriptedClock([T0, T0 + 5]);
  const stamp = createLineStamper(clock.now);
  assert.equal(stamp(bytes("a\nb")).toString("utf8"), "2026-11-27T01:02:03.456Z a\n2026-11-27T01:02:03.461Z b");
  assert.equal(stamp(bytes("cd")).toString("utf8"), "cd", "同じ行の続きには時刻を付けない");
  assert.equal(clock.calls(), 2);
});

test("塊の切れ目で行が割れても、時刻は行の頭に 1 つだけ", () => {
  const clock = scriptedClock([T0, T0 + 1]);
  const out = feed(createLineStamper(clock.now), [bytes("ab"), bytes("c"), bytes("\nd"), bytes("e\n")]);
  assert.equal(out.toString("utf8"), "2026-11-27T01:02:03.456Z abc\n2026-11-27T01:02:03.457Z de\n");
});

test("多バイト文字が塊の途中で割れても、文字の間に時刻を入れない", () => {
  const clock = scriptedClock([T0, T0 + 1]);
  const whole = bytes("日本語のエラー\n🙂次\n");
  const chunks = [whole.subarray(0, 1), whole.subarray(1, 4), whole.subarray(4, 23), whole.subarray(23, 26), whole.subarray(26)];
  const out = feed(createLineStamper(clock.now), chunks);
  assert.equal(out.toString("utf8"), "2026-11-27T01:02:03.456Z 日本語のエラー\n2026-11-27T01:02:03.457Z 🙂次\n");
});

test("空の塊には空を返し、時計を読まない", () => {
  const clock = scriptedClock([T0]);
  const stamp = createLineStamper(clock.now);
  assert.equal(stamp(new Uint8Array(0)).length, 0);
  assert.equal(clock.calls(), 0);
  assert.equal(stamp(bytes("x")).toString("utf8"), "2026-11-27T01:02:03.456Z x");
  assert.equal(stamp(new Uint8Array(0)).length, 0);
  assert.equal(clock.calls(), 1);
});

test("時計が戻っても、戻った時刻をそのまま書く", () => {
  const clock = scriptedClock([T0, T0 - 60_000]);
  const out = feed(createLineStamper(clock.now), [bytes("before\nafter\n")]);
  assert.equal(out.toString("utf8"), "2026-11-27T01:02:03.456Z before\n2026-11-27T01:01:03.456Z after\n");
});

test("\\r、NUL、UTF-8 として正しくないバイト、ANSI の色はそのまま通す", () => {
  const clock = scriptedClock([T0, T0]);
  const input = Buffer.concat([bytes("a\r\n"), Buffer.from([0x00, 0xff, 0xc3, 0x1b]), bytes("[31mred\x1b[0m")]);
  const out = feed(createLineStamper(clock.now), [input]);
  const prefix = bytes("2026-11-27T01:02:03.456Z ");
  assert.deepEqual(out, Buffer.concat([prefix, bytes("a\r\n"), prefix, input.subarray(3)]));
});

test("とても長い行でも、時刻は 1 つだけで、中身を変えない", () => {
  const clock = scriptedClock([T0]);
  const piece = Buffer.alloc(64 * 1024, "x");
  const chunks = Array.from({ length: 64 }, () => piece);
  const out = feed(createLineStamper(clock.now), chunks);
  assert.equal(out.length, STAMP_LEN + 1 + 64 * 64 * 1024);
  assert.equal(out.subarray(0, STAMP_LEN + 1).toString("utf8"), "2026-11-27T01:02:03.456Z ");
  assert.ok(out.subarray(STAMP_LEN + 1).every((b) => b === 0x78));
});

test("2 本の流れは状態を共有しない", () => {
  const clock = scriptedClock([T0, T0 + 1, T0 + 2]);
  const out = createLineStamper(clock.now);
  const err = createLineStamper(clock.now);
  assert.equal(out(bytes("partial")).toString("utf8"), "2026-11-27T01:02:03.456Z partial");
  assert.equal(err(bytes("err\n")).toString("utf8"), "2026-11-27T01:02:03.457Z err\n");
  assert.equal(out(bytes(" rest\n")).toString("utf8"), " rest\n", "標準エラーの行で標準出力の行の頭は変わらない");
  assert.equal(err(bytes("e2")).toString("utf8"), "2026-11-27T01:02:03.458Z e2");
});

test("時計を渡さなければ、いまの時刻を使う", () => {
  const before = Date.now();
  const out = createLineStamper()(bytes("x\n")).toString("utf8");
  const after = Date.now();
  const stamp = out.slice(0, STAMP_LEN);
  assert.match(stamp, STAMP_RE);
  const at = Date.parse(stamp);
  assert.ok(at >= before && at <= after, `${stamp} は ${before}..${after} の間`);
  assert.equal(out.slice(STAMP_LEN), " x\n");
});

// ===== 性質テスト（PBT、除外なし） =====
//
// 任意のバイト列を任意の切れ目で流し、任意の（戻るものを含む）時刻を返す時計を使う。
// 出力を行の頭から読み、時刻と空白 1 つを外したものが入力と一致すること、外した時刻が
// 時計の返した値を呼ばれた順に書いたものであること、時計の呼び出しが行の数と同じことを確かめる。

const mulberry32 = (seed: number): (() => number) => {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
};

const MULTIBYTE = ["é", "日", "🙂", "\u{10FFFF}"].map((c) => bytes(c));

const randomInput = (rand: () => number): Buffer => {
  const parts: Buffer[] = [];
  const length = Math.floor(rand() * 400);
  for (let i = 0; i < length; i += 1) {
    const pick = rand();
    if (pick < 0.15) parts.push(Buffer.from([0x0a]));
    else if (pick < 0.2) parts.push(Buffer.from([0x0d]));
    else if (pick < 0.35) parts.push(MULTIBYTE[Math.floor(rand() * MULTIBYTE.length)]!);
    else parts.push(Buffer.from([Math.floor(rand() * 256)]));
  }
  return Buffer.concat(parts);
};

const randomCuts = (rand: () => number, input: Buffer): Buffer[] => {
  const chunks: Buffer[] = [];
  let at = 0;
  while (at < input.length) {
    // 空の塊も混ぜる。
    const size = rand() < 0.1 ? 0 : 1 + Math.floor(rand() * 40);
    chunks.push(input.subarray(at, Math.min(input.length, at + size)));
    at += size;
  }
  if (rand() < 0.3) chunks.push(Buffer.alloc(0));
  return chunks;
};

const countLines = (input: Buffer): number => {
  let lines = 0;
  for (let i = 0; i < input.length; i += 1) {
    if (i === 0 || input[i - 1] === 0x0a) lines += 1;
  }
  return lines;
};

test("性質: 500 系列の任意のバイト列と切れ目で、時刻を除いた出力は入力と一致し、各行が時計の値で始まる", () => {
  for (let seed = 1; seed <= 500; seed += 1) {
    const where = `seed=${seed}`;
    const rand = mulberry32(0x186 * 1000 + seed);
    const input = randomInput(rand);
    const chunks = randomCuts(rand, input);
    const issued: number[] = [];
    const now = () => {
      // 2000 年から 2100 年の間で、戻るものを含めて選ぶ。
      const value = Date.UTC(2000, 0, 1) + Math.floor(rand() * 100 * 365 * 86_400_000);
      issued.push(value);
      return value;
    };
    const stamp = createLineStamper(now);
    const outputs = chunks.map((chunk) => {
      const out = stamp(chunk);
      if (chunk.length === 0) assert.equal(out.length, 0, `${where}: 空の塊には空を返す`);
      return out;
    });
    const { body, stamps } = unstamp(Buffer.concat(outputs), where);
    assert.deepEqual(body, input, `${where}: 時刻を除いた出力は入力と一致する`);
    assert.equal(stamps.length, countLines(input), `${where}: 中身のある行の数だけ時刻が付く`);
    assert.equal(issued.length, stamps.length, `${where}: 時計は行ごとに 1 回だけ読む`);
    assert.deepEqual(stamps, issued.map(formatStamp), `${where}: 時刻は時計の値を呼ばれた順に書いたもの`);
  }
});

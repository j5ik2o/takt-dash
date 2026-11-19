// Web プロセスの stdout と stderr を別々に時刻付きへ変換する契約（issue #186）。
//
// ラッパーは stdout と stderr の区別を崩さず、各ストリームの行だけを時刻付きへ
// 変換する。あわせて `write` の通常の Node 契約（callback 呼び出し・戻り値・
// encoding 引数）を保ち、終了時には保持中の部分行を flush する。終了時 flush の
// 観測は、実プロセスを終了させず `on` だけを持つテスト用の process へ画面の
// exit イベントを流して確かめる。
import test from "node:test";
import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import { installPrefixOnStream } from "../../scripts/web-log-prefix.mjs";

const STAMP = "2026-11-19T06:31:20.123Z";
const fixedNow = (): Date => new Date(STAMP);

type Captured = { stream: "stdout" | "stderr"; chunk: string };

type WriteCallback = () => void;

// 差し替え対象の write は、呼び出し側が渡す形をそのまま記録し、callback を1回だけ呼ぶ。
const makeStream = (
  stream: Captured["stream"],
  captured: Captured[],
): {
  writes: string[];
  writeArgs: unknown[][];
  target: { write: (...args: unknown[]) => boolean };
} => {
  const writes: string[] = [];
  const writeArgs: unknown[][] = [];
  return {
    writes,
    writeArgs,
    target: {
      write: (...args: unknown[]): boolean => {
        const [chunk, encoding, callback] = args as [string | Uint8Array, string | WriteCallback | undefined, WriteCallback | undefined];
        writeArgs.push([chunk, encoding, callback]);
        const text = typeof chunk === "string" ? chunk : Buffer.from(chunk).toString();
        writes.push(text);
        captured.push({ stream, chunk: text });
        const done = typeof encoding === "function" ? encoding : callback;
        done?.();
        return true;
      },
    },
  };
};

test("stdout の行は stdout の書き込み先へ時刻付きで出し、stderr へ混ぜない", () => {
  const captured: Captured[] = [];
  const stdout = makeStream("stdout", captured);
  const stderr = makeStream("stderr", captured);

  const restoreStdout = installPrefixOnStream(stdout.target, { now: fixedNow });
  const restoreStderr = installPrefixOnStream(stderr.target, { now: fixedNow });
  try {
    stdout.target.write("Ready in 512ms\n");
  } finally {
    restoreStderr();
    restoreStdout();
  }

  assert.deepEqual(stderr.writes, []);
  assert.deepEqual(captured[0], { stream: "stdout", chunk: `${STAMP} Ready in 512ms\n` });
});

test("stderr の行は stderr の書き込み先へ時刻付きで出し、stdout へ混ぜない", () => {
  const captured: Captured[] = [];
  const stdout = makeStream("stdout", captured);
  const stderr = makeStream("stderr", captured);

  const restoreStdout = installPrefixOnStream(stdout.target, { now: fixedNow });
  const restoreStderr = installPrefixOnStream(stderr.target, { now: fixedNow });
  try {
    stderr.target.write("⨯ Error: spin up failed\n");
  } finally {
    restoreStderr();
    restoreStdout();
  }

  assert.deepEqual(stdout.writes, []);
  assert.deepEqual(captured[0], { stream: "stderr", chunk: `${STAMP} ⨯ Error: spin up failed\n` });
});

test("write(str, cb) の callback を1回呼び、元の write の戻り値を返す", () => {
  const stdout = makeStream("stdout", []);

  const restore = installPrefixOnStream(stdout.target, { now: fixedNow });
  try {
    let calls = 0;
    const result = stdout.target.write("line\n", () => {
      calls += 1;
    });

    assert.equal(calls, 1);
    assert.equal(result, true);
  } finally {
    restore();
  }
});

test("write(chunk, encoding, cb) の第2引数 encoding を落とさず callback も渡す", () => {
  const stdout = makeStream("stdout", []);

  const restore = installPrefixOnStream(stdout.target, { now: fixedNow });
  try {
    let calls = 0;
    stdout.target.write("line\n", "utf8", () => {
      calls += 1;
    });

    assert.equal(calls, 1);
    assert.equal(stdout.writeArgs[0]?.[1], "utf8");
  } finally {
    restore();
  }
});

test("install したストリームは終了時に保持中の部分行を flush する", () => {
  const stdout = makeStream("stdout", []);
  const processStub = new EventEmitter();

  const restore = installPrefixOnStream(stdout.target, { now: fixedNow, process: processStub });
  try {
    stdout.target.write("last partial");
    assert.deepEqual(stdout.writes, []);

    processStub.emit("exit", 0);
  } finally {
    restore();
  }

  assert.deepEqual(stdout.writes, [`${STAMP} last partial`]);
});

test("改行を含まない write(str, cb) の callback は行が揃うまで待ち、改行到達時に1回呼ぶ", () => {
  const stdout = makeStream("stdout", []);

  const restore = installPrefixOnStream(stdout.target, { now: fixedNow });
  try {
    let calls = 0;
    stdout.target.write("tail", () => {
      calls += 1;
    });

    assert.equal(calls, 0, "部分行を保持している間は callback を呼ばない");
    assert.deepEqual(stdout.writes, []);

    stdout.target.write("\n");

    assert.equal(calls, 1, "行が揃った時点で保持した callback を1回呼ぶ");
    assert.deepEqual(stdout.writes, [`${STAMP} tail\n`]);
  } finally {
    restore();
  }
});

test("改行を含まない write(str, cb) の callback は終了時 flush でも1回呼ぶ", () => {
  const stdout = makeStream("stdout", []);
  const processStub = new EventEmitter();

  const restore = installPrefixOnStream(stdout.target, { now: fixedNow, process: processStub });
  try {
    let calls = 0;
    stdout.target.write("tail", () => {
      calls += 1;
    });
    assert.equal(calls, 0);

    processStub.emit("exit", 0);

    assert.equal(calls, 1, "終了時 flush で保持した callback を1回呼ぶ");
    assert.deepEqual(stdout.writes, [`${STAMP} tail`]);
  } finally {
    restore();
  }
});

test("改行を含まない write(chunk, encoding, cb) は encoding を保持し callback を1回呼ぶ", () => {
  const stdout = makeStream("stdout", []);

  const restore = installPrefixOnStream(stdout.target, { now: fixedNow });
  try {
    let calls = 0;
    stdout.target.write("x", "utf8", () => {
      calls += 1;
    });
    assert.equal(calls, 0);

    stdout.target.write("y\n");

    assert.equal(calls, 1);
    assert.deepEqual(stdout.writes, [`${STAMP} xy\n`]);
  } finally {
    restore();
  }
});

test("部分行 write の callback は、その行を確定させた改行 write の callback より先に1回呼ぶ", () => {
  const stdout = makeStream("stdout", []);

  const restore = installPrefixOnStream(stdout.target, { now: fixedNow });
  try {
    const order: string[] = [];
    stdout.target.write("tail", () => {
      order.push("cbA");
    });
    stdout.target.write("\n", () => {
      order.push("cbB");
    });

    assert.deepEqual(order, ["cbA", "cbB"]);
    assert.deepEqual(stdout.writes, [`${STAMP} tail\n`]);
  } finally {
    restore();
  }
});

test("同一行へ連結する write の callback も、行の書き出し完了後に write 順で1回ずつ呼ぶ", () => {
  const stdout = makeStream("stdout", []);

  const restore = installPrefixOnStream(stdout.target, { now: fixedNow });
  try {
    const order: string[] = [];
    const calls = { a: 0, b: 0 };
    stdout.target.write("tail", () => {
      calls.a += 1;
      order.push("cbA");
    });
    stdout.target.write("tail2\n", () => {
      calls.b += 1;
      order.push("cbB");
    });

    assert.deepEqual(order, ["cbA", "cbB"]);
    assert.deepEqual(calls, { a: 1, b: 1 });
    assert.deepEqual(stdout.writes, [`${STAMP} tailtail2\n`]);
  } finally {
    restore();
  }
});

test("終了時 flush でも保留した callback を write 順で1回ずつ呼ぶ", () => {
  const stdout = makeStream("stdout", []);
  const processStub = new EventEmitter();

  const restore = installPrefixOnStream(stdout.target, { now: fixedNow, process: processStub });
  try {
    const order: string[] = [];
    stdout.target.write("tail", () => {
      order.push("cbA");
    });
    stdout.target.write("tail2", () => {
      order.push("cbB");
    });
    assert.deepEqual(order, []);

    processStub.emit("exit", 0);

    assert.deepEqual(order, ["cbA", "cbB"]);
    assert.deepEqual(stdout.writes, [`${STAMP} tailtail2`]);
  } finally {
    restore();
  }
});

test("restore の後は生の行をそのまま流し、時刻を付けない", () => {
  const stdout = makeStream("stdout", []);

  const restore = installPrefixOnStream(stdout.target, { now: fixedNow });
  restore();

  stdout.target.write("raw line\n");

  assert.deepEqual(stdout.writes, ["raw line\n"]);
});

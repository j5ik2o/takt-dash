// Web の起動を包むラッパー（issue #186、docs/design/issue-186.md）。
//
// `node scripts/with-timestamps.mjs <program> [args...]` は子を起こし、子の標準出力と
// 標準エラーの各行に時刻を付けて、それぞれ自分の標準出力と標準エラーへ書く。受けた合図は
// 子へ送り、子の終わり方（終了コードか合図）をそのまま自分の終わり方にする。
import test from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { STAMP_LEN, unstamp } from "../support/stamped-lines.ts";

const WRAPPER = fileURLToPath(new URL("../../scripts/with-timestamps.mjs", import.meta.url));

type Run = {
  stdout: () => Buffer;
  stderr: () => Buffer;
  closed: Promise<{ code: number | null; signal: NodeJS.Signals | null }>;
  waitForStdout: (text: string, timeoutMs: number) => Promise<void>;
  kill: (signal: NodeJS.Signals) => void;
};

const runWrapper = (args: string[]): Run => {
  const child = spawn(process.execPath, [WRAPPER, ...args], { stdio: ["ignore", "pipe", "pipe"] });
  const out: Buffer[] = [];
  const err: Buffer[] = [];
  const waiters: (() => void)[] = [];
  child.stdout.on("data", (chunk: Buffer) => {
    out.push(chunk);
    waiters.splice(0).forEach((wake) => wake());
  });
  child.stderr.on("data", (chunk: Buffer) => err.push(chunk));
  const closed = new Promise<{ code: number | null; signal: NodeJS.Signals | null }>((resolve) => {
    child.on("close", (code, signal) => {
      resolve({ code, signal });
      waiters.splice(0).forEach((wake) => wake());
    });
  });
  let done = false;
  void closed.then(() => {
    done = true;
  });
  const stdout = () => Buffer.concat(out);
  const stderr = () => Buffer.concat(err);
  const waitForStdout = async (text: string, timeoutMs: number) => {
    const deadline = Date.now() + timeoutMs;
    while (!stdout().toString("utf8").includes(text)) {
      if (done) {
        assert.fail(`標準出力に ${JSON.stringify(text)} が出る前にラッパーが終わりました。stderr: ${stderr().toString("utf8")}`);
      }
      if (Date.now() > deadline) {
        child.kill("SIGKILL");
        assert.fail(`${timeoutMs}ms 待っても標準出力に ${JSON.stringify(text)} が出ません。`);
      }
      await new Promise<void>((resolve) => {
        const timer = setTimeout(resolve, 50);
        waiters.push(() => {
          clearTimeout(timer);
          resolve();
        });
      });
    }
  };
  return { stdout, stderr, closed, waitForStdout, kill: (signal) => child.kill(signal) };
};

// 子として起こす node の小さなプログラム。テストが固まらないよう、どれも 5 秒で自分から終わる。
const nodeChild = (source: string): string[] => [process.execPath, "-e", `setTimeout(() => process.exit(99), 5000);\n${source}`];

const lines = (buffer: Buffer): string[] => buffer.toString("utf8").split("\n").filter((line) => line !== "");

test("子の標準出力と標準エラーの各行に時刻を付け、混ぜずに書く", async () => {
  const before = Date.now();
  const run = runWrapper(
    nodeChild(`
      process.stdout.write("▲ Next.js 16\\n✓ Ready in 1s\\n");
      process.stderr.write("Error: 失敗\\n    at x (/app/a.js:1:1)\\n", () => process.exit(0));
    `),
  );
  const { code, signal } = await run.closed;
  const after = Date.now();
  assert.equal(code, 0, `stderr: ${run.stderr().toString("utf8")}`);
  assert.equal(signal, null);
  const out = unstamp(run.stdout(), "stdout");
  const err = unstamp(run.stderr(), "stderr");
  assert.equal(out.body.toString("utf8"), "▲ Next.js 16\n✓ Ready in 1s\n");
  assert.equal(err.body.toString("utf8"), "Error: 失敗\n    at x (/app/a.js:1:1)\n", "スタックの各行に時刻を付ける");
  assert.equal(out.stamps.length, 2);
  assert.equal(err.stamps.length, 2);
  for (const stamp of [...out.stamps, ...err.stamps]) {
    const at = Date.parse(stamp);
    assert.ok(at >= before && at <= after, `${stamp} はテストの前後（${before}..${after}）の間`);
  }
  assert.ok(lines(run.stdout()).every((line) => !line.includes("Error")), "標準エラーの行は標準出力に出ない");
});

test("改行の無い最後の行は、時刻を付けて中身を書き、改行を足さない", async () => {
  const run = runWrapper(nodeChild(`process.stdout.write("a\\nlast", () => process.exit(0));`));
  const { code } = await run.closed;
  assert.equal(code, 0);
  const out = run.stdout();
  assert.equal(unstamp(out, "stdout").body.toString("utf8"), "a\nlast");
  assert.notEqual(out[out.length - 1], 0x0a);
});

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

const randomStream = (rand: () => number): { data: Buffer; cuts: number[] } => {
  const pieces: Buffer[] = [];
  const count = 50 + Math.floor(rand() * 200);
  for (let i = 0; i < count; i += 1) {
    const pick = rand();
    if (pick < 0.2) pieces.push(Buffer.from("\n"));
    else if (pick < 0.35) pieces.push(Buffer.from(["é", "日本", "🙂"][Math.floor(rand() * 3)]!, "utf8"));
    else if (pick < 0.37) pieces.push(Buffer.alloc(256 * 1024 + Math.floor(rand() * 1024), "L"));
    else pieces.push(Buffer.from(Array.from({ length: 1 + Math.floor(rand() * 30) }, () => Math.floor(rand() * 256))));
  }
  const data = Buffer.concat(pieces);
  const cuts: number[] = [];
  for (let at = 0; at < data.length; ) {
    const size = 1 + Math.floor(rand() * 5000);
    cuts.push(size);
    at += size;
  }
  return { data, cuts };
};

test("性質: 子の任意のバイト列を任意の切れ目で流しても、時刻を除いて標準出力と標準エラーのそれぞれで一致する", async () => {
  const dir = await mkdtemp(join(tmpdir(), "with-timestamps-"));
  try {
    for (let seed = 1; seed <= 4; seed += 1) {
      const where = `seed=${seed}`;
      const rand = mulberry32(0x186 * 7919 + seed);
      const outStream = randomStream(rand);
      const errStream = randomStream(rand);
      const files = {
        out: join(dir, `${seed}.out`),
        err: join(dir, `${seed}.err`),
        cuts: join(dir, `${seed}.json`),
      };
      await writeFile(files.out, outStream.data);
      await writeFile(files.err, errStream.data);
      await writeFile(files.cuts, JSON.stringify({ out: outStream.cuts, err: errStream.cuts }));
      // 子は 2 本の流れを、決めた切れ目で、書き終わりを待ちながら交互に書く。
      const run = runWrapper(
        nodeChild(`
          const fs = require("node:fs");
          const cuts = JSON.parse(fs.readFileSync(${JSON.stringify(files.cuts)}, "utf8"));
          const streams = [
            { data: fs.readFileSync(${JSON.stringify(files.out)}), cuts: cuts.out, at: 0, sink: process.stdout },
            { data: fs.readFileSync(${JSON.stringify(files.err)}), cuts: cuts.err, at: 0, sink: process.stderr },
          ];
          const write = (s, chunk) => new Promise((resolve) => s.sink.write(chunk, resolve));
          (async () => {
            for (let i = 0; streams.some((s) => s.at < s.data.length); i += 1) {
              for (const s of streams) {
                if (i >= s.cuts.length) continue;
                const chunk = s.data.subarray(s.at, s.at + s.cuts[i]);
                s.at += s.cuts[i];
                await write(s, chunk);
              }
            }
            process.exit(0);
          })();
        `),
      );
      const { code } = await run.closed;
      assert.equal(code, 0, `${where}: stderr の先頭 ${run.stderr().subarray(0, 200).toString("utf8")}`);
      const out = unstamp(run.stdout(), `${where} stdout`);
      const err = unstamp(run.stderr(), `${where} stderr`);
      assert.ok(out.body.equals(outStream.data), `${where}: 標準出力は時刻を除いて一致する`);
      assert.ok(err.body.equals(errStream.data), `${where}: 標準エラーは時刻を除いて一致する`);
    }
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("子の終了コード 0 と 0 以外を引き継ぐ", async () => {
  for (const exitCode of [0, 1, 7, 255]) {
    const run = runWrapper(nodeChild(`process.stdout.write("bye\\n", () => process.exit(${exitCode}));`));
    const { code, signal } = await run.closed;
    assert.equal(code, exitCode, `子の終了コード ${exitCode}。stderr: ${run.stderr().toString("utf8")}`);
    assert.equal(signal, null);
    assert.equal(unstamp(run.stdout(), `exit=${exitCode}`).body.toString("utf8"), "bye\n", "最後の行を落とさない");
  }
});

test("SIGTERM を受けると子へ送り、子の終了コードで終わる", async () => {
  const run = runWrapper(
    nodeChild(`
      process.on("SIGTERM", () => process.stdout.write("got SIGTERM\\n", () => process.exit(43)));
      process.stdout.write("ready\\n");
      setInterval(() => {}, 1000);
    `),
  );
  await run.waitForStdout("ready\n", 4000);
  run.kill("SIGTERM");
  const { code, signal } = await run.closed;
  assert.equal(code, 43, `stderr: ${run.stderr().toString("utf8")}`);
  assert.equal(signal, null);
  assert.equal(unstamp(run.stdout(), "stdout").body.toString("utf8"), "ready\ngot SIGTERM\n");
});

const isAlive = (pid: number): boolean => {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
};

test("SIGTERM を子へ送り、子が合図で終われば、ラッパーも SIGTERM で終わる。子を残さない", async () => {
  const run = runWrapper(nodeChild(`process.stdout.write("pid=" + process.pid + "\\nready\\n"); setInterval(() => {}, 1000);`));
  await run.waitForStdout("ready\n", 4000);
  const pid = Number(/pid=(\d+)/.exec(run.stdout().toString("utf8"))?.[1]);
  assert.ok(Number.isInteger(pid) && pid > 0, "子の pid を読めた");
  run.kill("SIGTERM");
  const { code, signal } = await run.closed;
  const orphaned = isAlive(pid);
  if (orphaned) process.kill(pid, "SIGKILL");
  assert.equal(orphaned, false, "ラッパーが終わった時点で、子は終わっている（子の終わりを待ってから終わる）");
  assert.equal(signal, "SIGTERM", `code=${code} stderr: ${run.stderr().toString("utf8")}`);
  assert.equal(code, null);
});

test("SIGINT と SIGHUP も子へ送る", async () => {
  for (const sent of ["SIGINT", "SIGHUP"] as const) {
    const run = runWrapper(
      nodeChild(`
        process.on(${JSON.stringify(sent)}, () => process.stdout.write("got ${sent}\\n", () => process.exit(44)));
        process.stdout.write("ready\\n");
        setInterval(() => {}, 1000);
      `),
    );
    await run.waitForStdout("ready\n", 4000);
    run.kill(sent);
    const { code } = await run.closed;
    assert.equal(code, 44, `${sent}: stderr: ${run.stderr().toString("utf8")}`);
    assert.equal(unstamp(run.stdout(), sent).body.toString("utf8"), `ready\ngot ${sent}\n`);
  }
});

test("子が自分で合図を受けて終わると、ラッパーも同じ合図で終わり、それまでの行を落とさない", async () => {
  const run = runWrapper(nodeChild(`process.stdout.write("last words\\n", () => process.kill(process.pid, "SIGKILL"));`));
  const { code, signal } = await run.closed;
  assert.equal(signal, "SIGKILL", `code=${code} stderr: ${run.stderr().toString("utf8")}`);
  assert.equal(unstamp(run.stdout(), "stdout").body.toString("utf8"), "last words\n");
});

test("子を起こせなければ、時刻を付けた行を標準エラーに書いて 127 で終わる", async () => {
  const run = runWrapper(["/nonexistent/issue-186/program"]);
  const { code, signal } = await run.closed;
  assert.equal(code, 127, `stderr: ${run.stderr().toString("utf8")}`);
  assert.equal(signal, null);
  assert.equal(run.stdout().length, 0);
  const err = unstamp(run.stderr(), "stderr");
  assert.ok(err.stamps.length >= 1);
  assert.ok(err.body.toString("utf8").includes("/nonexistent/issue-186/program"), "起こせなかったプログラムを名指す");
});

test("起こすプログラムが無ければ、時刻を付けた使い方を標準エラーに書いて 2 で終わる", async () => {
  const run = runWrapper([]);
  const { code } = await run.closed;
  assert.equal(code, 2, `stderr: ${run.stderr().toString("utf8")}`);
  assert.equal(run.stdout().length, 0);
  const err = run.stderr();
  assert.ok(err.length > STAMP_LEN);
  unstamp(err, "stderr");
});

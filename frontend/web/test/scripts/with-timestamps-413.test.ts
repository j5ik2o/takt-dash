// with-timestamps.mjs の点検の指摘の直し（issue #413、docs/design/issue-186.md「#413 の直し」）。
//
// 子が合図で終わったときに 0 で終わらないこと、自分へ SIGUSR1 を送らないこと、引数なしは常に 2、
// 起こせないときは時刻付きの 1 行と 127、常駐中の書き込みの失敗はすぐ 1 回だけ知らせること、を確かめる。
import test from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { constants, tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { unstamp } from "../support/stamped-lines.ts";

const WRAPPER = fileURLToPath(new URL("../../scripts/with-timestamps.mjs", import.meta.url));

const CLOSE_TIMEOUT_MS = 15000;

type Ended = { code: number | null; signal: NodeJS.Signals | null };

type Run = {
  stdout: () => Buffer;
  stderr: () => Buffer;
  closed: Promise<Ended>;
  isClosed: () => boolean;
  waitFor: (stream: "stdout" | "stderr", text: string, timeoutMs: number) => Promise<void>;
  writeStdin: (line: string) => void;
  kill: (signal: NodeJS.Signals) => void;
};

type RunOptions = {
  // ラッパーの前に node へ渡す引数（--import など）。
  nodeArgs?: string[];
  // 起こした直後に、テストの側で読み口を閉じる流れ。ラッパーから見ると閉じたパイプになる。
  closeStdout?: boolean;
  closeStderr?: boolean;
  // 子の標準入力へ合図を書けるようにする（ラッパーは子の標準入力をそのまま引き継ぐ）。
  stdin?: boolean;
};

const runWrapper = (args: string[], options: RunOptions = {}): Run => {
  const child = spawn(process.execPath, [...(options.nodeArgs ?? []), WRAPPER, ...args], {
    stdio: [options.stdin ? "pipe" : "ignore", "pipe", "pipe"],
  });
  const out: Buffer[] = [];
  const err: Buffer[] = [];
  const waiters: (() => void)[] = [];
  const wake = () => waiters.splice(0).forEach((resolve) => resolve());
  if (options.closeStdout) child.stdout.destroy();
  else
    child.stdout.on("data", (chunk: Buffer) => {
      out.push(chunk);
      wake();
    });
  if (options.closeStderr) child.stderr.destroy();
  else
    child.stderr.on("data", (chunk: Buffer) => {
      err.push(chunk);
      wake();
    });
  let done = false;
  // ラッパーの終わりの待ちの上限。超えたら SIGKILL して失敗にする（テストを固めない）。
  const closed = new Promise<Ended>((resolve, reject) => {
    const limit = setTimeout(() => {
      child.kill("SIGKILL");
      reject(new Error(`${CLOSE_TIMEOUT_MS}ms 待ってもラッパーが終わりません。args: ${JSON.stringify(args)}`));
    }, CLOSE_TIMEOUT_MS);
    child.on("close", (code, signal) => {
      clearTimeout(limit);
      done = true;
      resolve({ code, signal });
      wake();
    });
  });
  const stdout = () => Buffer.concat(out);
  const stderr = () => Buffer.concat(err);
  const waitFor = async (stream: "stdout" | "stderr", text: string, timeoutMs: number) => {
    const read = stream === "stdout" ? stdout : stderr;
    const deadline = Date.now() + timeoutMs;
    while (!read().toString("utf8").includes(text)) {
      const seen = `stdout: ${stdout().toString("utf8")} stderr: ${stderr().toString("utf8")}`;
      if (done) {
        assert.fail(`${stream} に ${JSON.stringify(text)} が出る前にラッパーが終わりました。${seen}`);
      }
      if (Date.now() > deadline) {
        child.kill("SIGKILL");
        assert.fail(`${timeoutMs}ms 待っても ${stream} に ${JSON.stringify(text)} が出ません。${seen}`);
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
  const writeStdin = (line: string) => {
    assert.ok(options.stdin && child.stdin, "標準入力へ書くには options.stdin が要ります");
    child.stdin.write(line);
  };
  return {
    stdout,
    stderr,
    closed,
    isClosed: () => done,
    waitFor,
    writeStdin,
    kill: (signal) => child.kill(signal),
  };
};

// 子が自分へ合図 S を送って終わる。node の子だと SIGPIPE・SIGXFSZ は無視され、SIGUSR1 は
// Inspector を起こすので、合図の既定の動作で死ぬ sh を使う。
const shKillsItself = (signal: NodeJS.Signals): string[] => ["/bin/sh", "-c", `kill -s ${signal.slice(3)} $$`];

const signalNumber = (signal: NodeJS.Signals): number => {
  const n = constants.signals[signal];
  assert.ok(typeof n === "number" && n > 0, `${signal} の番号`);
  return n;
};

// 自分へ送ってよい合図（既定の動作が core を作らずに終わり、Node が無視も別の用途にも使わないもの）。
// これらで子が終わったら、ラッパーは同じ合図か 128 + 番号で終わる。それ以外は 128 + 番号で終わり、合図では終わらない。
const SELF_SENDABLE: readonly NodeJS.Signals[] = ["SIGHUP", "SIGINT", "SIGTERM", "SIGKILL", "SIGALRM", "SIGUSR2", "SIGVTALRM", "SIGPROF"];

const assertEndedLikeSignal = (ended: Ended, signal: NodeJS.Signals, where: string, stderr: string) => {
  const exitCode = 128 + signalNumber(signal);
  const sendable = SELF_SENDABLE.includes(signal);
  const ok = (sendable && ended.signal === signal) || (ended.signal === null && ended.code === exitCode);
  assert.ok(
    ok,
    `${where}: 子が ${signal} で終わったら、ラッパーは ${sendable ? `${signal} か ` : ""}${exitCode} で終わる。` +
      `実際は code=${ended.code} signal=${ended.signal}。stderr: ${stderr}`,
  );
  assert.notEqual(ended.code, 0, `${where}: 0 では終わらない`);
};

const isAlive = (pid: number): boolean => {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
};

const readPid = (output: Buffer): number => {
  const pid = Number(/pid=(\d+)/.exec(output.toString("utf8"))?.[1]);
  assert.ok(Number.isInteger(pid) && pid > 0, "子の pid を読めた");
  return pid;
};

const noticeLines = (body: Buffer): string[] =>
  body
    .toString("utf8")
    .split("\n")
    .filter((line) => line.includes("with-timestamps:"));

const withTempDir = async (body: (dir: string) => Promise<void>) => {
  const dir = await mkdtemp(join(tmpdir(), "with-timestamps-413-"));
  try {
    await body(dir);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
};

// 親（ラッパーの node）に SIGHUP の受け手を、ラッパーより先に足す。親が SIGHUP を無視している状態を作る。
const writeSighupListener = async (dir: string): Promise<string> => {
  const path = join(dir, "ignore-sighup.mjs");
  await writeFile(path, `process.on("SIGHUP", () => {});\n`);
  return path;
};

test("子が SIGPIPE で終わると、ラッパーは 0 で終わらず、128 + 13 で終わる", async () => {
  const run = runWrapper(shKillsItself("SIGPIPE"));
  const ended = await run.closed;
  assertEndedLikeSignal(ended, "SIGPIPE", "SIGPIPE", run.stderr().toString("utf8"));
});

test("子が SIGUSR1 で終わると、ラッパーは Inspector を起こさず、128 + 番号で終わる", async () => {
  const run = runWrapper(shKillsItself("SIGUSR1"));
  const ended = await run.closed;
  const stderr = run.stderr().toString("utf8");
  assert.ok(!stderr.includes("Debugger listening"), `自分へ SIGUSR1 を送らない。stderr: ${stderr}`);
  assertEndedLikeSignal(ended, "SIGUSR1", "SIGUSR1", stderr);
});

test("親が SIGHUP を無視している状態で子が SIGHUP で終わると、ラッパーは 0 で終わらない", async () => {
  await withTempDir(async (dir) => {
    const listener = await writeSighupListener(dir);
    const run = runWrapper(shKillsItself("SIGHUP"), { nodeArgs: ["--import", listener] });
    const ended = await run.closed;
    assertEndedLikeSignal(ended, "SIGHUP", "SIGHUP の受け手あり", run.stderr().toString("utf8"));
  });
});

test("性質（総当たり）: どの合図で子が終わっても、親の SIGHUP の受け手の有無によらず、送ってよい合図は合図か 128 + 番号、それ以外は 128 + 番号で終わり、Inspector を起こさない", async () => {
  const signals: NodeJS.Signals[] = ["SIGHUP", "SIGINT", "SIGTERM", "SIGKILL", "SIGALRM", "SIGUSR1", "SIGUSR2", "SIGPIPE", "SIGXFSZ"];
  // core とクラッシュレポートを作る合図。macOS では子（sh）のクラッシュレポートが手元に溜まるので、CI（Linux）で確かめる。
  // 足すのは子を終わらせる合図だけ（SIGSTOP などの止める合図、SIGCHLD などの無視される合図は子が終わらない）。
  if (process.platform !== "darwin") signals.push("SIGABRT", "SIGQUIT", "SIGSEGV", "SIGTRAP", "SIGBUS", "SIGILL", "SIGFPE", "SIGSYS");
  await withTempDir(async (dir) => {
    const listener = await writeSighupListener(dir);
    for (const withListener of [false, true]) {
      for (const signal of signals) {
        const where = `${signal}（SIGHUP の受け手${withListener ? "あり" : "なし"}）`;
        const run = runWrapper(shKillsItself(signal), withListener ? { nodeArgs: ["--import", listener] } : {});
        const ended = await run.closed;
        const stderr = run.stderr().toString("utf8");
        assert.ok(!stderr.includes("Debugger listening"), `${where}: Inspector を起こさない。stderr: ${stderr}`);
        assertEndedLikeSignal(ended, signal, where, stderr);
      }
    }
  });
});

test("引数なしで標準エラーが閉じたパイプでも、2 で終わる", async () => {
  const run = runWrapper([], { closeStderr: true });
  const ended = await run.closed;
  assert.equal(ended.signal, null);
  assert.equal(ended.code, 2);
  assert.equal(run.stdout().length, 0);
});

test("プログラムが空の文字列なら、時刻付きの行を標準エラーに書いて 127 で終わる", async () => {
  const run = runWrapper([""]);
  const ended = await run.closed;
  const stderr = run.stderr();
  assert.equal(ended.signal, null);
  assert.equal(ended.code, 127, `stderr: ${stderr.toString("utf8")}`);
  assert.equal(run.stdout().length, 0);
  assert.ok(stderr.length > 0, "起こせなかったことを書く");
  // 全部の行が時刻で始まる（捕まえていない例外のスタックを時刻なしで出さない）。
  const err = unstamp(stderr, "stderr");
  assert.ok(err.stamps.length >= 1);
});

test("実行の権限の無いファイルなら、時刻付きの行を標準エラーに書いて 127 で終わる", async () => {
  await withTempDir(async (dir) => {
    const path = join(dir, "not-executable.sh");
    await writeFile(path, "#!/bin/sh\necho hi\n", { mode: 0o644 });
    const run = runWrapper([path]);
    const ended = await run.closed;
    const stderr = run.stderr();
    assert.equal(ended.signal, null);
    assert.equal(ended.code, 127, `stderr: ${stderr.toString("utf8")}`);
    assert.equal(run.stdout().length, 0);
    const err = unstamp(stderr, "stderr");
    assert.ok(err.body.toString("utf8").includes(path), "起こせなかったプログラムを名指す");
  });
});

// 子は pid を「知らせを受ける側」の流れへ書き、そのあと「失敗する側」の流れへ書き続ける。
// 失敗する側へ 30 回書いても印の行 "after 30" は書かず、テストが標準入力へ 1 行書く合図を
// 待ってから書く（知らせを見てから合図する順を、印の位置で確かめられるようにする）。
// nodeChild と同じく、固まらないよう 10 秒で自分から終わる。
const keepsWriting = (failing: "stdout" | "stderr"): string[] => {
  const other = failing === "stdout" ? "stderr" : "stdout";
  return [
    process.execPath,
    "-e",
    `
      setTimeout(() => process.exit(99), 10000);
      process.${other}.write("pid=" + process.pid + "\\n");
      let i = 0;
      let signaled = false;
      let wroteMarker = false;
      const writeMarker = () => {
        if (!signaled || i < 30 || wroteMarker) return;
        wroteMarker = true;
        process.${other}.write("after 30\\n");
      };
      process.stdin.setEncoding("utf8");
      process.stdin.on("data", () => {
        signaled = true;
        writeMarker();
      });
      setInterval(() => {
        process.${failing}.write("line " + (i += 1) + "\\n");
        writeMarker();
      }, 20);
    `,
  ];
};

for (const failing of ["stdout", "stderr"] as const) {
  const other = failing === "stdout" ? "stderr" : "stdout";
  test(`常駐中に ${failing} の書き込みが失敗すると、子が動いている間に、${other} へ時刻付きの知らせを 1 行だけ書く`, async () => {
    const run = runWrapper(keepsWriting(failing), {
      ...(failing === "stdout" ? { closeStdout: true } : { closeStderr: true }),
      stdin: true,
    });
    await run.waitFor(other, "with-timestamps:", 5000);
    // 知らせを見てから合図する。子は合図まで印を書かず、合図のあとも失敗する側へ書き続ける。
    // 2 回目以降の失敗で知らせが増えないことを、印の行まで待って見る。
    run.writeStdin("go\n");
    await run.waitFor(other, "after 30\n", 5000);
    const read = other === "stdout" ? run.stdout : run.stderr;
    const pid = readPid(read());
    const alive = isAlive(pid);
    const wrapperRunning = !run.isClosed();
    const snapshot = read();
    run.kill("SIGTERM");
    await run.closed;
    if (isAlive(pid)) process.kill(pid, "SIGKILL");
    assert.ok(alive, "知らせは子が終わる前に出る");
    assert.ok(wrapperRunning, "知らせは子が終わる前に出る（ラッパーはまだ動いている）");
    const notices = noticeLines(unstamp(snapshot, other).body);
    assert.equal(notices.length, 1, `知らせは 1 行だけ。${other}: ${snapshot.toString("utf8")}`);
    assert.ok(notices[0]!.includes(failing), `知らせは失敗した流れ（${failing}）を名指す: ${notices[0]}`);
    // 子は知らせを見たあとの合図で印を書く。バイトの位置でも、印が知らせの行の終端より後ろにあることを確かめる。
    const noticeStart = snapshot.indexOf("with-timestamps:");
    assert.ok(noticeStart >= 0, `知らせがある。${other}: ${snapshot.toString("utf8")}`);
    const noticeLineEnd = snapshot.indexOf(0x0a, noticeStart);
    assert.ok(noticeLineEnd >= 0, `知らせの行は改行で終わる。${other}: ${snapshot.toString("utf8")}`);
    const markerStart = snapshot.indexOf("after 30\n");
    assert.ok(
      markerStart > noticeLineEnd,
      `印は知らせの行の終端より後ろにある。${other}: ${snapshot.toString("utf8")}`,
    );
  });
}

test("標準エラーの改行の無い行の途中で標準出力が失敗しても、知らせは独立した行になり、子の行の続きにも時刻が付く", async () => {
  // 子は標準エラーへ改行なしで書き、少し待ってから標準出力へ書き（ここで失敗する）、さらに待ってから標準エラーの行を終える。
  const childSource = `
    setTimeout(() => process.exit(99), 10000);
    process.stderr.write("pid=" + process.pid + "\\npartial");
    setTimeout(() => {
      const timer = setInterval(() => process.stdout.write("line\\n"), 20);
      setTimeout(() => {
        clearInterval(timer);
        process.stderr.write(" rest\\n", () => process.exit(0));
      }, 1000);
    }, 300);
  `;
  const run = runWrapper([process.execPath, "-e", childSource], { closeStdout: true });
  await run.waitFor("stderr", "with-timestamps:", 5000);
  const ended = await run.closed;
  const stderr = run.stderr();
  assert.equal(ended.code, 0, `stderr: ${stderr.toString("utf8")}`);
  // 全部の行が時刻で始まる（知らせのあとの子の行の続きにも時刻が付く）。
  const lines = unstamp(stderr, "stderr").body.toString("utf8").split("\n");
  assert.equal(lines.filter((line) => line.includes("with-timestamps:")).length, 1, `stderr: ${stderr.toString("utf8")}`);
  const noticeAt = lines.findIndex((line) => line.includes("with-timestamps:"));
  assert.ok(lines[noticeAt]!.startsWith("with-timestamps:"), `知らせは行の頭から始まる: ${JSON.stringify(lines[noticeAt])}`);
  assert.equal(lines[noticeAt - 1], "partial", "知らせの前に、子の行の途中までを独立した行として書く");
  assert.equal(lines[noticeAt + 1], " rest", "子の行の続きは、知らせのあとの別の行になる");
});

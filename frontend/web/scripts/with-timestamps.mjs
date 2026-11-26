// Web の起動を包むラッパー（issue #186、docs/design/issue-186.md）。
//
// node scripts/with-timestamps.mjs <program> [args...]
// 子をシェルを通さずに起こし、子の標準出力と標準エラーの各行に時刻を付けて、
// それぞれ自分の標準出力と標準エラーへ書く。受けた合図は子へ送り、
// 子の終わり方（終了コードか合図）をそのまま自分の終わり方にする。
import { spawn } from "node:child_process";
import { createLineStamper } from "./line-stamper.mjs";

const FORWARDED_SIGNALS = ["SIGTERM", "SIGINT", "SIGHUP"];

function writeStampedLine(sink, text) {
  const stamper = createLineStamper();
  return new Promise((resolve) => {
    sink.write(stamper(Buffer.from(text, "utf8")), () => resolve());
  });
}

function forward(source, sink) {
  const stamper = createLineStamper();
  const pending = [];
  return new Promise((resolve, reject) => {
    source.on("data", (chunk) => {
      const out = stamper(chunk);
      if (out.length === 0) {
        return;
      }
      let callbackResolve;
      const written = new Promise((res) => {
        callbackResolve = res;
      });
      pending.push(written);
      const ok = sink.write(out, () => callbackResolve());
      if (!ok) {
        source.pause();
        sink.once("drain", () => {
          source.resume();
        });
      }
    });
    source.on("end", () => {
      Promise.all(pending).then(resolve, reject);
    });
    source.on("error", reject);
  });
}

async function main() {
  const [program, ...args] = process.argv.slice(2);
  if (program === undefined) {
    await writeStampedLine(
      process.stderr,
      "usage: node scripts/with-timestamps.mjs <program> [args...]\n",
    );
    process.exitCode = 2;
    return;
  }

  const child = spawn(program, args, { stdio: ["inherit", "pipe", "pipe"] });

  const handlers = new Map();
  for (const signal of FORWARDED_SIGNALS) {
    const handler = () => {
      try {
        child.kill(signal);
      } catch {
        // 子が既に終わっている場合は何もしない。
      }
    };
    handlers.set(signal, handler);
    process.on(signal, handler);
  }

  let spawnError = null;
  const errorPromise = new Promise((resolve) => {
    child.on("error", (error) => {
      spawnError = error;
      resolve();
    });
  });

  const closePromise = new Promise((resolve) => {
    child.on("close", (code, signal) => {
      resolve({ code, signal });
    });
  });

  const forwardPromises = [];
  if (child.stdout) {
    forwardPromises.push(forward(child.stdout, process.stdout));
  }
  if (child.stderr) {
    forwardPromises.push(forward(child.stderr, process.stderr));
  }
  // 出力が無いまま起こせなかった場合でも forward が終わるよう、error でも進める。
  const forwardsDone = Promise.all(forwardPromises).catch(() => {});

  const result = await Promise.race([
    closePromise,
    errorPromise.then(() => closePromise),
  ]);
  await forwardsDone;

  if (spawnError !== null) {
    const message =
      spawnError instanceof Error ? spawnError.message : String(spawnError);
    await writeStampedLine(
      process.stderr,
      `${program}: ${message}\n`,
    );
    process.exitCode = 127;
    return;
  }

  const { code, signal } = result;
  if (signal !== null && signal !== undefined) {
    for (const [sig, handler] of handlers) {
      process.removeListener(sig, handler);
    }
    try {
      process.kill(process.pid, signal);
    } catch {
      process.exitCode = 1;
      return;
    }
    // 合図の既定の動作で終わるのが通常。万一届かなかった場合の保険。
    setTimeout(() => {
      process.exit(1);
    }, 1000).unref();
    return;
  }

  process.exitCode = code ?? 0;
}

await main();

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
    try {
      sink.write(stamper(Buffer.from(text, "utf8")), (error) => {
        resolve(error ? false : true);
      });
    } catch {
      resolve(false);
    }
  });
}

function forward(source, sink) {
  const stamper = createLineStamper();
  return new Promise((resolve) => {
    let pendingCount = 0;
    let ended = false;
    let settled = false;
    let failure = null;
    const maybeFinish = () => {
      if (!settled && ended && (failure !== null || pendingCount === 0)) {
        settled = true;
        resolve(failure);
      }
    };
    const noteFailure = (error) => {
      if (failure === null) {
        failure = error instanceof Error ? error : new Error(String(error));
      }
      // 書き込みが失敗した流れは転送をやめ、子の出力を捨て読みにする。
      // drain は待たずに再開し、子が出力で止まらないようにする。
      try {
        source.resume();
      } catch {
        // 再開できなくても、失敗は値として返す。
      }
      maybeFinish();
    };
    if (typeof sink.on === "function") {
      sink.on("error", noteFailure);
    }
    source.on("data", (chunk) => {
      if (failure !== null) {
        return;
      }
      const out = stamper(chunk);
      if (out.length === 0) {
        return;
      }
      pendingCount += 1;
      let ok;
      try {
        ok = sink.write(out, (error) => {
          pendingCount -= 1;
          if (error) {
            noteFailure(error);
            return;
          }
          maybeFinish();
        });
      } catch (error) {
        pendingCount -= 1;
        noteFailure(error);
        return;
      }
      if (!ok && failure === null) {
        source.pause();
        sink.once("drain", () => {
          source.resume();
        });
      }
    });
    source.on("end", () => {
      ended = true;
      maybeFinish();
    });
    source.on("error", (error) => {
      ended = true;
      noteFailure(error);
    });
  });
}

function createForwardEntries(child) {
  const entries = [];
  if (child.stdout) {
    entries.push({
      name: "stdout",
      sink: process.stdout,
      result: forward(child.stdout, process.stdout),
    });
  }
  if (child.stderr) {
    entries.push({
      name: "stderr",
      sink: process.stderr,
      result: forward(child.stderr, process.stderr),
    });
  }
  return entries;
}

async function reportForwardFailure(forwardEntries, forwardResults) {
  const firstFailureIndex = forwardResults.findIndex((value) => value !== null);
  if (firstFailureIndex === -1) {
    return;
  }
  const failure = forwardResults[firstFailureIndex];
  const reportEntry = forwardEntries.find(
    (_, index) => forwardResults[index] === null,
  );
  if (reportEntry === undefined) {
    return;
  }
  const message = failure instanceof Error ? failure.message : String(failure);
  // 失敗していないもう一方の流れへ 1 行だけ書く。それも失敗したら何もしない。
  await writeStampedLine(
    reportEntry.sink,
    `with-timestamps: 書き込みに失敗しました: ${message}\n`,
  );
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

  const forwardEntries = createForwardEntries(child);
  // forward は失敗を reject せず値として返すため、ここで失敗が捨てられることはない。
  const forwardsDone = Promise.all(forwardEntries.map((entry) => entry.result));

  const result = await Promise.race([
    closePromise,
    errorPromise.then(() => closePromise),
  ]);
  const forwardResults = await forwardsDone;

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

  await reportForwardFailure(forwardEntries, forwardResults);

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

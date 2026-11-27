// Web の起動を包むラッパー（issue #186、docs/design/issue-186.md）。
//
// node scripts/with-timestamps.mjs <program> [args...]
// 子をシェルを通さずに起こし、子の標準出力と標準エラーの各行に時刻を付けて、
// それぞれ自分の標準出力と標準エラーへ書く。受けた合図は子へ送り、
// 子の終わり方（終了コードか合図）をそのまま自分の終わり方にする。
import { spawn } from "node:child_process";
import { constants } from "node:os";
import { createLineStamper } from "./line-stamper.mjs";

const FORWARDED_SIGNALS = ["SIGTERM", "SIGINT", "SIGHUP"];

// 自分へ送ってよい合図（issue #413、docs/design/issue-186.md「#413 の直し」）。
// 既定の動作が core を作らずに終わり、Node が無視も別の用途にも使わないもの。
// これ以外（SIGPIPE・SIGXFSZ は Node が無視し、SIGUSR1 は Inspector を起こし、
// SIGABRT・SIGSEGV などは core とクラッシュレポートを作る）は自分へ送らず、
// 128 + 番号で終わる。
const SELF_SENDABLE_SIGNALS = new Set([
  "SIGHUP",
  "SIGINT",
  "SIGTERM",
  "SIGKILL",
  "SIGALRM",
  "SIGUSR2",
  "SIGVTALRM",
  "SIGPROF",
]);

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

function finishForward(state) {
  if (!state.settled && state.ended && (state.failure !== null || state.pendingCount === 0)) {
    state.settled = true;
    state.resolve(state.failure);
  }
}

function failForward(state, source, onFirstFailure, error) {
  const isFirst = state.failure === null;
  if (isFirst) {
    state.failure = error instanceof Error ? error : new Error(String(error));
  }
  // 書き込みが失敗した流れは転送をやめ、子の出力を捨て読みにする。
  // drain は待たずに再開し、子が出力で止まらないようにする。
  try {
    source.resume();
  } catch {
    // 再開できなくても、失敗は値として返す。
  }
  if (isFirst) {
    onFirstFailure(state.failure);
  }
  finishForward(state);
}

function forwardChunk(state, source, sink, chunk) {
  if (state.failure !== null) {
    return;
  }
  if (chunk.length > 0) {
    state.atLineStart = chunk[chunk.length - 1] === 0x0a;
  }
  const out = state.stamper(chunk);
  if (out.length === 0) {
    return;
  }
  state.pendingCount += 1;
  let ok;
  try {
    ok = sink.write(out, (error) => {
      state.pendingCount -= 1;
      if (error) {
        failForward(state, source, state.onFirstFailure, error);
        return;
      }
      finishForward(state);
    });
  } catch (error) {
    state.pendingCount -= 1;
    failForward(state, source, state.onFirstFailure, error);
    return;
  }
  if (!ok && state.failure === null) {
    source.pause();
    sink.once("drain", () => {
      source.resume();
    });
  }
}

// 書き込みの失敗の知らせを、この流れへ独立した行として書く。
// 行の途中なら前に改行を 1 つ足し、知らせのあとは行の頭に戻す
// （子の行の続きには新しい時刻が付く）。
function writeForwardNotice(state, source, sink, text) {
  const bytes = Buffer.from(`${state.atLineStart ? "" : "\n"}${text}`, "utf8");
  state.atLineStart = true;
  const out = state.stamper(bytes);
  if (out.length === 0) {
    return;
  }
  try {
    sink.write(out, (error) => {
      if (error) {
        failForward(state, source, state.onFirstFailure, error);
      }
    });
  } catch (error) {
    failForward(state, source, state.onFirstFailure, error);
  }
}

function forward(source, sink, onFirstFailure) {
  const state = {
    stamper: createLineStamper(),
    atLineStart: true,
    pendingCount: 0,
    ended: false,
    settled: false,
    failure: null,
    resolve: () => {},
    onFirstFailure,
  };
  const result = new Promise((resolve) => {
    state.resolve = resolve;
  });
  const noteFailure = (error) => failForward(state, source, onFirstFailure, error);
  if (typeof sink.on === "function") {
    sink.on("error", noteFailure);
  }
  source.on("data", (chunk) => forwardChunk(state, source, sink, chunk));
  const noteEnded = () => {
    state.ended = true;
    finishForward(state);
  };
  source.on("end", noteEnded);
  source.on("close", noteEnded);
  source.on("error", (error) => {
    state.ended = true;
    noteFailure(error);
  });
  const writeNotice = (text) => writeForwardNotice(state, source, sink, text);
  return { result, writeNotice };
}

function createForwardEntries(child, notifyFailure) {
  const entries = [];
  if (child.stdout) {
    entries.push({
      name: "stdout",
      ...forward(child.stdout, process.stdout, (error) => notifyFailure("stdout", error)),
    });
  }
  if (child.stderr) {
    entries.push({
      name: "stderr",
      ...forward(child.stderr, process.stderr, (error) => notifyFailure("stderr", error)),
    });
  }
  return entries;
}

// 起こせなかったことを、時刻付きの 1 行で標準エラーへ書く。
// spawn が投げたときはその例外を、pid が無いときは続いて来る error を理由にする。
async function reportSpawnFailure(program, child, spawnThrown) {
  let reason;
  if (spawnThrown !== null) {
    reason = spawnThrown instanceof Error ? spawnThrown.message : String(spawnThrown);
  } else {
    // 子の流れの error で落ちないよう、受け手を先に付ける。
    if (child.stdout) {
      child.stdout.on("error", () => {});
    }
    if (child.stderr) {
      child.stderr.on("error", () => {});
    }
    const error = await new Promise((resolve) => {
      child.once("error", resolve);
    });
    reason = error instanceof Error ? error.message : String(error);
  }
  await writeStampedLine(process.stderr, `${JSON.stringify(program)}: ${reason}\n`);
}

function installForwardingHandlers(child) {
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
  return handlers;
}

// 子が合図で終わったときの、自分の終わり方を決める。
// 送ってよい合図で受け手が残っていなければ自分へ送り、飲み込まれたら
// unref しない 1000ms のタイマーで 128 + 番号で終わる。それ以外は
// すぐ 128 + 番号で終わる。
function endBySignal(handlers, signal) {
  for (const [sig, handler] of handlers) {
    process.removeListener(sig, handler);
  }
  const number = constants.signals[signal];
  if (number === undefined) {
    // 名前の無い合図は、close が (0, null) と報告する経路に委ねる。
    process.exitCode = 1;
    return;
  }
  if (SELF_SENDABLE_SIGNALS.has(signal) && process.listenerCount(signal) === 0) {
    try {
      process.kill(process.pid, signal);
    } catch {
      process.exit(128 + number);
      return;
    }
    setTimeout(() => {
      process.exit(128 + number);
    }, 1000);
    return;
  }
  process.exit(128 + number);
}

async function main() {
  // 書き込みのコールバックに EPIPE が来たあとで error の出来事が来ても、
  // 捕まえていない例外にしないよう、標準出力と標準エラーには常に
  // 何もしない error の受け手を付ける（終わるまで外さない）。
  process.stdout.on("error", () => {});
  process.stderr.on("error", () => {});

  const [program, ...args] = process.argv.slice(2);
  if (program === undefined) {
    await writeStampedLine(
      process.stderr,
      "usage: node scripts/with-timestamps.mjs <program> [args...]\n",
    );
    process.exitCode = 2;
    return;
  }

  let child = null;
  let spawnThrown = null;
  try {
    child = spawn(program, args, { stdio: ["inherit", "pipe", "pipe"] });
  } catch (error) {
    spawnThrown = error;
  }

  // 起こせたかは spawn のその場で決める（投げたか、pid が undefined か）。
  if (spawnThrown !== null || child === null || child.pid === undefined) {
    await reportSpawnFailure(program, child, spawnThrown);
    process.exitCode = 127;
    return;
  }

  // 起こせたあとの error（動いている子への kill の失敗など）は、
  // 終わり方の理由にしない。受け手だけ付けて何もしない。
  child.on("error", () => {});

  const handlers = installForwardingHandlers(child);

  const closePromise = new Promise((resolve) => {
    child.on("close", (code, signal) => {
      resolve({ code, signal });
    });
  });

  // 書き込みの失敗は、流れごとに最初の 1 回だけ、すぐもう一方の流れへ知らせる。
  const failedStreams = new Set();
  let forwardEntries = [];
  const notifyFailure = (name, error) => {
    failedStreams.add(name);
    const other = forwardEntries.find((entry) => entry.name !== name);
    if (other === undefined || failedStreams.has(other.name)) {
      return;
    }
    const message = error instanceof Error ? error.message : String(error);
    other.writeNotice(`with-timestamps: ${name} への書き込みに失敗しました: ${message}\n`);
  };
  forwardEntries = createForwardEntries(child, notifyFailure);
  // forward は失敗を reject せず値として返すため、ここで失敗が捨てられることはない。
  const forwardsDone = Promise.all(forwardEntries.map((entry) => entry.result));

  const result = await closePromise;
  // 合図を自分へ送るのと process.exit は、両方の流れの転送の終わりを
  // 待ったあとに限る（最後の行を落とさない）。
  await forwardsDone;

  const { code, signal } = result;
  if (signal !== null && signal !== undefined) {
    endBySignal(handlers, signal);
    return;
  }

  process.exitCode = code ?? 0;
}

let unexpectedHandled = false;
async function handleUnexpected(error) {
  if (unexpectedHandled) {
    return;
  }
  unexpectedHandled = true;
  const message = error instanceof Error ? error.message : String(error);
  try {
    await writeStampedLine(process.stderr, `with-timestamps: 想定外の失敗: ${message}\n`);
  } catch {
    // 書けなくても、1 で終わることは続ける。
  }
  process.exit(1);
}

process.on("unhandledRejection", (error) => {
  void handleUnexpected(error);
});
process.on("uncaughtException", (error) => {
  void handleUnexpected(error);
});

try {
  await main();
} catch (error) {
  await handleUnexpected(error);
}

#!/usr/bin/env node
// takt-mcp モックの標準入出力を中継し、応答の転送だけを外側から遅らせる E2E 用ラッパー。
//
// backend はモックを毎サイクル起動するため、遅延量を起動引数に固定すると切り替えのたびに
// モックを起動し直す必要がある。遅延量をファイルで受け取り、転送する行ごとに読むことで、
// モックを起動したまま E2E から遅延の有無と量を切り替えられるようにする。
import { spawn } from "node:child_process";
import { readFileSync } from "node:fs";
import { createInterface } from "node:readline";

const argv = process.argv.slice(2);
const separator = argv.indexOf("--");
if (separator < 0 || separator === argv.length - 1) {
  process.stderr.write("-- の後ろにモックの起動コマンドを指定してください。\n");
  process.exit(2);
}

const ownArgs = argv.slice(0, separator);
const delayIndex = ownArgs.indexOf("--delay-file");
if (delayIndex < 0 || delayIndex + 1 >= ownArgs.length) {
  process.stderr.write("--delay-file を指定してください。\n");
  process.exit(2);
}

const delayPath = ownArgs[delayIndex + 1];
const childArgs = argv.slice(separator + 1);

const child = spawn(process.execPath, childArgs, { stdio: ["pipe", "pipe", "ignore"] });
child.on("error", (error) => {
  process.stderr.write(`モックを起動できませんでした: ${error.message}\n`);
  process.exit(1);
});

// 遅延ファイルは E2E が原子的に置換する。読めない場合や値が不正な場合は、遅延なしの
// 透過として扱う（通常経路を遅延ファイルの一時的な不在で止めない）。
const readDelayMs = () => {
  let parsed;
  try {
    parsed = JSON.parse(readFileSync(delayPath, "utf8"));
  } catch {
    return 0;
  }
  const ms = parsed?.stdoutDelayMs;
  return typeof ms === "number" && Number.isFinite(ms) && ms > 0 ? ms : 0;
};

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

const writeLine = (line) =>
  new Promise((resolve) => {
    if (process.stdout.write(`${line}\n`)) {
      resolve();
      return;
    }
    process.stdout.once("drain", resolve);
  });

// 子の stdout を1行ずつ順に転送する。並行に sleep すると行の順序が入れ替わり、
// JSON-RPC の応答対応が壊れるため、行ごとに待ってから次を読む。
const forward = async () => {
  const lines = createInterface({ input: child.stdout, crlfDelay: Infinity });
  for await (const line of lines) {
    const ms = readDelayMs();
    if (ms > 0) {
      await sleep(ms);
    }
    await writeLine(line);
  }
};

process.stdin.pipe(child.stdin);

child.on("exit", (code) => {
  process.exit(code === null ? 1 : code);
});

const forwardSignal = (signal) => {
  if (child.exitCode !== null || child.signalCode !== null) {
    return;
  }
  child.kill(signal);
};

// 親が止められたときにモックだけ残らないよう、SIGTERM/SIGINT を子へ渡し、
// 子が応答しない場合だけ 3 秒後に強制終了する。
const onSignal = (signal) => {
  forwardSignal(signal);
  setTimeout(() => forwardSignal("SIGKILL"), 3_000).unref();
};
process.on("SIGTERM", () => onSignal("SIGTERM"));
process.on("SIGINT", () => onSignal("SIGINT"));

await forward();

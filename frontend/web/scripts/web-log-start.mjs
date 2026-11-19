// `next` CLI を時刻付きの子プロセスとして起動し、起動引数と終了コードを中継する（issue #186）。
//
// 時刻の前置は子プロセスの `--import`（web-log-stdout.mjs / web-log-stderr.mjs）が
// 行う。子の stdout / stderr は親へ継承するため、子が書いた行はそのまま
// launchd のログファイルへ届く。stdio を継承すると親プロセスの write 差し替えは
// 子へ効かない（子は継承した fd へ直接書く）ため、差し替えは子側で行う。
//
// 既存呼び出し元は `next` の起動引数（`start -H -p` など）をそのまま渡す。
// `--emit-partial <text>` は時刻前置の回帰確認用の入口で、`next` へは渡さず、
// 同じ `--import` を読み込んだ子プロセスに改行なしの文字列を書かせて終了する
// （終了時の部分行 flush を実プロセスの終了経路で観測するため）。
import { spawn } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const nextBin = join(scriptDir, "..", "node_modules", "next", "dist", "bin", "next");

const args = process.argv.slice(2);
const importArgs = [
  "--import",
  pathToFileURL(join(scriptDir, "web-log-stdout.mjs")).href,
  "--import",
  pathToFileURL(join(scriptDir, "web-log-stderr.mjs")).href,
];

const emitPartialIndex = args.indexOf("--emit-partial");
if (emitPartialIndex !== -1) {
  const text = String(args[emitPartialIndex + 1] ?? "");
  const emitter = spawn(
    process.execPath,
    [...importArgs, "-e", "process.stdout.write(process.argv[1])", text],
    { stdio: "inherit" },
  );
  emitter.on("exit", (code) => {
    process.exit(code ?? 1);
  });
} else {
  const child = spawn(process.execPath, [...importArgs, nextBin, ...args], { stdio: "inherit" });

  const forwardSignal = (signal) => {
    child.kill(signal);
  };
  process.on("SIGTERM", forwardSignal);
  process.on("SIGINT", forwardSignal);

  child.on("exit", (code, signal) => {
    if (signal !== null) {
      process.kill(process.pid, signal);
      return;
    }
    process.exit(code ?? 1);
  });
}

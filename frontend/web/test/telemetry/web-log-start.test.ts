// Web プロセスの起動ランナーが起動引数・終了コード・終了時の部分行を保つ契約（issue #186）。
//
// ランナーは `next` CLI を子プロセスで起動し、子へ `--import` を渡して子自身の
// stdout / stderr を時刻付きにする。子の stdio は親へ継承するため、時刻付きの行は
// そのまま親のログファイルへ流れる。ここでは実 `next` の起動に依存しない不正な
// サブコマンドを使い、ランナー経由の出力が時刻付きであること、非 0 終了コードを
// 中継すること、終了時に改行なしの部分行が失われないことを観測する。
import test from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const runner = join(scriptDir, "..", "..", "scripts", "web-log-start.mjs");
const STAMP_LINE = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z /;

const runRunner = (args: string[]) =>
  spawnSync(process.execPath, [runner, ...args], { encoding: "utf8" });

test("ランナー経由の出力は各行が UTC 時刻で始まる", () => {
  const result = runRunner(["unknown-subcommand"]);

  const lines = `${result.stdout}\n${result.stderr}`
    .split("\n")
    .filter((line) => line.trim() !== "");
  assert.ok(lines.length > 0, "next は不一致のサブコマンドでメッセージを出す");
  for (const line of lines) {
    assert.match(line, STAMP_LINE);
  }
});

test("子プロセスの非 0 終了コードをランナーの終了コードとして返す", () => {
  const result = runRunner(["unknown-subcommand"]);

  assert.notEqual(result.status, 0);
});

test("子プロセスが改行なしの部分行で終了しても、終了時に時刻付きで出力する", () => {
  const result = runRunner(["--emit-partial", "terminal-partial-without-newline"]);

  const line = (result.stdout ?? "")
    .split("\n")
    .find((candidate) => candidate.endsWith("terminal-partial-without-newline"));
  assert.ok(line !== undefined, `終了時の部分行が必要です: ${result.stdout}`);
  assert.match(line, STAMP_LINE);
});

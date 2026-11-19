#!/usr/bin/env node
// SSH の接続先だけをローカルへ置き換える。本番の読取プログラムをそのまま実行する。
const fs = require("node:fs");
const nodePath = require("node:path");
const { spawnSync } = require("node:child_process");
const args = process.argv.slice(2);
// 読取コマンドは本番では /bin/sh -lc（ログインシェル）で node を探す。ローカルの
// fake には実在のログイン先が無く、~/.profile 等が PATH を再計算すると node を見失う
// 環境がある。テストでは継承 PATH をそのまま使う -c で実行し、この fake を動かして
// いる node のディレクトリを PATH の先頭へ足して同じ node に揃える。
const env = { ...process.env, PATH: `${nodePath.dirname(process.execPath)}:${process.env.PATH ?? ""}` };
const readerCommand = (arg) => arg.startsWith("/bin/sh -lc ") ? "/bin/sh -c " + arg.slice("/bin/sh -lc ".length) : arg;
if (args[0] !== "-p" || args[1] !== "2222" || args[2] !== "-oBatchMode=yes" || args[3] !== "fixture-host") {
  process.exit(30);
}
const command = args.slice(4);
if (command[0] === "fixture-mcp") {
  const result = spawnSync("node", [process.env.FIXTURE_MCP, ...JSON.parse(process.env.FIXTURE_MCP_ARGS)], { stdio: "inherit", env });
  process.exit(result.status ?? 31);
}
fs.appendFileSync(process.env.FIXTURE_READ_CALLS, JSON.stringify(args) + "\n");
if (process.env.FIXTURE_READ_FAIL === "yes") process.exit(32);
if (command.length !== 1 || !command[0].startsWith("/bin/sh -lc ")) process.exit(33);
const request = JSON.parse(fs.readFileSync(0, "utf8"));
if (request.project === "/remote/project" || request.project === process.env.FIXTURE_LOGICAL_PROJECT) request.project = process.env.FIXTURE_PROJECT;
const result = spawnSync("/bin/sh", ["-c", readerCommand(command[0])], { input: JSON.stringify(request), encoding: "utf8", env, cwd: nodePath.dirname(process.env.FIXTURE_PROJECT) });
process.stdout.write(result.stdout ?? "");
process.stderr.write(result.stderr ?? "");
process.exit(result.status ?? 34);

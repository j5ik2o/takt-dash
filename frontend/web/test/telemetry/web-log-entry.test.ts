// Web の起動入口が時刻付きログのランナーを指す契約（issue #186）。
//
// launchd も e2e の support も `frontend/web` の `npm start` と同じ起動引数
// （`start -H -p`）を渡す。入口が `next start` 直起動のままだと、時刻が付かない
// ログが本番経路へ流れるため、`package.json` の `start` がランナーを指すことと、
// e2e の Web 起動が同じランナーを経由することを観測する。
import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";

const webRoot = new URL("../../", import.meta.url);
const readText = (path: string) => readFile(fileURLToPath(new URL(path, webRoot)), "utf8");

test("frontend/web の npm start は時刻付きランナーを起動する", async () => {
  const manifest = JSON.parse(await readText("package.json")) as { scripts?: Record<string, string> };

  assert.match(manifest.scripts?.start ?? "", /web-log-start\.mjs/);
});

test("e2e の Web 起動は本番と同じランナーを経由する", async () => {
  const stack = await readText("../../e2e/support/stack.ts");

  assert.match(stack, /web-log-start\.mjs/);
});

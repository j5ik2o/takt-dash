// 否定契約と境界の契約（C1/C13/C19）。
//
// C1: ブラウザ側のコードが @grpc/grpc-js や生成クライアントを import しない。
// C13: モバイルで1列化する CSS 規則がある。
// C19: frontend/web のコードが .takt や SQLite を直接読まない。
import test from "node:test";
import assert from "node:assert/strict";
import { readdir, readFile } from "node:fs/promises";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const webRoot = fileURLToPath(new URL("../", import.meta.url));

async function listFiles(dir: string, extensions: string[]): Promise<string[]> {
  let entries;
  try {
    entries = await readdir(dir, { withFileTypes: true });
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") {
      return [];
    }
    throw error;
  }
  const files: string[] = [];
  for (const entry of entries) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) {
      files.push(...(await listFiles(path, extensions)));
    } else if (extensions.some((extension) => entry.name.endsWith(extension))) {
      files.push(path);
    }
  }
  return files;
}

const read = (path: string) => readFile(path, "utf8");

test("クライアントコンポーネントは gRPC クライアントを import しない", async () => {
  const clientFiles = [
    ...(await listFiles(join(webRoot, "app"), [".tsx", ".ts"])),
    ...(await listFiles(join(webRoot, "src", "components"), [".tsx", ".ts"])),
  ].filter((path) => !path.endsWith(".test.ts") && !path.endsWith(".test.mts"));

  const offenders: string[] = [];
  for (const path of clientFiles) {
    const source = await read(path);
    if (!/^\s*["']use client["']/.test(source)) {
      continue;
    }
    if (/@grpc\/grpc-js/.test(source) || /proto\/taktdash/.test(source) && /TaktDashServiceClient/.test(source)) {
      offenders.push(path.replace(webRoot, ""));
    }
  }
  assert.deepEqual(offenders, [], "クライアントコンポーネントから gRPC クライアントを import しない");
});

test("globals.css は狭い幅で1列化するメディアクエリを持つ", async () => {
  const css = await read(join(webRoot, "app", "globals.css"));
  assert.match(css, /@media[^{]*max-width/);
  assert.match(css, /grid-template-columns:\s*1fr/);
});

test("画面のコードは dangerouslySetInnerHTML を使わない", async () => {
  const files = [
    ...(await listFiles(join(webRoot, "app"), [".tsx", ".ts"])),
    ...(await listFiles(join(webRoot, "src"), [".tsx", ".ts"])),
  ].filter((path) => !path.endsWith(".test.ts") && !path.endsWith(".test.mts"));

  const offenders: string[] = [];
  for (const path of files) {
    const source = await read(path);
    if (/dangerouslySetInnerHTML/.test(source)) {
      offenders.push(path.replace(webRoot, ""));
    }
  }
  assert.deepEqual(offenders, [], "dangerouslySetInnerHTML を使わない");
});

test("frontend/web のコードは .takt や SQLite を直接読まない", async () => {
  const files = [
    ...(await listFiles(join(webRoot, "app"), [".tsx", ".ts"])),
    ...(await listFiles(join(webRoot, "src"), [".tsx", ".ts"])),
  ].filter((path) => !path.includes(`${join("src", "proto")}`) && !path.endsWith(".test.ts") && !path.endsWith(".test.mts"));

  const offenders: string[] = [];
  for (const path of files) {
    const source = await read(path);
    if (/"\.takt|'\.takt|\.takt\/|sqlite|better-sqlite3/i.test(source)) {
      offenders.push(path.replace(webRoot, ""));
    }
  }
  assert.deepEqual(offenders, [], ".takt や SQLite へ直接触れない");
});

import { readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import type { Page } from "@playwright/test";
import { GOAL_TEXT, RUN_SLUG } from "../support/fixture";
import { expect, test } from "../support/test";

// issue #430: エラー画面の「もう一度読み込む」が、サーバーの再取得まで行って復帰するか。
//
// run 詳細の取得を一度だけ失敗させて error.tsx を出し、ボタンを1回押したときに
// GetRunDetail の再取得が走り、成功したら元の run 詳細へ戻ることを実ブラウザで観測する。
// 取得の失敗は、backend が run の保存場所を読む meta.json の status を壊して INTERNAL にし、
// 成功は元の meta.json へ戻して作る（backend も Web も本番のコードのまま）。
// あわせて、失敗が続くときに1回のクリックで再取得が1回を超えないこと（無限ループ・
// 二重送信が無いこと）を同じファイルで確かめる。
const RUN_URL_PATH = `/runs/${RUN_SLUG}`;
const metaPath = (projectDir: string): string => path.join(projectDir, ".takt", "runs", RUN_SLUG, "meta.json");

// status は RunStatus として解釈できない値にし、backend の RunReadFailure::InvalidMeta から
// INTERNAL を返させる。文言やその他の欄は元のまま残す。
const corruptMeta = async (projectDir: string): Promise<Record<string, unknown>> => {
  const file = metaPath(projectDir);
  const original = JSON.parse(await readFile(file, "utf8")) as Record<string, unknown>;
  await writeFile(file, JSON.stringify({ ...original, status: "issue-430-broken" }), "utf8");
  return original;
};

const restoreMeta = async (projectDir: string, meta: Record<string, unknown>): Promise<void> => {
  await writeFile(metaPath(projectDir), JSON.stringify(meta), "utf8");
};

// エラー画面から押すボタン。文言は app/error.tsx のもの。
const reloadButton = (page: Page) => page.getByRole("button", { name: "もう一度読み込む" });

// run 詳細の URL への要求（初回の文書と、reset の先で起きる RSC の再取得の両方）を数える。
const countRunRequests = (page: Page): (() => number) => {
  let count = 0;
  page.on("request", (request) => {
    if (new URL(request.url()).pathname === RUN_URL_PATH) {
      count += 1;
    }
  });
  return () => count;
};

test("もう一度読み込むを1回押すと、run 詳細を取り直して元の画面に戻る", async ({ page, isolatedStack: stack }) => {
  const original = await corruptMeta(stack.projectDir);

  const response = await page.goto(`${stack.webUrl}${RUN_URL_PATH}`);
  expect(response?.status()).toBe(500);
  await expect(page.getByRole("heading", { name: "画面を表示できませんでした", level: 1 })).toBeVisible();
  await expect(reloadButton(page)).toBeVisible();

  // 取得が成功する状態へ戻す。ここで押した1回が再取得を起こせば、run 詳細が出る。
  await restoreMeta(stack.projectDir, original);
  const runRequests = countRunRequests(page);

  await reloadButton(page).click();

  await expect(page.getByRole("heading", { name: GOAL_TEXT, level: 1 })).toBeVisible();
  await expect(page.getByRole("heading", { name: "画面を表示できませんでした", level: 1 })).toHaveCount(0);
  expect(runRequests()).toBe(1);
});

test("失敗が続くときも、1回のクリックで再取得は1回までで、無限ループにも二重送信にもならない", async ({
  page,
  isolatedStack: stack,
}) => {
  await corruptMeta(stack.projectDir);

  await page.goto(`${stack.webUrl}${RUN_URL_PATH}`);
  await expect(page.getByRole("heading", { name: "画面を表示できませんでした", level: 1 })).toBeVisible();

  // 壊れたまま押す。再取得が続くなら要求が増え続けるので、待ってから数える。
  const runRequests = countRunRequests(page);
  await reloadButton(page).click();
  await page.waitForTimeout(1_500);

  expect(runRequests()).toBeLessThanOrEqual(1);
  await expect(page.getByRole("heading", { name: "画面を表示できませんでした", level: 1 })).toHaveCount(1);
});

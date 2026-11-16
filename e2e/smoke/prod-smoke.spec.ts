import { expect, test, type Page } from "@playwright/test";

// 展開済みの本番に対する読み取り専用スモーク。GET とリンク遷移だけを行い、
// 回答・書き込み・設定変更・CSRF 境界の書込は行わない。
// 特定のゴール名や件数など本番データの固有値は仮定せず、役割と URL パターンで観測する。

type CollectedErrors = string[];

const collectConsoleErrors = (page: Page): CollectedErrors => {
  const errors: CollectedErrors = [];
  page.on("pageerror", (error) => {
    errors.push(error.message);
  });
  page.on("console", (message) => {
    if (message.type() !== "error") {
      return;
    }
    errors.push(message.text());
  });
  return errors;
};

// 読み取り専用の契約を、このスモークが発行する要求の method で観測する。
const collectWriteRequests = (page: Page): string[] => {
  const writes: string[] = [];
  page.on("request", (request) => {
    if (request.method() !== "GET") {
      writes.push(`${request.method()} ${request.url()}`);
    }
  });
  return writes;
};

const gotoOk = async (page: Page, path: string): Promise<void> => {
  const response = await page.goto(path);
  expect(response?.status(), `${path} の HTTP ステータス`).toBe(200);
};

test("概要ページが 200 で描画され主要領域が見える", async ({ page }) => {
  const errors = collectConsoleErrors(page);
  const writes = collectWriteRequests(page);

  await gotoOk(page, "/");
  await expect(page.getByRole("heading", { name: "概要", level: 1 })).toBeVisible();
  await expect(page.getByRole("region", { name: "概要の集計" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "プロジェクト", level: 2 })).toBeVisible();
  await expect(page.getByRole("heading", { name: "サーバー", level: 2 })).toBeVisible();

  expect(writes).toEqual([]);
  expect(errors).toEqual([]);
});

test("ゴール一覧からゴール詳細へ遷移できる", async ({ page }) => {
  const errors = collectConsoleErrors(page);
  const writes = collectWriteRequests(page);

  await gotoOk(page, "/goals");
  await expect(page.getByRole("heading", { name: "ゴール", level: 1 })).toBeVisible();

  const firstGoalLink = page.locator('main a[href^="/goals/"]').first();
  await expect(firstGoalLink).toBeVisible();
  const goalHref = await firstGoalLink.getAttribute("href");
  expect(goalHref, "ゴール詳細リンクの href").toMatch(/^\/goals\/[^/]+$/);

  await firstGoalLink.click();
  await expect(page).toHaveURL((url) => url.pathname === goalHref);
  await expect(page.getByRole("heading", { name: /^ゴール /, level: 1 })).toBeVisible();
  await expect(page.getByRole("link", { name: "ゴール一覧に戻る" }).first()).toBeVisible();

  expect(writes).toEqual([]);
  expect(errors).toEqual([]);
});

test("判断待ちページが表示される", async ({ page }) => {
  const errors = collectConsoleErrors(page);
  const writes = collectWriteRequests(page);

  await gotoOk(page, "/decisions");
  await expect(page.getByRole("heading", { name: "判断待ち", level: 1 })).toBeVisible();
  await expect(page.getByRole("region", { name: "判断待ちの一覧" })).toBeVisible();

  expect(writes).toEqual([]);
  expect(errors).toEqual([]);
});

test("会話ページへ直接リンクで入れる", async ({ page }) => {
  const errors = collectConsoleErrors(page);
  const writes = collectWriteRequests(page);

  await gotoOk(page, "/conversations");
  await expect(page.getByRole("heading", { name: "会話", level: 1 })).toBeVisible();
  await expect(page.getByRole("combobox", { name: "プロジェクト" })).toBeVisible();

  expect(writes).toEqual([]);
  expect(errors).toEqual([]);
});

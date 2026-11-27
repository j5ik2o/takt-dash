import { test as base, expect } from "../support/test";
import { startStack, type Stack } from "../support/stack";
import { buildMockState, QUESTION_BODY } from "../support/fixture";

// issue #315 の狭い幅（375px）の見た目を、実 Chrome の描画で測る（docs/design/issue-315.md）。
// D7：結果の行に足す「閉じる」が画面の幅からはみ出さず、押す場所が 44px 以上ある。
// D5：句点の無い長い登録案でも、確認ダイアログの確定と「やめる」に届く。
const MOBILE = { width: 375, height: 667 } as const;
const ANSWER_TEXT = "E2E 狭い幅の確かめの回答";

// 句点の無い長い登録案の本文。goalHeading は句点で切れないので全文を返す。
const LONG_PROPOSAL = `狭い幅の確認ダイアログを確かめる登録案の本文${"ながいぶん".repeat(400)}`;

const longProposalStack = async ({}, use: (stack: Stack) => Promise<void>): Promise<void> => {
  const stack = await startStack({
    mockState: (now) => ({
      ...buildMockState(now),
      proposals: [{ revision: 7, repositoryDirectory: "/abs/e2e-corporate-site", summary: LONG_PROPOSAL }],
    }),
  });
  try {
    await use(stack);
  } finally {
    await stack.stop();
  }
};

const test = base.extend<{ longProposalStack: Stack }>({ longProposalStack });

test("D7: 375px の幅でも、結果の行は画面からはみ出さず、「閉じる」は 44px 以上の押す場所を持ち、押すと行が消える", async ({
  page,
  isolatedStack: stack,
}) => {
  await page.setViewportSize(MOBILE);
  await page.goto(`${stack.webUrl}/decisions`);
  const tray = page.getByRole("region", { name: "判断待ちの一覧" });
  const item = tray.getByRole("listitem").filter({ hasText: QUESTION_BODY });
  await expect(item).toBeVisible();
  await item.getByRole("textbox").fill(ANSWER_TEXT);
  await item.getByRole("button", { name: /回答する/ }).click();

  const row = tray.locator(".decision-results li").filter({ hasText: "回答を送信しました。" });
  await expect(row).toBeVisible({ timeout: 15_000 });
  const close = row.getByRole("button", { name: "閉じる" });
  await expect(close).toBeVisible();

  const rowBox = await row.boundingBox();
  const closeBox = await close.boundingBox();
  expect(rowBox, "結果の行の箱がある").not.toBeNull();
  expect(closeBox, "「閉じる」の箱がある").not.toBeNull();
  expect(rowBox!.x).toBeGreaterThanOrEqual(0);
  expect(rowBox!.x + rowBox!.width, "結果の行の右端が画面の幅に収まる").toBeLessThanOrEqual(MOBILE.width);
  expect(closeBox!.width, "「閉じる」の幅が 44px 以上").toBeGreaterThanOrEqual(44);
  expect(closeBox!.height, "「閉じる」の高さが 44px 以上").toBeGreaterThanOrEqual(44);
  expect(closeBox!.x, "「閉じる」は行の中にある").toBeGreaterThanOrEqual(rowBox!.x);
  expect(closeBox!.x + closeBox!.width, "「閉じる」は行の中にある").toBeLessThanOrEqual(rowBox!.x + rowBox!.width + 0.5);
  expect(closeBox!.y).toBeGreaterThanOrEqual(rowBox!.y - 0.5);
  expect(closeBox!.y + closeBox!.height).toBeLessThanOrEqual(rowBox!.y + rowBox!.height + 0.5);
  // ページ全体も横にはみ出さない。
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(MOBILE.width);

  await close.click();
  await expect(row).toHaveCount(0);
});

test("D5: 375px の幅で句点の無い長い登録案を開いても、確認ダイアログは画面の高さに収まり、確定と「やめる」に届く", async ({
  page,
  longProposalStack: stack,
}) => {
  await page.setViewportSize(MOBILE);
  await page.goto(`${stack.webUrl}/decisions`);
  const tray = page.getByRole("region", { name: "判断待ちの一覧" });
  const item = tray.getByRole("listitem").filter({ hasText: "狭い幅の確認ダイアログを確かめる登録案の本文" });
  await expect(item).toBeVisible({ timeout: 15_000 });

  await item.getByRole("button", { name: "ゴール登録を承認する" }).first().click();
  const dialog = page.getByRole("dialog", { name: "ゴール登録の承認" });
  await expect(dialog).toBeVisible();
  const dialogBox = await dialog.boundingBox();
  expect(dialogBox, "ダイアログの箱がある").not.toBeNull();
  expect(dialogBox!.y, "ダイアログの上端が画面の中").toBeGreaterThanOrEqual(0);
  expect(dialogBox!.y + dialogBox!.height, "ダイアログの下端が画面の高さに収まる").toBeLessThanOrEqual(MOBILE.height);

  // ダイアログの中をスクロールして、確定と「やめる」を画面の中に出せる。
  for (const name of ["ゴール登録を承認する", "やめる"]) {
    const button = dialog.getByRole("button", { name });
    await button.scrollIntoViewIfNeeded();
    const box = await button.boundingBox();
    expect(box, `${name} の箱がある`).not.toBeNull();
    expect(box!.y, `${name} の上端が画面の中`).toBeGreaterThanOrEqual(0);
    expect(box!.y + box!.height, `${name} の下端が画面の中`).toBeLessThanOrEqual(MOBILE.height);
    expect(box!.y + box!.height, `${name} はダイアログの中`).toBeLessThanOrEqual(dialogBox!.y + dialogBox!.height + 0.5);
  }

  // 届いた「やめる」で閉じる（確定は押さず、書き込みは出さない）。
  await dialog.getByRole("button", { name: "やめる" }).click();
  await expect(dialog).toBeHidden();
});

import os from "node:os";
import type { Page } from "@playwright/test";
import { expect, test } from "../support/test";
import { PROJECT_NAME, PULL_REQUEST_NUMBER, PULL_REQUEST_URL } from "../support/fixture";

// issue #304: マージの確認ダイアログに、対象のプロジェクトと、プルリクエストを新しいタブで
// 開くリンクが出る。リンクの宛先は takt が持つプルリクエストの URL そのもので、画面を開いた
// ホスト（ループバック、LAN、VPN）に依存しない。
const expectMergeDialogTarget = async (page: Page, webUrl: string): Promise<void> => {
  await page.goto(`${webUrl}/decisions`);

  const item = page
    .getByRole("region", { name: "判断待ちの一覧" })
    .getByRole("listitem")
    .filter({ hasText: "人のマージ待ち" });
  await expect(item).toBeVisible();

  const listLink = item.getByRole("link", { name: `プルリクエスト #${PULL_REQUEST_NUMBER}` });
  await expect(listLink).toHaveAttribute("href", PULL_REQUEST_URL);
  await expect(listLink).toHaveAttribute("target", "_blank");

  await item.getByRole("button", { name: "マージする" }).first().click();

  const dialog = page.getByRole("dialog", { name: "プルリクエストのマージ" });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole("definition").first()).toHaveText(PROJECT_NAME);

  const link = dialog.getByRole("link", { name: `#${PULL_REQUEST_NUMBER} を開く（新しいタブ）` });
  await expect(link).toHaveAttribute("href", PULL_REQUEST_URL);
  await expect(link).toHaveAttribute("target", "_blank");
  await expect(link).toHaveAttribute("rel", "noopener noreferrer");
  // 解決後の宛先も、開いているホストではなくプルリクエストの URL になる。
  expect(await link.evaluate((element) => (element as HTMLAnchorElement).href)).toBe(PULL_REQUEST_URL);

  // リンクはダイアログのフォーカスの循環に入り、キーボードだけで届く。
  await page.keyboard.press("Tab");
  await expect(link).toBeFocused();
  await page.keyboard.press("Shift+Tab");
  await expect(dialog.getByRole("button", { name: "マージする" })).toBeFocused();

  // 確かめるだけで閉じ、マージの書き込みは送らない。
  await dialog.getByRole("button", { name: "やめる" }).click();
  await expect(dialog).toBeHidden();
};

test("マージの確認ダイアログにプロジェクトとプルリクエストへのリンクが出る", async ({ page, isolatedStack: stack }) => {
  await expectMergeDialogTarget(page, stack.webUrl);
  expect((await stack.readCalls()).filter((call) => call.name === "takt_merge_goal_pr")).toHaveLength(0);
});

// ループバック以外の IPv4 アドレス（LAN、VPN）をすべて挙げる。Web は 0.0.0.0 で待ち受けるため、
// どのアドレスからも同じ画面が開く。
const nonLoopbackWebUrls = (webUrl: string): string[] => {
  const port = new URL(webUrl).port;
  return Object.values(os.networkInterfaces())
    .flatMap((entries) => entries ?? [])
    .filter((entry) => entry.family === "IPv4" && !entry.internal)
    .map((entry) => `http://${entry.address}:${port}`);
};

test("LAN や VPN のアドレスで開いた画面でも、マージの確認ダイアログのリンクの宛先は変わらない", async ({
  page,
  isolatedStack: stack,
}, testInfo) => {
  const webUrls = nonLoopbackWebUrls(stack.webUrl);
  if (webUrls.length === 0) {
    testInfo.annotations.push({ type: "note", description: "ループバック以外のアドレスが無く、LAN や VPN のアドレスでは確かめていない" });
    return;
  }
  for (const webUrl of webUrls) {
    await test.step(`${new URL(webUrl).hostname} で開く`, async () => {
      await expectMergeDialogTarget(page, webUrl);
    });
  }
  expect((await stack.readCalls()).filter((call) => call.name === "takt_merge_goal_pr")).toHaveLength(0);
});

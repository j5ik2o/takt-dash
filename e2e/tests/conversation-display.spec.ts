import { expect, test } from "../support/test";
import {
  seedConversation, DAMAGED_TEXT, CURRENT_TEXT, MANAGER_TEXT,
  QUOTED_USER_TEXT, QUOTED_CURRENT_INPUT, ASSISTANT_EXAMPLE, UNCLOSED_RESPONSE,
  CURRENT_HUMAN_TEXT, CURRENT_DAMAGED_TEXT,
} from "../support/conversation-fixture";

test("会話画面は包みとJSONを展開し、文字化けした発言だけに注記する", async ({ page, isolatedStack: stack }, testInfo) => {
  await seedConversation(stack.projectDir);
  await page.goto(stack.webUrl);
  await page.getByRole("link", { name: "会話", exact: true }).click();
  await expect(page).toHaveURL(`${stack.webUrl}/conversations`);
  const row = (seq: number) => page.locator("ol.conversation > li").filter({
    has: page.locator(".conversation-seq", { hasText: new RegExp(`^seq ${seq}$`) }),
  });
  await expect(row(30)).toBeVisible();
  await testInfo.attach("会話の表示", { body: await page.screenshot({ fullPage: true }), contentType: "image/png" });
  await testInfo.attach("発言本文", { body: JSON.stringify(await page.locator("ol.conversation > li").allTextContents()), contentType: "application/json" });
  await expect.soft(row(8).locator(".conversation-text")).toHaveText(DAMAGED_TEXT, { useInnerText: true });
  await expect.soft(row(22).locator(".conversation-text")).toHaveText(CURRENT_TEXT);
  await expect.soft(row(23).locator(".conversation-text")).toHaveText(MANAGER_TEXT);
  await expect.soft(row(8).getByText("元の記録に文字化けがあります。", { exact: true })).toBeVisible();
  await expect.soft(row(22).getByText("元の記録に文字化けがあります。", { exact: true })).toHaveCount(0);
  await expect.soft(row(23).getByText("元の記録に文字化けがあります。", { exact: true })).toHaveCount(0);
  for (const [seq, text] of [[24, QUOTED_USER_TEXT], [25, ASSISTANT_EXAMPLE], [26, UNCLOSED_RESPONSE], [27, QUOTED_CURRENT_INPUT]] as const) {
    expect(await row(seq).locator(".conversation-text").textContent()).toBe(text);
  }
  for (const [seq, text, damaged] of [[28, CURRENT_HUMAN_TEXT, false], [29, CURRENT_HUMAN_TEXT, false], [30, CURRENT_DAMAGED_TEXT, true]] as const) {
    expect(await row(seq).locator(".conversation-text").textContent()).toBe(text);
    await expect(row(seq).getByText("元の記録に文字化けがあります。", { exact: true })).toHaveCount(damaged ? 1 : 0);
  }
});

import { expect, test } from "../support/test";
import { GOAL_TEXT, PROJECT_NAME } from "../support/fixture";

// 流れ1: fixture のゴールとタスクが概要ページに表示される。
// ブラウザで実際に見える見出し・run 行・集計の件数を観測し、HTTP の成否だけでは済ませない。
test("概要ページに fixture のゴールとタスクが表示される", async ({ page, stack }) => {
  await page.goto(stack.webUrl);

  await expect(page.getByRole("heading", { name: "概要", level: 1 })).toBeVisible();
  await expect(page.getByRole("heading", { name: PROJECT_NAME, level: 3 })).toBeVisible();

  // 実行中タスクの name はゴールの objective と同じ識別文字列にしてある。
  await expect(page.getByText(`タスク「${GOAL_TEXT}」`, { exact: true })).toBeVisible();

  // ゴールが awaiting_merge で人のマージ待ちに射影され、集計の件数へ反映される。
  const mergeCount = page
    .getByRole("region", { name: "概要の集計" })
    .getByRole("listitem")
    .filter({ hasText: "人のマージ待ち" });
  await expect(mergeCount).toContainText("1件");
});

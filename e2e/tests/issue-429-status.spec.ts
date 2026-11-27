import { test, expect } from "../support/test";
import { PROJECT_NAME } from "../support/fixture";

// issue #429（docs/design/issue-429.md）の確かめ。
// 存在しない run・ゴール・タスクの詳細 URL が HTTP 200 ではなく 404 を返すことを固定する。
// いまの本番はルートの loading.tsx が存在チェックより先に fallback を流すため 200 になる（red）。
const targets = [
  "/runs/no-such-run-429",
  "/goals/00000000-0000-4000-8000-000000004290",
  `/tasks/${encodeURIComponent(PROJECT_NAME)}?task=no-such-task-429`,
];

test("存在しない run・ゴール・タスクの詳細は 404 を返し、読み込み中の文言を出さない", async ({ page, stack }) => {
  for (const path of targets) {
    const response = await page.goto(`${stack.webUrl}${path}`);
    expect(response?.status(), path).toBe(404);
    await expect(page.getByRole("heading", { name: "ページが見つかりません" })).toBeVisible();
    await expect(page.getByRole("link", { name: "概要に戻る" })).toBeVisible();
    await expect(page.getByText("読み込んでいます…", { exact: true })).toHaveCount(0);
  }
});

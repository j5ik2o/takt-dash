import { expect, test } from "../support/test";
import { GOAL_TEXT, PROJECT_NAME, RUN_SLUG } from "../support/fixture";

// 流れ5: run 詳細と会話ページへ直接リンクで入れる。概要の導線から遷移でき、
// 同じ run 詳細を URL から直接開いても 500 にならず fixture の内容が出る。
test("run 詳細と会話ページへ直接リンクで入れる", async ({ page, isolatedStack: stack }) => {
  await page.goto(stack.webUrl);
  await expect(page.getByRole("heading", { name: "概要", level: 1 })).toBeVisible();

  await page.getByRole("link", { name: "run の詳細を見る" }).click();
  await expect(page).toHaveURL(`${stack.webUrl}/runs/${RUN_SLUG}`);
  await expect(page.getByRole("heading", { name: GOAL_TEXT, level: 1 })).toBeVisible();
  await expect(page.getByRole("region", { name: "run の要約" }).getByText("今のステップ")).toBeVisible();
  await expect(page.getByRole("region", { name: "run の要約" }).getByText("implement", { exact: true })).toBeVisible();

  const runResponse = await page.goto(`${stack.webUrl}/runs/${RUN_SLUG}`);
  expect(runResponse?.status()).toBe(200);
  await expect(page.getByRole("heading", { name: GOAL_TEXT, level: 1 })).toBeVisible();
  await expect(page.getByRole("region", { name: "run の要約" }).getByText("implement", { exact: true })).toBeVisible();

  const conversationResponse = await page.goto(`${stack.webUrl}/conversations`);
  expect(conversationResponse?.status()).toBe(200);
  await expect(page.getByRole("heading", { name: "会話", level: 1 })).toBeVisible();
  await expect(page.getByRole("combobox", { name: "プロジェクト" })).toHaveValue(PROJECT_NAME);
  await expect(
    page.getByRole("status").filter({ hasText: "会話を表示できません: 会話の保存先のディレクトリが見つかりません" }),
  ).toBeVisible();
});

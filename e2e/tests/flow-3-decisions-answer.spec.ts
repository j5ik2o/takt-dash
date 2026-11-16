import { expect, test } from "../support/test";
import { GOAL_ID, QUESTION_BODY, QUESTION_ID } from "../support/fixture";

const ANSWER_TEXT = "E2E から入力した回答本文";

// 流れ3: 判断待ちページで自由記述の質問へ回答すると、モックへ正しい引数で1件届き、
// 画面に操作結果が出る。ツール名だけの確認では済ませず、引数全体を JSON として照合する。
test("判断待ちの回答がモックへ正しい引数で届き結果が表示される", async ({ page, isolatedStack: stack }) => {
  await page.goto(`${stack.webUrl}/decisions`);

  const decisionItem = page
    .getByRole("region", { name: "判断待ちの一覧" })
    .getByRole("listitem")
    .filter({ hasText: QUESTION_BODY });
  await expect(decisionItem).toBeVisible();

  await decisionItem.getByRole("textbox").fill(ANSWER_TEXT);
  await decisionItem.getByRole("button", { name: /回答する/ }).click();

  await expect(page.getByRole("status").filter({ hasText: "回答を送信しました。" })).toBeVisible();

  await expect
    .poll(
      async () => (await stack.readCalls()).filter((call) => call.name === "takt_answer_goal_question").length,
      { timeout: 15_000 },
    )
    .toBe(1);

  const calls = (await stack.readCalls()).filter((call) => call.name === "takt_answer_goal_question");
  expect(calls).toHaveLength(1);
  expect(calls[0]?.arguments).toEqual({
    cwd: stack.projectDir,
    goalId: GOAL_ID,
    questionId: QUESTION_ID,
    answer: { text: ANSWER_TEXT },
  });
});

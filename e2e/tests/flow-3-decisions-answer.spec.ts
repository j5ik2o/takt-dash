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
  const writeRequest = page.waitForRequest((candidate) => candidate.url().endsWith("/api/decisions/answer"));
  await decisionItem.getByRole("button", { name: /回答する/ }).click();

  // 書き込みの境界は Origin の無い要求を拒否する。画面からの正規の書き込みは、
  // ブラウザが開いている URL の Origin を付けて送る。
  const writeHeaders = await (await writeRequest).allHeaders();
  expect(writeHeaders["origin"]).toBe(stack.webUrl);

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

// LAN の IP で開いた画面からも、許可ホストの設定なしで回答できる。機械の IP が変わっても、
// 開いている URL の Origin と Host が一致していれば書き込みは通る。
test("LAN の IP で開いた画面から回答できる", async ({ page, isolatedStack: stack }, testInfo) => {
  if (stack.lanWebUrl === undefined) {
    testInfo.annotations.push({ type: "note", description: "ループバック以外のアドレスが無く、LAN の IP では確かめていない" });
    return;
  }
  await page.goto(`${stack.lanWebUrl}/decisions`);

  const decisionItem = page
    .getByRole("region", { name: "判断待ちの一覧" })
    .getByRole("listitem")
    .filter({ hasText: QUESTION_BODY });
  await expect(decisionItem).toBeVisible();

  await decisionItem.getByRole("textbox").fill(ANSWER_TEXT);
  const writeRequest = page.waitForRequest((candidate) => candidate.url().endsWith("/api/decisions/answer"));
  await decisionItem.getByRole("button", { name: /回答する/ }).click();

  expect((await (await writeRequest).allHeaders())["origin"]).toBe(stack.lanWebUrl);
  await expect(page.getByRole("status").filter({ hasText: "回答を送信しました。" })).toBeVisible();
  await expect
    .poll(
      async () => (await stack.readCalls()).filter((call) => call.name === "takt_answer_goal_question").length,
      { timeout: 15_000 },
    )
    .toBe(1);
});

import type { Stack } from "../support/stack";
import { QUESTION_BODY, QUESTION_ID } from "../support/fixture";
import { expect, test } from "../support/test";

const ANSWER_TEXT = "E2E から失敗の注入を確かめる回答本文";

const answerCallCount = async (stack: Stack): Promise<number> =>
  (await stack.readCalls()).filter((call) => call.name === "takt_answer_goal_question").length;

// 流れ8: 書き込み道具 takt_answer_goal_question を状態ファイルの isError で失敗させ、
// 判断待ちの一覧の同じ項目に失敗表示が出ることを確かめる。注入を解いて同じ項目で再試行すると、
// 同じ項目の結果が成功表示へ置き換わり、一覧と項目はそのまま保持される。
test("書き込みの失敗が項目に表示され、注入を解くと復帰する", async ({ page, isolatedStack: stack }) => {
  await page.goto(`${stack.webUrl}/decisions`);

  // レイアウトの backend 不達告知も role=alert を持つため、一覧の領域に限定して観測する。
  const tray = page.getByRole("region", { name: "判断待ちの一覧" });
  const decisionItem = tray.getByRole("listitem").filter({ hasText: QUESTION_BODY });
  await expect(decisionItem).toBeVisible();

  // プロセスは止めず、道具の応答だけを isError に置き換える。呼び出しは記録されてから失敗するため、
  // 失敗がアプリ側の事前拒否ではなくモックの isError に由来することを呼び出し件数で切り分けられる。
  await stack.updateState((state) => ({ ...state, isError: ["takt_answer_goal_question"] }));

  await decisionItem.getByRole("textbox").fill(ANSWER_TEXT);
  await decisionItem.getByRole("button", { name: /回答する/ }).click();

  const failure = tray.getByRole("alert").filter({ hasText: QUESTION_ID });
  await expect(failure).toBeVisible({ timeout: 15_000 });
  await expect(failure).toContainText(/takt が操作を受け付けませんでした。|backend に接続できません。/);
  await expect(decisionItem).toBeVisible();
  await expect.poll(async () => answerCallCount(stack), { timeout: 15_000 }).toBe(1);

  await stack.updateState((state) => ({ ...state, isError: [] }));

  await decisionItem.getByRole("button", { name: /回答する/ }).click();

  await expect(tray.getByRole("status").filter({ hasText: "回答を送信しました。" })).toBeVisible({ timeout: 15_000 });
  await expect(tray.getByRole("alert").filter({ hasText: QUESTION_ID })).toHaveCount(0);
  await expect.poll(async () => answerCallCount(stack), { timeout: 15_000 }).toBe(2);
});

import type { Page } from "@playwright/test";
import { expect, test } from "../support/test";
import type { Stack } from "../support/stack";
import { GOAL_ID } from "../support/fixture";

// issue #308: 作業の依頼の2択（実施した・対応できない）は、押すと確認ダイアログを開き、
// 確定したときだけ takt へ書き込みが届く。やめたら届かない。文言の案は docs/design/issue-308.md。
const OPERATION_ID = "e2e-operation-1";
// 確認ダイアログが開いてから、確定を受け付けるまでの時間（docs/design/issue-308.md）。開いた直後の確定は
// 送らないので、確定を押す前にこの時間だけ待つ。
const CONFIRM_DELAY_MS = 500;
const OPERATION_BODY = "E2E 判断待ちの作業の依頼の本文";

const answerCalls = async (stack: Stack) =>
  (await stack.readCalls()).filter((call) => call.name === "takt_answer_goal_question");

// 既定のゴールに、未回答の作業の依頼を1件足して、判断待ちのページを開く。
const openOperation = async (page: Page, stack: Stack) => {
  await stack.updateState((state) => {
    const goalQuestions = (state.goalQuestions as Record<string, unknown[]>) ?? {};
    return {
      ...state,
      goalQuestions: {
        ...goalQuestions,
        [GOAL_ID]: [
          ...(goalQuestions[GOAL_ID] ?? []),
          {
            id: OPERATION_ID,
            status: "pending",
            kind: "operation_request",
            body: OPERATION_BODY,
            options: [],
            createdAt: new Date().toISOString(),
          },
        ],
      },
    };
  });
  await page.goto(`${stack.webUrl}/decisions`);
  const item = page
    .getByRole("region", { name: "判断待ちの一覧" })
    .getByRole("listitem")
    .filter({ hasText: OPERATION_BODY });
  await expect(item).toBeVisible();
  return item;
};

test("作業の依頼の「実施した」は、確認ダイアログで確定したときだけ、書き込みが1回だけ届く", async ({
  page,
  isolatedStack: stack,
}) => {
  const item = await openOperation(page, stack);

  await item.getByRole("button", { name: "実施した" }).click();
  const dialog = page.getByRole("dialog", { name: "作業の依頼への回答（実施した）" });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole("heading", { name: "作業の依頼への回答（実施した）" })).toBeFocused();
  await expect(dialog).toContainText("作業の依頼に「実施した」と回答します。よろしいですか。");
  expect(await answerCalls(stack), "押しただけでは届かない").toHaveLength(0);

  const confirm = dialog.getByRole("button", { name: "実施した" });
  await page.waitForTimeout(CONFIRM_DELAY_MS);
  await confirm.click();
  await expect(page.getByRole("status").filter({ hasText: "回答を送信しました。" })).toBeVisible();
  await expect(dialog).toBeHidden();

  await expect.poll(async () => (await answerCalls(stack)).length, { timeout: 15_000 }).toBe(1);
  const calls = await answerCalls(stack);
  expect(calls).toHaveLength(1);
  expect(calls[0]?.arguments).toEqual({
    cwd: stack.projectDir,
    goalId: GOAL_ID,
    questionId: OPERATION_ID,
    answer: { report: "completed" },
  });
});

test("作業の依頼の2択を二重に押しても、やめれば書き込みは届かず、押したボタンへフォーカスが戻る", async ({
  page,
  isolatedStack: stack,
}) => {
  const item = await openOperation(page, stack);

  const completed = item.getByRole("button", { name: "実施した" });
  await completed.dblclick();
  const completedDialog = page.getByRole("dialog", { name: "作業の依頼への回答（実施した）" });
  await expect(completedDialog).toBeVisible();
  await completedDialog.getByRole("button", { name: "やめる" }).click();
  await expect(completedDialog).toBeHidden();
  await expect(completed).toBeFocused();

  const unable = item.getByRole("button", { name: "対応できない" });
  await unable.click();
  const unableDialog = page.getByRole("dialog", { name: "作業の依頼への回答（対応できない）" });
  await expect(unableDialog).toBeVisible();
  await expect(unableDialog).toContainText("作業の依頼に「対応できない」と回答します。よろしいですか。");
  await page.keyboard.press("Escape");
  await expect(unableDialog).toBeHidden();
  await expect(unable).toBeFocused();
  expect(await answerCalls(stack), "やめた確認は届かない").toHaveLength(0);

  // やめた2回の後に「対応できない」を確定する。届くのはこの1件だけで、やめた「実施した」は
  // 後から届かない（書き込みは押した順に出るので、先に出ていればここで2件になる）。
  await unable.click();
  await page.waitForTimeout(CONFIRM_DELAY_MS);
  await unableDialog.getByRole("button", { name: "対応できない" }).click();
  await expect(page.getByRole("status").filter({ hasText: "回答を送信しました。" })).toBeVisible();
  await expect.poll(async () => (await answerCalls(stack)).length, { timeout: 15_000 }).toBe(1);
  const calls = await answerCalls(stack);
  expect(calls).toHaveLength(1);
  expect(calls[0]?.arguments).toMatchObject({ questionId: OPERATION_ID, answer: { report: "unable" } });
});

// 一覧の2択を2回すばやく押すと、1回目で確認ダイアログが開き、2回目のクリックが、同じ座標に出た
// 確定のボタンに当たることがある（Devin Review、PR #404）。重なる配置を画面の大きさに頼らずに作るため、
// 1回目のクリックの直後、開いたダイアログの確定へ2回目のクリック（detail = 2）をページの中から送る。
// 開いた直後の確定は送らず、ダイアログは開いたまま残り、待ってから確定すれば1回だけ届く。
test("作業の依頼の2択を二重にすばやく押して、2回目が確定に当たっても、確認の前には書き込みが届かない", async ({
  page,
  isolatedStack: stack,
}) => {
  const item = await openOperation(page, stack);
  const unable = item.getByRole("button", { name: "対応できない" });
  const dialog = page.getByRole("dialog", { name: "作業の依頼への回答（対応できない）" });

  const elapsed = await unable.evaluate(async (trigger) => {
    const started = performance.now();
    trigger.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, detail: 1 }));
    const confirmButton = (): HTMLButtonElement | undefined =>
      Array.from(document.querySelectorAll<HTMLButtonElement>('[role="dialog"] button')).find(
        (button) => button.textContent !== "やめる",
      );
    while (confirmButton() === undefined && performance.now() - started < 200) {
      await new Promise((resolve) => setTimeout(resolve, 0));
    }
    const confirm = confirmButton();
    if (confirm === undefined) {
      throw new Error("1回目のクリックで確認ダイアログが開かない");
    }
    confirm.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, detail: 2 }));
    return performance.now() - started;
  });
  expect(elapsed, "2回目のクリックは、開いた直後（確定を受け付けるまでの時間の中）に当たる").toBeLessThan(
    CONFIRM_DELAY_MS,
  );

  await expect(dialog, "開いた直後の確定ではダイアログを閉じない").toBeVisible();
  // 書き込みは backend を経て届くので、届くなら届くだけの時間を待ってから数える。
  await page.waitForTimeout(2_000);
  expect(await answerCalls(stack), "二重クリックの2回目では届かない").toHaveLength(0);
  await expect(page.getByRole("status").filter({ hasText: "回答を送信しました。" })).toHaveCount(0);

  await dialog.getByRole("button", { name: "対応できない" }).click();
  await expect(page.getByRole("status").filter({ hasText: "回答を送信しました。" })).toBeVisible();
  await expect.poll(async () => (await answerCalls(stack)).length, { timeout: 15_000 }).toBe(1);
  const calls = await answerCalls(stack);
  expect(calls).toHaveLength(1);
  expect(calls[0]?.arguments).toMatchObject({ questionId: OPERATION_ID, answer: { report: "unable" } });
});

import { test as base, expect } from "../support/test";
import { startStack, type Stack } from "../support/stack";
import { buildMockState, GOAL_ID, PULL_REQUEST_NUMBER } from "../support/fixture";

// issue #311: マージ・承認・却下の確認ダイアログを通す e2e が無く、回答だけを通していた。
// 判断待ちの一覧から3つの確認ダイアログを開き、確認の文言を確かめ、確定したら
// ダイアログが閉じて一覧へ戻り、起点のボタンへフォーカスが戻り、結果が出て、
// takt への書き込みが1回だけ届くところまでを通す。却下は、承認のあとも登録案が残る
// モックの状態に依存しないよう、承認していない初期状態の stack で通す。
const CONFIRM_DELAY_MS = 600;
const REVISION = 7;
const PROPOSAL_BODY = "E2E 確認ダイアログを通すゴール登録案の本文";

// 登録案が保留中の状態でスタックを起動する。承認・却下のどちらも、この初期状態から始める。
const startDialogsStack = (): Promise<Stack> =>
  startStack({
    mockState: (now) => ({
      ...buildMockState(now),
      proposals: [{ revision: REVISION, repositoryDirectory: "/abs/e2e-corporate-site", summary: PROPOSAL_BODY }],
    }),
  });

const dialogsStack = async ({}, use: (stack: Stack) => Promise<void>): Promise<void> => {
  const stack = await startDialogsStack();
  try {
    await use(stack);
  } finally {
    await stack.stop();
  }
};

// 却下は、承認のあとも登録案が残るモックの状態に依存しないよう、承認していない stack で通す。
const rejectStack = async ({}, use: (stack: Stack) => Promise<void>): Promise<void> => {
  const stack = await startDialogsStack();
  try {
    await use(stack);
  } finally {
    await stack.stop();
  }
};

const test = base.extend<{ dialogsStack: Stack; rejectStack: Stack }>({ dialogsStack, rejectStack });

const callsOf = async (stack: Stack, name: string) =>
  (await stack.readCalls()).filter((call) => call.name === name);

test("マージと承認の確認ダイアログが開き、確定すると一覧へ戻って結果が出る", async ({
  page,
  dialogsStack: stack,
}) => {
  await page.goto(`${stack.webUrl}/decisions`);
  await expect(page.getByRole("heading", { name: "判断待ち", level: 1 })).toBeVisible();
  const list = page.getByRole("region", { name: "判断待ちの一覧" });
  const mergeItem = list.getByRole("listitem").filter({ hasText: "人のマージ待ち" });
  const registrationItem = list.getByRole("listitem").filter({ hasText: PROPOSAL_BODY });
  await expect(mergeItem).toBeVisible();
  await expect(registrationItem).toBeVisible({ timeout: 15_000 });

  // マージ: 一覧の「マージする」から確認ダイアログを開き、確定すると「マージを依頼しました。」が出る。
  const mergeTrigger = mergeItem.getByRole("button", { name: "マージする" });
  await mergeTrigger.click();
  const mergeDialog = page.getByRole("dialog", { name: "プルリクエストのマージ" });
  await expect(mergeDialog).toBeVisible();
  await expect(mergeDialog.getByRole("heading", { name: "プルリクエストのマージ" })).toBeFocused();
  await expect(mergeDialog).toContainText(
    `プルリクエスト #${PULL_REQUEST_NUMBER} をマージします。よろしいですか。`,
  );
  expect(await callsOf(stack, "takt_merge_goal_pr"), "開いただけでは届かない").toHaveLength(0);
  await page.waitForTimeout(CONFIRM_DELAY_MS);
  await mergeDialog.getByRole("button", { name: "マージする" }).click();
  await expect(page.getByRole("status").filter({ hasText: "マージを依頼しました。" })).toBeVisible();
  await expect(mergeDialog).toBeHidden();
  await expect(page.getByRole("heading", { name: "判断待ち", level: 1 })).toBeVisible();
  await expect(mergeTrigger, "確定したあと一覧の操作へ戻る").toBeVisible();
  await expect(mergeTrigger, "確定したあと起点のボタンへフォーカスが戻る").toBeFocused();
  await expect.poll(async () => (await callsOf(stack, "takt_merge_goal_pr")).length, { timeout: 15_000 }).toBe(1);
  expect((await callsOf(stack, "takt_merge_goal_pr"))[0]?.arguments).toMatchObject({
    cwd: stack.projectDir,
    goalId: GOAL_ID,
    prNumber: PULL_REQUEST_NUMBER,
    until: "merge",
  });

  // 承認: 一覧の「ゴール登録を承認する」から確認ダイアログを開き、確定すると「ゴール登録を承認しました。」が出る。
  const approveTrigger = registrationItem.getByRole("button", { name: "ゴール登録を承認する" });
  await approveTrigger.click();
  const approveDialog = page.getByRole("dialog", { name: "ゴール登録の承認" });
  await expect(approveDialog).toBeVisible();
  await expect(approveDialog.getByRole("heading", { name: "ゴール登録の承認" })).toBeFocused();
  await expect(approveDialog).toContainText(
    `revision ${REVISION} のゴール登録を承認します。よろしいですか。`,
  );
  expect(await callsOf(stack, "takt_approve_goal_registration"), "開いただけでは届かない").toHaveLength(0);
  await page.waitForTimeout(CONFIRM_DELAY_MS);
  await approveDialog.getByRole("button", { name: "ゴール登録を承認する" }).click();
  await expect(page.getByRole("status").filter({ hasText: "ゴール登録を承認しました。" })).toBeVisible();
  await expect(approveDialog).toBeHidden();
  await expect(approveTrigger, "確定したあと一覧の操作へ戻る").toBeVisible();
  await expect(approveTrigger, "確定したあと起点のボタンへフォーカスが戻る").toBeFocused();
  await expect
    .poll(async () => (await callsOf(stack, "takt_approve_goal_registration")).length, { timeout: 15_000 })
    .toBe(1);
  expect((await callsOf(stack, "takt_approve_goal_registration"))[0]?.arguments).toMatchObject({
    cwd: stack.projectDir,
    revision: REVISION,
  });
});

test("却下の確認ダイアログが開き、確定すると一覧へ戻って結果が出る", async ({ page, rejectStack: stack }) => {
  await page.goto(`${stack.webUrl}/decisions`);
  await expect(page.getByRole("heading", { name: "判断待ち", level: 1 })).toBeVisible();
  const list = page.getByRole("region", { name: "判断待ちの一覧" });
  const registrationItem = list.getByRole("listitem").filter({ hasText: PROPOSAL_BODY });
  await expect(registrationItem).toBeVisible({ timeout: 15_000 });

  // 却下: 承認していない初期状態から、一覧の「ゴール登録を却下する」で確認ダイアログを開き、
  // 確定すると「ゴール登録を却下しました。」が出る。
  const rejectTrigger = registrationItem.getByRole("button", { name: "ゴール登録を却下する" });
  await rejectTrigger.click();
  const rejectDialog = page.getByRole("dialog", { name: "ゴール登録の却下" });
  await expect(rejectDialog).toBeVisible();
  await expect(rejectDialog.getByRole("heading", { name: "ゴール登録の却下" })).toBeFocused();
  await expect(rejectDialog).toContainText(
    `revision ${REVISION} のゴール登録を却下します。よろしいですか。`,
  );
  expect(await callsOf(stack, "takt_reject_goal_registration"), "開いただけでは届かない").toHaveLength(0);
  await page.waitForTimeout(CONFIRM_DELAY_MS);
  await rejectDialog.getByRole("button", { name: "ゴール登録を却下する" }).click();
  await expect(page.getByRole("status").filter({ hasText: "ゴール登録を却下しました。" })).toBeVisible();
  await expect(rejectDialog).toBeHidden();
  await expect(rejectTrigger, "確定したあと一覧の操作へ戻る").toBeVisible();
  await expect(rejectTrigger, "確定したあと起点のボタンへフォーカスが戻る").toBeFocused();
  await expect
    .poll(async () => (await callsOf(stack, "takt_reject_goal_registration")).length, { timeout: 15_000 })
    .toBe(1);
  expect((await callsOf(stack, "takt_reject_goal_registration"))[0]?.arguments).toMatchObject({
    cwd: stack.projectDir,
    revision: REVISION,
  });
});

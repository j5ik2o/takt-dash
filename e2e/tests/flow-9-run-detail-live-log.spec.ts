import type { Page } from "@playwright/test";
import { expect, test } from "../support/test";
import { GOAL_ID, GOAL_TEXT, RUN_SLUG } from "../support/fixture";

const UPDATES_PATH = `/api/runs/${RUN_SLUG}/updates`;
const RUN_PATH = `/runs/${RUN_SLUG}`;

// ページが受け取る追記分の応答を、指定した件数だけ待つ。
const waitForUpdates = async (page: Page, count: number): Promise<void> => {
  for (let index = 0; index < count; index += 1) {
    await page.waitForResponse((response) => new URL(response.url()).pathname === UPDATES_PATH);
  }
};

const stepComplete = (step: string, timestamp: string): Record<string, unknown> => ({
  type: "step_complete",
  step,
  persona: "coder",
  status: "done",
  iteration: 1,
  timestamp,
});

// 流れ9: run 詳細を開いたまま、fixture の本体ログへ行を追記すると、再読み込みなしで3秒以内に
// ログの行が増える。追記はスタックの土台（appendRunLog）で行い、spec の中でファイルを直接作らない。
// 同じ文書のままであることは、追記の前に置いた印が追記の後も残っていることで確かめる。
test("run 詳細を開いたまま本体ログへ追記すると、再読み込みなしで3秒以内に行が増える", async ({
  page,
  isolatedStack: stack,
}) => {
  await page.goto(`${stack.webUrl}/runs/${RUN_SLUG}`);
  await expect(page.getByRole("heading", { name: GOAL_TEXT, level: 1 })).toBeVisible();

  const rows = page.getByRole("list", { name: "ログの行" }).getByRole("listitem");
  await expect(rows.last()).toContainText("step_start implement");
  const before = await rows.count();
  await page.evaluate(() => {
    (window as unknown as { __e2eSameDocument?: number }).__e2eSameDocument = 1;
  });

  const appendedAt = new Date().toISOString();
  await stack.appendRunLog([
    { type: "step_complete", step: "implement", persona: "coder", status: "done", iteration: 1, timestamp: appendedAt },
  ]);

  await expect(rows).toHaveCount(before + 1, { timeout: 3_000 });
  await expect(rows.last()).toHaveText(`${appendedAt} step_complete implement`);
  const sameDocument = await page.evaluate(
    () => (window as unknown as { __e2eSameDocument?: number }).__e2eSameDocument,
  );
  expect(sameDocument, "ページを再読み込みせずに行が増える").toBe(1);
});

// 開いている間に積んだ行は、画面の状態の自動更新（WatchState を受けた router.refresh で run 詳細を
// 取り直すこと）があっても減らない。refresh の後も前回の続きから足し、行を末尾20行へ置き換えない。
test("run 詳細で積んだ行は、画面の自動更新の後も保ったまま続きを足す", async ({ page, isolatedStack: stack }) => {
  // 最初のポーリングは位置を持たずに全体（末尾20行）を受け取る。その応答を受けてから積み始め、
  // 以後の行が前回の続きとして足されるようにする。
  const firstUpdates = waitForUpdates(page, 1);
  await page.goto(`${stack.webUrl}${RUN_PATH}`);
  await expect(page.getByRole("heading", { name: GOAL_TEXT, level: 1 })).toBeVisible();
  await firstUpdates;
  const rows = page.getByRole("list", { name: "ログの行" }).getByRole("listitem");
  await expect(rows).toHaveCount(2);
  await page.evaluate(() => {
    (window as unknown as { __e2eSameDocument?: number }).__e2eSameDocument = 1;
  });

  // GetRunDetail の log_tail（末尾20行）より多い行を、開いている間に積む。
  const base = Date.now();
  await stack.appendRunLog(
    Array.from({ length: 25 }, (_, index) => stepComplete(`step-${index + 1}`, new Date(base + index).toISOString())),
  );
  await expect(rows).toHaveCount(27, { timeout: 5_000 });

  // 状態を変え、WatchState を受けた画面に run 詳細を取り直させる。
  const refreshed = page.waitForResponse(
    (response) => response.request().headers()["rsc"] === "1" && new URL(response.url()).pathname === RUN_PATH,
    { timeout: 15_000 },
  );
  await stack.updateState((state) => {
    const goals = state.goals as Array<Record<string, unknown>>;
    return {
      ...state,
      goals: goals.map((goal) => (goal.id === GOAL_ID ? { ...goal, status: "completed" } : goal)),
    };
  });
  await refreshed;
  // 取り直した後のポーリングの応答を2回受け取り、その応答が画面へ反映された状態で数える。
  await waitForUpdates(page, 2);
  await expect(rows).toHaveCount(27);

  const appendedAt = new Date(base + 1_000).toISOString();
  await stack.appendRunLog([stepComplete("after-refresh", appendedAt)]);
  await expect(rows.last()).toHaveText(`${appendedAt} step_complete after-refresh`, { timeout: 3_000 });
  await expect(rows).toHaveCount(28);
  const sameDocument = await page.evaluate(
    () => (window as unknown as { __e2eSameDocument?: number }).__e2eSameDocument,
  );
  expect(sameDocument, "ページを再読み込みせずに行を保つ").toBe(1);
});

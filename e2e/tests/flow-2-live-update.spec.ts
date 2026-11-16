import { expect, test } from "../support/test";
import { GOAL_ID } from "../support/fixture";

// 流れ2: モックの状態変化が、ページを再読み込みせずに概要へ反映される。
// 同じブラウザページのまま「人のマージ待ち」1件→0件を観測し、初期値の固定表示や
// reload による再取得では通らないようにする。
test("状態ファイルの変更が再読み込みなしで概要へ反映される", async ({ page, isolatedStack: stack }) => {
  await page.goto(stack.webUrl);

  const mergeCount = page
    .getByRole("region", { name: "概要の集計" })
    .getByRole("listitem")
    .filter({ hasText: "人のマージ待ち" });
  await expect(mergeCount.getByText(/^1件$/)).toBeVisible();

  await stack.updateState((state) => {
    const goals = state.goals as Array<Record<string, unknown>>;
    return {
      ...state,
      goals: goals.map((goal) => (goal.id === GOAL_ID ? { ...goal, status: "completed" } : goal)),
    };
  });

  // reload も goto もせず、同じページの表示がポーリング由来で 0件へ変わることを待つ。
  await expect(mergeCount.getByText(/^0件$/)).toBeVisible({ timeout: 15_000 });
});

import { SERVER_NAME } from "../support/fixture";
import { expect, test } from "../support/test";

// 流れ7: モックの応答を外側で遅らせ、backend の呼び出し期限の超過を「応答なし」として観測する。
// 流れ6a のプロセス停止とは違い、遅延を解いた次のサイクルで同じページが「動作中」へ戻ることを、
// 再読み込みせずに前→変化→後の一続きとして確かめる。
test("応答を遅らせると応答なしになり、遅延を解くと動作中へ戻る", async ({ page, isolatedStack: stack }) => {
  await page.goto(stack.webUrl);

  const serverRow = page
    .getByRole("navigation", { name: "画面と集約対象のサーバー" })
    .getByRole("listitem")
    .filter({ hasText: SERVER_NAME });
  await expect(serverRow).toContainText("動作中", { timeout: 15_000 });

  // backend の takt-mcp 呼び出し期限は 15 秒（backend/src/mcp/tools.rs の request_timeout）。
  // それを超える 18 秒を遅らせ、停止ではなく期限超過で応答なしになる状況を作る。
  await stack.setMockDelay(18_000);
  await expect(serverRow).toContainText("応答なし", { timeout: 30_000 });

  // 遅延を解くと次の読み取りサイクルが成功し、同じサーバー行が動作中へ戻る。
  await stack.setMockDelay(0);
  await expect(serverRow).toContainText("動作中", { timeout: 30_000 });
});

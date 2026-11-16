import { expect, test } from "../support/test";
import { SERVER_NAME } from "../support/fixture";

// 流れ6a: モック takt-mcp を pid ファイルの pid で止め、同じページのサイドバーが
// サーバーの応答なしを示すことを待つ。backend は次のサイクルでモックを起動し直すため、
// 応答なしを観測する間は pid を読み直して繰り返し止め、再起動後も読み取りを失敗させる。
test("モックを止めるとサイドバーが応答なしになる", async ({ page, isolatedStack: stack }) => {
  await page.goto(stack.webUrl);

  const serverRow = page
    .getByRole("navigation", { name: "画面と集約対象のサーバー" })
    .getByRole("listitem")
    .filter({ hasText: SERVER_NAME });
  await expect(serverRow).toContainText("動作中", { timeout: 15_000 });

  await stack.stopMock();
  await stack.updateState((state) => ({ ...state, transportErrors: ["takt_list_goals"] }));

  await expect
    .poll(
      async () => {
        await stack.stopMock();
        return (await serverRow.textContent()) ?? "";
      },
      { timeout: 30_000 },
    )
    .toContain("応答なし");
});

// 流れ6b: takt-dash backend を止め、同じページに接続できない旨の告知が出ることを待つ。
// backend を止めた後に teardown が二重停止で失敗しないことも、isolatedStack の stop で確かめる。
test("backend を止めると接続できない告知が出る", async ({ page, isolatedStack: stack }) => {
  // Next の route announcer も role=alert を持つため、告知は文言で絞り込む。
  // backend を止めるとレイアウトと概要の両方が同じ告知を出すため、先頭だけを見る。
  const notice = page.getByRole("alert").filter({ hasText: "backend に接続できません。" }).first();

  await page.goto(stack.webUrl);
  await expect(page.getByRole("heading", { name: "概要", level: 1 })).toBeVisible();
  await expect(notice).toHaveCount(0);

  await stack.stopBackend();

  await expect(notice).toBeVisible({ timeout: 30_000 });
});

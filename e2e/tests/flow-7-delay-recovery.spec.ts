import { SERVER_NAME } from "../support/fixture";
import { expect, test } from "../support/test";

// 流れ7: モックの応答を外側で遅らせても、backend の読み取りが成功し続けることを確かめる。
// 一覧読み取りの期限を60秒へ広げた緊急対策（2026-11-17）で、かつての期限（15秒）を
// 超える遅延は「遅いが生きている」として扱い、「応答なし」にはしない。遅延を解いても
// 同じページが「動作中」のままであることを、前→変化→後の一続きとして確かめる。
// 期限超過そのもので「応答なし」になる観測は、プロセス停止を扱う流れ6が担う。
test("応答を遅らせても応答なしにならず、遅延を解いても動作中のまま", async ({ page, isolatedStack: stack }) => {
  await page.goto(stack.webUrl);

  const serverRow = page
    .getByRole("navigation", { name: "画面と集約対象のサーバー" })
    .getByRole("listitem")
    .filter({ hasText: SERVER_NAME });
  await expect(serverRow).toContainText("動作中", { timeout: 15_000 });

  // backend の一覧読み取り（takt_list_goals・takt_list_tasks）の期限は60秒
  // （backend/src/mcp/tools/mod.rs の LIST_READ_TIMEOUT）。かつての期限（15秒）を
  // 超える18秒を遅らせても、読み取りは成功し、応答なしにはならない。
  // 旧来の作りでは最初の読み取りが約15秒で期限切れになり、失敗1回（このスタックの
  // 閾値）で応答なしへ移っていたため、45秒待っても応答なしへ移らないことをもって
  // 新しい作りを確認する。
  await stack.setMockDelay(18_000);
  await page.waitForTimeout(45_000);
  await expect(serverRow).toContainText("動作中");
  await expect(serverRow).not.toContainText("応答なし");

  // 遅延を解いても、同じサーバー行は動作中のままである。
  await stack.setMockDelay(0);
  await expect(serverRow).toContainText("動作中", { timeout: 30_000 });
});

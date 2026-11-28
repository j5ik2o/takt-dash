import { expect, test } from "../support/test";

// issue #329 の通常幅・列幅の実測。1280px の重なりは issue-317-layout.spec.ts で
// boundingBox() を測っている。ここでは 1280px より広い通常幅（ほかの e2e と同じ 1920px）で、
// 種類の文字そのものが列の中に収まっているかを、要素の枠ではなく種類の要素の
// scrollWidth と clientWidth で測る。文字が列より広ければ scrollWidth の方が大きくなる。
//
// この見方は、2列目（種類）を文字より狭い固定幅にすると失敗する。列が中身に合わせて
// 伸縮する指定（auto など）であることを前提にした断言にしている。
const WIDE = { width: 1920, height: 1080 };

test("1920px でアクティビティの種類の文字が列の中に収まる", async ({ page, isolatedStack: stack }, testInfo) => {
  // 種類の列には長い種類名（blocked 通知）と短い種類名（問い合わせ）を同じ一覧に並べる。
  // 長い種類名は blocked 通知で作るため、状態ファイルへ blocked 通知と作業の依頼（問い合わせ）を足す。
  const goalId = "11111111-1111-4111-8111-111111111111";
  await stack.updateState((state) => {
    const goalQuestions = (state.goalQuestions as Record<string, unknown[]>) ?? {};
    const existing = goalQuestions[goalId] ?? [];
    const notifications = (state.notifications as Record<string, unknown[]>) ?? {};
    const existingNotifications = notifications[goalId] ?? [];
    const at = new Date().toISOString();
    return {
      ...state,
      goalQuestions: {
        ...goalQuestions,
        [goalId]: [
          ...existing,
          {
            id: "e2e-inquiry-blocked",
            status: "pending",
            kind: "operation_request",
            body: "blocked 通知を確認するための作業依頼です。",
            options: [],
            createdAt: at,
          },
        ],
      },
      notifications: {
        ...notifications,
        [goalId]: [
          ...existingNotifications,
          {
            id: "e2e-blocked-notification",
            kind: "blocked",
            body: "blocked 通知を確認するための通知です。",
            recordedAt: at,
          },
        ],
      },
    };
  });

  await page.setViewportSize(WIDE);
  await page.goto(`${stack.webUrl}/activity`);

  // 可視のシェル配下に限定する。ストリーミングの複製（body > div[hidden]）の行は
  // display:none で測れないため、対象にしない。
  const rows = page.locator(".shell ul.events > li.ev");
  await expect(rows.first()).toBeVisible();
  // 足した問い合わせと blocked 通知が届き、起動時の行と合わせて3行になるまで待つ。
  // 先頭の1行だけを測って通ることを防ぐため、母数を固定する。
  await expect(rows).toHaveCount(3);
  // 長い種類名が実際に出ていることを確かめる（この列幅の検証の前提）。
  await expect(rows.locator(".kl").filter({ hasText: "blocked 通知" })).toHaveCount(1);

  const count = await rows.count();
  expect(count).toBeGreaterThan(0);

  // 種類の要素の文字が、その要素の内側の幅に収まっていること。長い種類名でも
  // scrollWidth が clientWidth を超えない（超えるなら文字が列からはみ出している）。
  // あわせて、種類の箱そのものが隣の本文（.msg）へ広がっていないことを、行の中の
  // getBoundingClientRect の右端と左端で測る。内側の scrollWidth だけでは、箱が
  // 本文へ重なっても通ってしまうため。右端と左端が同じ位置で接するのは許す。
  for (let index = 0; index < count; index += 1) {
    const kind = rows.nth(index).locator(".kl");
    const metrics = await kind.evaluate((element) => {
      const row = element.closest("li.ev");
      const message = row?.querySelector(".msg");
      if (!message) {
        throw new Error("種類の行に本文（.msg）が見つからない");
      }
      const kindRect = element.getBoundingClientRect();
      const messageRect = message.getBoundingClientRect();
      return {
        text: element.textContent ?? "",
        scrollWidth: element.scrollWidth,
        clientWidth: element.clientWidth,
        kindRight: kindRect.right,
        messageLeft: messageRect.left,
      };
    });
    testInfo.attach(`kind-column-${index}`, {
      body: JSON.stringify(metrics),
      contentType: "application/json",
    });
    expect(
      metrics.scrollWidth,
      `種類「${metrics.text}」の文字幅 ${metrics.scrollWidth} が列の内側の幅 ${metrics.clientWidth} を超える`,
    ).toBeLessThanOrEqual(metrics.clientWidth);
    expect(
      metrics.kindRight,
      `種類「${metrics.text}」の右端 ${metrics.kindRight} が本文の左端 ${metrics.messageLeft} を超える`,
    ).toBeLessThanOrEqual(metrics.messageLeft);
  }
});

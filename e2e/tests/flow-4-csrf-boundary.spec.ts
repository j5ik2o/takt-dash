import type { Stack } from "../support/stack";
import { expect, test } from "../support/test";
import { GOAL_ID, PROJECT_NAME, QUESTION_BODY, QUESTION_ID, SERVER_ID } from "../support/fixture";

const ANSWER_TEXT = "CSRF 境界の対照で送る回答本文";

const answerCallCount = async (stack: Stack): Promise<number> =>
  (await stack.readCalls()).filter((call) => call.name === "takt_answer_goal_question").length;

// 流れ4: CSRF の前提を欠く書き込みが、画面を経ずに組み上げた要求でも境界で拒否される。
// 拒否が境界検証によることを、同一オリジン・application/json の対照と切り分ける。
test("判断待ちへの書き込みが content-type と origin の境界で拒否される", async ({
  page,
  request,
  isolatedStack: stack,
}) => {
  await page.goto(`${stack.webUrl}/decisions`);
  await expect(
    page.getByRole("region", { name: "判断待ちの一覧" }).getByText(QUESTION_BODY, { exact: true }),
  ).toBeVisible();

  const url = `${stack.webUrl}/api/decisions/answer`;
  const body = JSON.stringify({
    serverId: SERVER_ID,
    project: PROJECT_NAME,
    inquiryId: QUESTION_ID,
    goalId: GOAL_ID,
    text: ANSWER_TEXT,
  });
  const before = await answerCallCount(stack);

  const notJson = await request.post(url, {
    headers: { "content-type": "text/plain" },
    data: body,
  });
  expect(notJson.status()).toBe(415);

  const foreignOrigin = await request.post(url, {
    headers: { "content-type": "application/json", origin: "https://cross-site.example" },
    data: body,
  });
  expect(foreignOrigin.status()).toBe(403);

  const crossSite = await request.post(url, {
    headers: { "content-type": "application/json", origin: stack.webUrl, "sec-fetch-site": "cross-site" },
    data: body,
  });
  expect(crossSite.status()).toBe(403);

  // 許可ホストに無いホスト名（同じポートの localhost）は、Host と Origin が一致していても拒否する。
  const webPort = new URL(stack.webUrl).port;
  const notAllowedHost = `localhost:${webPort}`;
  const notAllowed = await request.post(`http://${notAllowedHost}/api/decisions/answer`, {
    headers: { "content-type": "application/json", origin: `http://${notAllowedHost}` },
    data: body,
  });
  expect(notAllowed.status()).toBe(403);

  // Web は 0.0.0.0 で待ち受けており、listen 名と違う 127.0.0.1 から Origin 付きで送る。
  // origin 境界を通過して入力検証まで届くことを、403 ではなく 400 で確かめる。
  const invalidInput = await request.post(url, {
    headers: { "content-type": "application/json", origin: stack.webUrl },
    data: "{}",
  });
  expect(invalidInput.status()).toBe(400);
  expect(await invalidInput.json()).toEqual({ ok: false, error: "serverId がありません。" });

  expect(await answerCallCount(stack)).toBe(before);

  const accepted = await request.post(url, {
    headers: { "content-type": "application/json", origin: stack.webUrl },
    data: body,
  });
  expect(accepted.status()).toBe(200);
  await expect.poll(async () => answerCallCount(stack), { timeout: 15_000 }).toBe(before + 1);
});

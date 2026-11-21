import { test as base, expect, type Locator } from "@playwright/test";
import { writeFile } from "node:fs/promises";
import { GOAL_ID, GOAL_TEXT, RUN_SLUG, SERVER_ID } from "../support/fixture";
import { startStack, type Stack } from "../support/stack";

const test = base.extend<{}, { taktStack: Stack }>({
  taktStack: [async ({}, use) => {
    const stack = await startStack({ projects: [{ name: "takt", about: "概要の狭幅表示を確認する" }] });
    try {
      await use(stack);
    } finally {
      await stack.stop();
    }
  }, { scope: "worker" }],
});

// DOM の存在や可視判定だけでは、折り返しと overflow による見切れを検出できない。
async function measureText(locator: Locator) {
  return locator.evaluate((element) => {
    const lines = new Map<number, string>();
    const glyphs: Array<{ left: number; right: number; top: number; bottom: number }> = [];
    const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT);
    let node = walker.nextNode();
    while (node !== null) {
      const text = node.textContent ?? "";
      for (let offset = 0; offset < text.length; offset += 1) {
        const character = text.charAt(offset);
        if (character.trim() === "") continue;
        const range = document.createRange();
        range.setStart(node, offset);
        range.setEnd(node, offset + 1);
        const rect = range.getBoundingClientRect();
        const top = Math.round(rect.top);
        lines.set(top, (lines.get(top) ?? "") + character);
        glyphs.push({ left: rect.left, right: rect.right, top: rect.top, bottom: rect.bottom });
      }
      node = walker.nextNode();
    }
    const box = element.getBoundingClientRect();
    const card = element.closest(".scard, .callout, .run-row");
    if (card === null) throw new Error("表示項目のカードが見つかりません");
    const cardBox = card.getBoundingClientRect();
    const left = Math.max(0, box.left, cardBox.left);
    const right = Math.min(window.innerWidth, box.right, cardBox.right);
    const top = Math.max(box.top, cardBox.top);
    const bottom = Math.min(box.bottom, cardBox.bottom);
    return {
      text: element.textContent,
      lines: [...lines.values()],
      box: { left: box.left, right: box.right, width: box.width },
      card: { left: cardBox.left, right: cardBox.right, width: cardBox.width },
      // CSS の小数ピクセル丸めだけを許容する。
      contained: glyphs.length > 0 && glyphs.every((rect) =>
        rect.right > rect.left && rect.bottom > rect.top &&
        rect.left >= left - 1 && rect.right <= right + 1 &&
        rect.top >= top - 1 && rect.bottom <= bottom + 1,
      ),
    };
  });
}

function expectReadable(measurement: Awaited<ReturnType<typeof measureText>>, label: string) {
  expect.soft(measurement.contained, `${label}: 全文が要素・カード内と画面の横幅に収まる`).toBe(true);
  // 最終行の端数は許容し、1〜2文字の行が連続する潰れだけを拒否する。
  const fullLines = measurement.lines.slice(0, -1);
  const collapsed = fullLines.some((line, index) =>
    line.length <= 2 && index > 0 && fullLines[index - 1].length <= 2,
  );
  expect.soft(collapsed, `${label}: 描画行 ${JSON.stringify(measurement.lines)}`).toBe(false);
}

test("同じ概要を狭めてもマージ待ち通知と実行中 run の全文を読める", async ({ page, taktStack }, testInfo) => {
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.goto(taktStack.webUrl);
  const notice = page.getByRole("region", { name: "判断待ちの項目", exact: true })
    .getByRole("listitem").filter({ has: page.getByText("人のマージ待ち", { exact: true }) });
  const run = page.locator(".run-row").filter({
    has: page.getByRole("link", { name: "run の詳細を見る", exact: true })
      .and(page.locator(`[href="/runs/${RUN_SLUG}"]`)),
  });
  await expect(notice).toHaveCount(1);
  await expect(run).toHaveCount(1);
  const project = notice.getByText("takt", { exact: true });
  const goal = run.getByText(GOAL_TEXT, { exact: true });
  const task = run.getByText(`タスク「${GOAL_TEXT}」`, { exact: true });
  const elapsed = run.getByText(/^最後のログは .+前$/);
  await expect(project).toBeVisible();
  await expect(goal).toBeVisible();
  await expect(task).toBeVisible();
  await expect(elapsed).toBeVisible();

  // 同じ document を開いたまま、カードの列数とモバイル配置の境界をまたぐ。
  for (const width of [1440, 1366, 1281, 1280, 1024, 820, 761, 760, 390, 360, 320]) {
    await page.setViewportSize({ width, height: 1000 });
    await page.evaluate(() => new Promise<void>((resolve) => requestAnimationFrame(() => resolve())));
    const measurements = {
      width,
      project: await measureText(project),
      goal: await measureText(goal),
      task: await measureText(task),
      elapsed: await measureText(elapsed),
      layout: await run.evaluate((element) => ({
        columns: getComputedStyle(element).gridTemplateColumns,
        gap: getComputedStyle(element).columnGap,
      })),
      noticeLayout: await notice.evaluate((element) => ({
        columns: getComputedStyle(element).gridTemplateColumns,
        gap: getComputedStyle(element).columnGap,
      })),
    };
    const measurementsPath = testInfo.outputPath(`overview-${width}.json`);
    const screenshotPath = testInfo.outputPath(`overview-${width}.png`);
    await writeFile(measurementsPath, JSON.stringify(measurements, null, 2));
    await page.screenshot({ path: screenshotPath, fullPage: true });
    await testInfo.attach(`overview-${width}.json`, {
      path: measurementsPath, contentType: "application/json",
    });
    await testInfo.attach(`overview-${width}.png`, {
      path: screenshotPath, contentType: "image/png",
    });
    expect.soft(measurements.project.lines, `${width}px: takt を途中で折り返さない`).toEqual(["takt"]);
    expectReadable(measurements.project, `${width}px 通知`);
    expectReadable(measurements.goal, `${width}px ゴール`);
    expectReadable(measurements.task, `${width}px 本文`);
    expectReadable(measurements.elapsed, `${width}px 経過`);
  }
});

for (const width of [1440, 820, 390, 320]) {
  test(`${width}px の概要のリンクで該当するマージ待ちと run を開ける`, async ({ page, taktStack }) => {
    await page.setViewportSize({ width, height: 1000 });
    await page.goto(taktStack.webUrl);
    const notice = page.getByRole("region", { name: "判断待ちの項目", exact: true })
      .getByRole("listitem").filter({ has: page.getByText("人のマージ待ち", { exact: true }) });
    await expect(notice.getByText("takt", { exact: true })).toBeVisible();
    await expect(page.getByText(`タスク「${GOAL_TEXT}」`, { exact: true })).toBeVisible();
    await notice.getByRole("link", { name: "内容を見る", exact: true }).click();
    const targetId = `decision-merge:${SERVER_ID}:takt:${GOAL_ID}`;
    await expect.poll(() => {
      const url = new URL(page.url());
      return url.pathname + decodeURIComponent(url.hash);
    }).toBe(`/decisions#${targetId}`);
    const target = page.getByRole("region", { name: "判断待ちの一覧", exact: true })
      .locator(`[id="${targetId}"]`);
    await expect(target).toBeVisible();
    await expect(target).toContainText("プロジェクト: takt");
    await expect(target).toContainText("種別: 人のマージ待ち");
    await expect(target).toContainText(GOAL_TEXT);

    await page.goBack();
    await page.getByRole("link", { name: "run の詳細を見る", exact: true }).click();
    await expect(page).toHaveURL(`${taktStack.webUrl}/runs/${RUN_SLUG}`);
    await expect(page.getByRole("heading", { name: GOAL_TEXT, level: 1, exact: true })).toBeVisible();
  });
}

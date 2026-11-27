import { expect, test } from "../support/test";
import { GOAL_TEXT, RUN_SLUG } from "../support/fixture";

// issue #311: run 詳細の画面のテストにモバイルの幅がない。実 Chrome の描画で 360px と 390px を開き、
// 横にはみ出さず、要約が1列に積まれ、ステップ・ログ・レポート・指示書の見出しとログの行が読めることを測る。
const HEIGHT = 844;
const WIDTHS = [360, 390] as const;

for (const width of WIDTHS) {
  test(`幅 ${width}px の run 詳細は横にはみ出さず、要約と各節の見出しが読める`, async ({ page, stack }) => {
    await page.setViewportSize({ width, height: HEIGHT });
    const response = await page.goto(`${stack.webUrl}/runs/${RUN_SLUG}`);
    expect(response?.status()).toBe(200);
    await expect(page.getByRole("heading", { name: GOAL_TEXT, level: 1 })).toBeVisible();

    const overflow = await page.evaluate(
      () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
    );
    expect(overflow, "ページが横にはみ出さない").toBeLessThanOrEqual(0);

    const heading = page.getByRole("heading", { name: GOAL_TEXT, level: 1 });
    const headingBox = await heading.boundingBox();
    expect(headingBox).not.toBeNull();
    expect(headingBox!.x, "見出しの左端が画面の中").toBeGreaterThanOrEqual(0);
    expect(headingBox!.x + headingBox!.width, "見出しの右端が画面の幅に収まる").toBeLessThanOrEqual(width + 0.5);

    // 要約は狭い幅で1列に積まれ、各項目の右端が画面の幅に収まる。
    const summary = page.getByRole("region", { name: "run の要約" });
    await expect(summary).toBeVisible();
    const columns = await summary
      .locator(".sum-grid")
      .evaluate((element) => getComputedStyle(element).gridTemplateColumns.trim().split(/\s+/).filter(Boolean));
    expect(columns, "要約は1列に積まれる").toHaveLength(1);
    const fields = summary.locator(".f");
    const fieldCount = await fields.count();
    expect(fieldCount, "要約の項目がある").toBeGreaterThan(0);
    for (let index = 0; index < fieldCount; index += 1) {
      const box = await fields.nth(index).boundingBox();
      expect(box, "要約の項目が描画されている").not.toBeNull();
      expect(box!.x, "要約の項目の左端が画面の中").toBeGreaterThanOrEqual(0);
      expect(box!.x + box!.width, "要約の項目の右端が画面の幅に収まる").toBeLessThanOrEqual(width + 0.5);
    }

    for (const name of ["ステップ", "ログの末尾", "レポート", "指示書"]) {
      await expect(page.getByRole("heading", { name, level: 2 }), `節「${name}」の見出しが見える`).toBeVisible();
    }

    const log = page.getByRole("list", { name: "ログの行" });
    await expect(log).toBeVisible();
    await expect(log.getByRole("listitem").first(), "ログの行が見える").toBeVisible();

    const back = page.locator("main.page > a.back").first();
    await expect(back).toBeVisible();
    const backBox = await back.boundingBox();
    expect(backBox).not.toBeNull();
    expect(backBox!.x, "戻るリンクの左端が画面の中").toBeGreaterThanOrEqual(0);
    expect(backBox!.x + backBox!.width, "戻るリンクの右端が画面の幅に収まる").toBeLessThanOrEqual(width + 0.5);
  });
}

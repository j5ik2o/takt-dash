import type { Locator } from "@playwright/test";
import { expect, test } from "../support/test";
import { GOAL_TEXT, RUN_SLUG } from "../support/fixture";

// issue #311: run 詳細の画面のテストにモバイルの幅がない。実 Chrome の描画で 360px と 390px を開き、
// 横にはみ出さず、要約が1列に積まれ、ステップ・ログ・レポート・指示書の見出しとログの行が読めることを測る。
const HEIGHT = 844;
const WIDTHS = [360, 390] as const;

// 右端の位置だけでは、箱が幅に収まったまま中で文字がはみ出しても落ちない。中身の幅（scrollWidth）が
// 見える幅（clientWidth）を超えないことを測る。inline の要素は両方 0 で空振りするので、幅があることも見る。
const expectContentFits = async (locator: Locator, label: string): Promise<void> => {
  const size = await locator.evaluate((element) => ({
    scrollWidth: element.scrollWidth,
    clientWidth: element.clientWidth,
  }));
  expect(size.clientWidth, `${label}が幅を持って描画されている`).toBeGreaterThan(0);
  expect(size.scrollWidth, `${label}の中身が横にはみ出さない`).toBeLessThanOrEqual(size.clientWidth);
};

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
    await expectContentFits(heading, "見出し");

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
      await expectContentFits(fields.nth(index), `要約の${index + 1}番目の項目`);
    }

    for (const name of ["ステップ", "ログの末尾", "レポート", "指示書"]) {
      await expect(page.getByRole("heading", { name, level: 2 }), `節「${name}」の見出しが見える`).toBeVisible();
    }

    const log = page.getByRole("list", { name: "ログの行" });
    await expect(log).toBeVisible();
    const firstLogLine = log.getByRole("listitem").first();
    await expect(firstLogLine, "ログの行が見える").toBeVisible();
    // body が横のはみ出しを隠すため、document の scrollWidth が 0 でも中身は切れ得る。
    // 見出しと同じく、先頭のログの行の右端が画面の幅を超えないことを測る。
    const firstLogBox = await firstLogLine.boundingBox();
    expect(firstLogBox).not.toBeNull();
    expect(firstLogBox!.x, "ログの先頭の行の左端が画面の中").toBeGreaterThanOrEqual(0);
    expect(firstLogBox!.x + firstLogBox!.width, "ログの先頭の行の右端が画面の幅に収まる").toBeLessThanOrEqual(
      width + 0.5,
    );

    // ol は横にスクロールするので、行の中身がはみ出しても行の箱は幅に収まる。一覧と各行の中身の幅を測る。
    // 行は1秒ごとの追記で増え得るので、数えてから1件ずつ辿らず、その時点の行を1回で測る。
    await expectContentFits(log, "ログの一覧");
    const logLines = await log.getByRole("listitem").evaluateAll((elements) =>
      elements.map((element) => {
        const rect = element.getBoundingClientRect();
        return {
          text: element.textContent ?? "",
          scrollWidth: element.scrollWidth,
          clientWidth: element.clientWidth,
          left: rect.left,
          right: rect.right,
        };
      }),
    );
    expect(logLines.length, "ログの行が2行以上ある").toBeGreaterThanOrEqual(2);
    for (const [index, line] of logLines.entries()) {
      const label = `ログの${index + 1}行目（${line.text.slice(0, 40)}）`;
      expect(line.clientWidth, `${label}が幅を持って描画されている`).toBeGreaterThan(0);
      expect(line.scrollWidth, `${label}の中身が横にはみ出さない`).toBeLessThanOrEqual(line.clientWidth);
      expect(line.left, `${label}の左端が画面の中`).toBeGreaterThanOrEqual(0);
      expect(line.right, `${label}の右端が画面の幅に収まる`).toBeLessThanOrEqual(width + 0.5);
    }

    const back = page.locator("main.page > a.back").first();
    await expect(back).toBeVisible();
    const backBox = await back.boundingBox();
    expect(backBox).not.toBeNull();
    expect(backBox!.x, "戻るリンクの左端が画面の中").toBeGreaterThanOrEqual(0);
    expect(backBox!.x + backBox!.width, "戻るリンクの右端が画面の幅に収まる").toBeLessThanOrEqual(width + 0.5);
    await expectContentFits(back, "戻るリンク");
  });
}

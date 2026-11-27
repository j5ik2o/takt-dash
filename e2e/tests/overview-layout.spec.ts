import type { Page } from "@playwright/test";
import { expect, test } from "../support/test";
import { SERVER_ID } from "../support/fixture";

// issue #120（統合した #149・#171・#193 を含む）の概要のレイアウトを、実 Chrome の描画で測る。
// ページ幅の上限、タイルの幅、サーバーの1行、実行中の run の全幅の表、run 行の文字の重なり、
// 横へのはみ出し、プロジェクトのカードの可変列を、boundingBox() と getClientRects() で観測する。
const HEIGHT = 900;
const WIDTHS = [1100, 1280, 1920, 2560] as const;

type Box = { x: number; y: number; width: number; height: number };

const openOverview = async (page: Page, url: string, width: number): Promise<void> => {
  await page.setViewportSize({ width, height: HEIGHT });
  await page.goto(url);
  await expect(page.getByRole("heading", { name: "概要", level: 1 })).toBeVisible();
  await expect(page.locator("#h-runs")).toBeVisible();
};

const boxOf = async (page: Page, selector: string): Promise<Box> => {
  const box = await page.locator(selector).first().boundingBox();
  expect(box, `${selector} が描画されている`).not.toBeNull();
  return box!;
};

// .content の内側（padding を除いた幅）。概要の本文が使える最大の幅。
const contentInnerWidth = (page: Page): Promise<number> =>
  page.locator(".content").evaluate((element) => {
    const style = getComputedStyle(element);
    return element.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight);
  });

// セルの中身（文字とその子要素）が実際に描かれた矩形を集める。セルの外へはみ出した文字も含める。
const cellContentRects = (page: Page, header: string): Promise<Box[]> =>
  page.locator('section[aria-labelledby="h-runs"]').evaluate((section, label) => {
    const headers = Array.from(section.querySelectorAll("thead th"), (th) => th.textContent?.trim() ?? "");
    const index = headers.indexOf(label);
    const row = section.querySelector("tbody tr.run-row");
    const cell = row?.children.item(index);
    if (index < 0 || cell === null || cell === undefined) {
      return [];
    }
    const range = document.createRange();
    range.selectNodeContents(cell);
    return Array.from(range.getClientRects())
      .filter((rect) => rect.width > 0 && rect.height > 0)
      .map((rect) => ({ x: rect.x, y: rect.y, width: rect.width, height: rect.height }));
  }, header);

const intersects = (a: Box, b: Box): boolean =>
  a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height;

for (const width of [1920, 2560]) {
  test(`幅 ${width}px で概要の本文が右に空白を残さず本文の幅いっぱいに広がる`, async ({ page, stack }) => {
    await openOverview(page, stack.webUrl, width);

    const overview = await boxOf(page, "main.overview");
    expect(Math.abs(overview.width - (await contentInnerWidth(page)))).toBeLessThanOrEqual(1);
  });
}

for (const width of WIDTHS) {
  test(`幅 ${width}px で概要のページが横にはみ出さず、実行中の run の表が本文の全幅になる`, async ({ page, stack }) => {
    await openOverview(page, stack.webUrl, width);

    const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
    expect(overflow).toBeLessThanOrEqual(0);

    const overview = await boxOf(page, "main.overview");
    const table = await boxOf(page, 'section[aria-labelledby="h-runs"] .l-table');
    expect(Math.abs(table.width - overview.width)).toBeLessThanOrEqual(1);
    await expect(page.locator('section[aria-labelledby="h-runs"] tbody tr.run-row')).toHaveCount(1);
  });
}

for (const width of [1280, 1920, 2560]) {
  test(`幅 ${width}px でサーバーは見出しの横の1行に収まる`, async ({ page, stack }) => {
    await openOverview(page, stack.webUrl, width);

    const heading = await boxOf(page, "#h-servers");
    const line = page.locator(`#server-${SERVER_ID}`);
    await expect(line).toBeVisible();
    const lineBox = (await line.boundingBox())!;
    expect(lineBox.x, "サーバーの1行は見出しの右に並ぶ").toBeGreaterThanOrEqual(heading.x + heading.width);
    const sameRow = lineBox.y < heading.y + heading.height && heading.y < lineBox.y + lineBox.height;
    expect(sameRow, "見出しと同じ高さに並ぶ").toBe(true);

    // 子要素が折り返していれば、ある子の上端が別の子の下端より下に来る。
    const children = await line.evaluate((element) =>
      Array.from(element.children, (child) => child.getBoundingClientRect()).filter((rect) => rect.height > 0),
    );
    expect(children.length).toBeGreaterThan(0);
    const lowestTop = Math.max(...children.map((rect) => rect.top));
    const highestBottom = Math.min(...children.map((rect) => rect.bottom));
    expect(lowestTop, "サーバーの1行の子要素が1行に並ぶ").toBeLessThan(highestBottom);
  });
}

for (const width of [1100, 1280, 1920]) {
  test(`幅 ${width}px で run 行のゴールの文が状態とステップの表示に重ならない`, async ({ page, stack }) => {
    await openOverview(page, stack.webUrl, width);

    const goal = await cellContentRects(page, "ゴール");
    const status = await cellContentRects(page, "状態");
    const step = await cellContentRects(page, "ステップ");
    expect(goal.length, "ゴールの列に文がある").toBeGreaterThan(0);
    expect(status.length, "状態の列に文がある").toBeGreaterThan(0);
    expect(step.length, "ステップの列に文がある").toBeGreaterThan(0);
    for (const goalRect of goal) {
      for (const other of [...status, ...step]) {
        expect(intersects(goalRect, other)).toBe(false);
      }
    }
  });
}

test("数字1個のタイルは 220px 以下の幅で左に詰めて並ぶ", async ({ page, stack }) => {
  await openOverview(page, stack.webUrl, 1920);

  const tiles = page.locator(".tiles > .tile");
  await expect(tiles).toHaveCount(3);
  const section = await boxOf(page, ".tiles");
  const boxes = await Promise.all([0, 1, 2].map(async (index) => (await tiles.nth(index).boundingBox())!));
  for (const box of boxes) {
    expect(box.width).toBeLessThanOrEqual(220.5);
  }
  expect(Math.abs(boxes[0]!.x - section.x), "先頭のタイルは区画の左端から並ぶ").toBeLessThanOrEqual(1);
});

test("プロジェクトが1件ならカードは一覧の全幅になる", async ({ page, stack }) => {
  await openOverview(page, stack.webUrl, 1920);

  const list = await boxOf(page, ".overview-cards");
  const card = await boxOf(page, ".overview-cards > li");
  expect(Math.abs(card.width - list.width)).toBeLessThanOrEqual(1);
});

test("プロジェクトが2件ならカードは同じ行に半分ずつ並ぶ", async ({ page, twoProjectStack }) => {
  await openOverview(page, twoProjectStack.webUrl, 1920);

  const list = await boxOf(page, ".overview-cards");
  const cards = page.locator(".overview-cards > li");
  await expect(cards).toHaveCount(2);
  const first = (await cards.nth(0).boundingBox())!;
  const second = (await cards.nth(1).boundingBox())!;
  expect(Math.round(first.y)).toBe(Math.round(second.y));
  for (const card of [first, second]) {
    expect(card.width).toBeGreaterThan(list.width / 2 - 20);
    expect(card.width).toBeLessThanOrEqual(list.width / 2);
  }
});

// issue #283: 狭い幅でマージ待ちの通知のプロジェクト名と、実行中の run のゴール・経過が潰れず読める。
const NARROW_WIDTHS = [375, 768, 900, 1000] as const;

const lineCount = (page: Page, selector: string): Promise<number> =>
  page
    .locator(selector)
    .first()
    .evaluate((element) => {
      const range = document.createRange();
      range.selectNodeContents(element);
      const tops = new Set(Array.from(range.getClientRects(), (rect) => Math.round(rect.top)));
      return tops.size;
    });

for (const width of NARROW_WIDTHS) {
  test(`幅 ${width}px で通知のプロジェクト名と run のゴール・経過が潰れず、横にはみ出さない`, async ({ page, stack }) => {
    await openOverview(page, stack.webUrl, width);

    const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
    expect(overflow, "ページが横にはみ出さない").toBeLessThanOrEqual(0);

    const tableOverflow = await page
      .locator('section[aria-labelledby="h-runs"] .l-table')
      .evaluate((element) => element.scrollWidth - element.clientWidth);
    expect(tableOverflow, "run の表が枠の中で横スクロールしない").toBeLessThanOrEqual(0);

    for (const label of ["プロジェクト", "ゴール", "ステップ", "最後のログ", "状態"]) {
      await expect(page.locator(`.run-row td[data-label="${label}"]`), `列「${label}」の名前が各セルに付く`).toHaveCount(1);
    }

    const proj = await boxOf(page, ".callout .proj");
    expect(proj.width, "通知のプロジェクト名が1文字幅に潰れない").toBeGreaterThan(48);
    expect(await lineCount(page, ".callout .proj"), "プロジェクト名が1〜2行").toBeLessThanOrEqual(2);

    const goal = await boxOf(page, ".run-row .run-goal .g");
    expect(goal.width, "run のゴールの見出しが潰れない").toBeGreaterThan(120);
    const goalText = await boxOf(page, ".run-row .run-goal .task");
    expect(goalText.width, "run のゴールの本文が潰れない").toBeGreaterThan(120);

    const viewport = page.viewportSize()!;
    for (const selector of [".run-row .run-elapsed span", ".callout .view", ".run-row .run-project .view"]) {
      const box = await boxOf(page, selector);
      expect(box.x + box.width, `${selector} が右端で見切れない`).toBeLessThanOrEqual(viewport.width + 0.5);
    }
  });
}

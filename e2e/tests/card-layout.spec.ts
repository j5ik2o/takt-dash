import { expect, test } from "../support/test";

// issue #147 の再発防止。カードの li は flex-direction: column で子を縦に積み、
// 概要のプロジェクト欄（issue #120 からは概要専用の .overview-cards）では .card-link を伸ばして
// カード高さを揃え、.view をカードの下へ置く。
// 実 Chrome の描画を通し、boundingBox() の座標・高さで観測する。
test("ゴール一覧のカードで段落が縦に並ぶ", async ({ page, stack }) => {
  await page.goto(`${stack.webUrl}/goals`);
  await expect(page.getByRole("heading", { name: "ゴール", level: 1 })).toBeVisible();

  const firstCard = page.locator(".cards > li.card").first();
  await expect(firstCard).toBeVisible();

  // 直下の段落のうち先頭2つを測り、2番目の上端が1番目の下端より下にあることを確かめる。
  // 段落の margin は 0 のため正しい状態では両者が接する。範囲外の許容は置かない。
  const paragraphs = firstCard.locator(":scope > p");
  await expect(paragraphs.nth(1)).toBeVisible();
  const first = await paragraphs.nth(0).boundingBox();
  const second = await paragraphs.nth(1).boundingBox();
  expect(first).not.toBeNull();
  expect(second).not.toBeNull();
  expect(second!.y).toBeGreaterThanOrEqual(first!.y + first!.height);
});

test("概要のプロジェクト欄で「ゴール一覧を見る」がカードの下に出る", async ({ page, stack }) => {
  await page.goto(stack.webUrl);
  await expect(page.getByRole("heading", { name: "概要", level: 1 })).toBeVisible();

  const viewLinks = page.locator(".overview-cards > li > a.view");
  await expect(viewLinks.first()).toBeVisible();
  const count = await viewLinks.count();
  expect(count).toBeGreaterThan(0);

  for (let i = 0; i < count; i += 1) {
    const view = viewLinks.nth(i);
    await expect(view).toHaveText("ゴール一覧を見る");
    const card = view.locator("xpath=preceding-sibling::a[contains(@class, 'card-link')][1]");
    const cardBox = await card.boundingBox();
    const viewBox = await view.boundingBox();
    expect(cardBox).not.toBeNull();
    expect(viewBox).not.toBeNull();
    // .view の上端が、対応するカードの下端以上にあること。横に並ぶと上端がカード内に収まる。
    expect(viewBox!.y).toBeGreaterThanOrEqual(cardBox!.y + cardBox!.height);
  }
});

test("概要のリンクカードの高さが同じ行で揃う", async ({ page, twoProjectStack }) => {
  await page.goto(twoProjectStack.webUrl);
  await expect(page.getByRole("heading", { name: "概要", level: 1 })).toBeVisible();

  // 内容高さの異なる2プロジェクトを同一行に描画する専用スタックを使い、
  // 同一 top に並ぶリンクカードの高さを boundingBox() で直接比較する。
  const links = page.locator(".overview-cards > li > a.card-link");
  await expect(links.nth(1)).toBeVisible();
  const count = await links.count();
  expect(count).toBeGreaterThanOrEqual(2);

  const boxes: Array<{ top: number; height: number }> = [];
  for (let i = 0; i < count; i += 1) {
    const linkBox = await links.nth(i).boundingBox();
    expect(linkBox).not.toBeNull();
    boxes.push({ top: Math.round(linkBox!.y), height: linkBox!.height });
  }

  // 同じ上端（同じ行）に 2 枚以上並ぶことを要求し、各グループの高さ差が 1px 以内であることを確かめる。
  const byTop = new Map<number, number[]>();
  for (const box of boxes) {
    byTop.set(box.top, [...(byTop.get(box.top) ?? []), box.height]);
  }
  const multiCardRows = [...byTop.values()].filter((heights) => heights.length >= 2);
  expect(multiCardRows.length).toBeGreaterThan(0);
  for (const heights of multiCardRows) {
    expect(Math.max(...heights) - Math.min(...heights)).toBeLessThanOrEqual(1);
  }
});

// issue #148 受入条件3。長い目的文を持つゴールのカードでも、題名は1行に収まり、カードの
// 見出し行を2行へ押し広げないことを実 Chrome の描画で観測する。題名の h3 が flex 項目として
// 縮まないと、題名が行の幅を取り、`about`（プロジェクト名）が次の行へ折り返されて見出し行が
// 2行になる。題名が縮んでいれば `about` と同じ行に収まり見出し行は1行のままになる。
test("ゴール一覧のカードの長い題名は見出し行を1行に保ち親の幅を超えない", async ({ page, longTitleStack }) => {
  await page.goto(`${longTitleStack.webUrl}/goals`);
  await expect(page.getByRole("heading", { name: "ゴール", level: 1 })).toBeVisible();

  const longCard = page
    .locator(".cards > li.card")
    .filter({ has: page.locator(".goal-card-head h3 a", { hasText: "採用ページのトップページにある見出し" }) });
  await expect(longCard).toHaveCount(1);

  const heading = longCard.locator(".goal-card-head h3");
  const head = longCard.locator(".goal-card-head");
  const about = longCard.locator(".goal-card-head > .about");
  await expect(heading).toBeVisible();
  await expect(about).toBeVisible();

  // 題名は折り返さず1行（white-space: nowrap）。
  const whiteSpace = await heading.evaluate((element) => getComputedStyle(element).whiteSpace);
  expect(whiteSpace).toBe("nowrap");

  const headingBox = await heading.boundingBox();
  const headBox = await head.boundingBox();
  const aboutBox = await about.boundingBox();
  expect(headingBox).not.toBeNull();
  expect(headBox).not.toBeNull();
  expect(aboutBox).not.toBeNull();

  // 題名の右端が親の見出し行の右端を超えない（縮んで内側に収まる）。
  expect(headingBox!.x + headingBox!.width).toBeLessThanOrEqual(headBox!.x + headBox!.width + 1);

  // 題名（h3）とプロジェクト名（about）が同じ行に並ぶ。題名が縮まずに幅を取ると about が
  // 折り返して縦位置がずれ、見出し行の高さが題名の高さを大きく超える。上下の中心が1行以内に
  // 収まることを観測し、題名がカードを圧迫していないことを確かめる。
  const headingCenter = headingBox!.y + headingBox!.height / 2;
  const aboutCenter = aboutBox!.y + aboutBox!.height / 2;
  expect(Math.abs(headingCenter - aboutCenter)).toBeLessThanOrEqual(headingBox!.height / 2 + 1);
});

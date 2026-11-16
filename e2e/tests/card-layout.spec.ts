import { expect, test } from "../support/test";

// issue #147 の再発防止。`.cards > li` は flex-direction: column で子を縦に積み、
// 概要のプロジェクト欄では .card-link を伸ばしてカード高さを揃え、.view をカードの下へ置く。
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

  const viewLinks = page.locator(".cards > li > a.view");
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
  const links = page.locator(".cards > li > a.card-link");
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

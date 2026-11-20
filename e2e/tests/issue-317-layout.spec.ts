import { expect, test } from "../support/test";

// issue #317 の狭い幅・列幅の実測。実 Chrome の描画を通し、boundingBox() の座標・
// サイズで観測する。幅とデータはこのファイルと下の定数に固定し、確認結果を残す。
//
// 測定条件（D25/D26/D27 は 390px、D23/D31 は 1280px）:
// - 390px: 操作ボタンの高さ・サイドバーの並び・現在地の可視性を測る。
// - 1280px: 種類の列とサーバー名の行数を測る。
const DESKTOP = { width: 1280, height: 900 };
const MOBILE = { width: 390, height: 844 };

test("390px で判断待ちの操作ボタンの高さが 44px 以上になる", async ({ page, isolatedStack: stack }, testInfo) => {
  await page.setViewportSize(MOBILE);
  await page.goto(`${stack.webUrl}/decisions`);
  await expect(page.getByRole("region", { name: "判断待ちの一覧" })).toBeVisible();

  const buttons = page.locator(".tray-item .actions button");
  await expect(buttons.first()).toBeVisible();
  const count = await buttons.count();
  expect(count).toBeGreaterThan(0);

  for (let index = 0; index < count; index += 1) {
    const box = await buttons.nth(index).boundingBox();
    expect(box).not.toBeNull();
    testInfo.attach(`decision-button-${index}`, {
      body: JSON.stringify({ text: await buttons.nth(index).innerText(), height: box!.height }),
      contentType: "application/json",
    });
    expect(box!.height, `操作ボタン「${await buttons.nth(index).innerText()}」の高さ`).toBeGreaterThanOrEqual(44);
  }
});

test("390px でサイドバーは本文の上に並び、本文を押し下げない", async ({ page, isolatedStack: stack }) => {
  await page.setViewportSize(MOBILE);
  await page.goto(`${stack.webUrl}/decisions`);
  await expect(page.getByRole("navigation", { name: "画面と集約対象のサーバー" })).toBeVisible();
  await expect(page.getByRole("heading", { level: 1 })).toBeVisible();

  const sidebar = await page.getByRole("navigation", { name: "画面と集約対象のサーバー" }).boundingBox();
  const main = await page.locator("main.page").boundingBox();
  expect(sidebar).not.toBeNull();
  expect(main).not.toBeNull();
  // サイドバーの下端が本文の上端以下にある（横並びではなく本文の上に積まれる）。
  expect(sidebar!.y + sidebar!.height).toBeLessThanOrEqual(main!.y + 1);
});

test("390px でサイドバー下端の情報は閉じた折りたたみになり、開くと読める", async ({ page, isolatedStack: stack }) => {
  await page.setViewportSize(MOBILE);
  await page.goto(`${stack.webUrl}/decisions`);
  const foot = page.locator("details.side-foot");
  await expect(foot).toBeVisible();

  // 閉じた状態では中身の最初の段落は見えない。
  const firstParagraph = foot.locator("p").first();
  await expect(firstParagraph).toBeHidden();

  await foot.locator("summary").click();
  await expect(firstParagraph).toBeVisible();
});

test("390px で現在のページのナビ項目が画面の横方向に収まる", async ({ page, isolatedStack: stack }) => {
  await page.setViewportSize(MOBILE);
  // ゴール一覧はナビの現在地（ゴール）を持つ。
  await page.goto(`${stack.webUrl}/goals`);
  const current = page.locator('.nav a[aria-current="page"]');
  await expect(current).toBeVisible();

  const box = await current.boundingBox();
  expect(box).not.toBeNull();
  expect(box!.x, "現在地のナビ項目の左端がビューポート内").toBeGreaterThanOrEqual(0);
  expect(box!.x + box!.width, "現在地のナビ項目の右端がビューポート内").toBeLessThanOrEqual(MOBILE.width);
});

test("1280px でアクティビティの種類の列の文字が隣の本文の列へ重ならない", async ({ page, isolatedStack: stack }) => {
  // 長い種類名（人のマージ待ち・blocked 通知）と短い種類名（run の開始・タスク完了）を
  // 同じ一覧に並べるため、マージ待ちの問い合わせと作業の依頼を状態ファイルへ足す。
  const goalId = "11111111-1111-4111-8111-111111111111";
  await stack.updateState((state) => {
    const goalQuestions = (state.goalQuestions as Record<string, unknown[]>) ?? {};
    const existing = goalQuestions[goalId] ?? [];
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
            createdAt: new Date().toISOString(),
          },
        ],
      },
    };
  });

  await page.setViewportSize(DESKTOP);
  await page.goto(`${stack.webUrl}/activity`);

  const rows = page.locator("ul.events > li.ev");
  await expect(rows.first()).toBeVisible();
  const count = await rows.count();
  expect(count).toBeGreaterThan(0);

  // 種類の列の文字が、隣の本文の列の左端より右へはみ出していないこと。
  for (let index = 0; index < count; index += 1) {
    const row = rows.nth(index);
    const kind = await row.locator(".kl").boundingBox();
    const message = await row.locator(".msg").boundingBox();
    expect(kind).not.toBeNull();
    expect(message).not.toBeNull();
    expect(
      kind!.x + kind!.width,
      `種類「${await row.locator(".kl").innerText()}」の列の右端が本文の列の左端を超える`,
    ).toBeLessThanOrEqual(message!.x + 1);
  }
});

test("1280px でサイドバーのサーバー名が2行に折り返さない", async ({ page, longServerStack: stack }, testInfo) => {
  await page.setViewportSize(DESKTOP);
  await page.goto(`${stack.webUrl}/decisions`);
  const name = page.locator(".srv-list .nm").first();
  await expect(name).toBeVisible();

  const box = await name.boundingBox();
  const fontSize = await name.evaluate((element) => parseFloat(getComputedStyle(element).fontSize));
  const lineHeight = await name.evaluate((element) => parseFloat(getComputedStyle(element).lineHeight));
  const singleLine = Number.isFinite(lineHeight) ? lineHeight : fontSize * 1.6;
  testInfo.attach("server-name-box", {
    body: JSON.stringify({ text: await name.innerText(), width: box!.width, height: box!.height, singleLine }),
    contentType: "application/json",
  });
  // 1行分（行送りの1.5倍を上限）に収まる。2行なら行送りの2倍近くになる。
  expect(box!.height).toBeLessThanOrEqual(singleLine * 1.5);
});

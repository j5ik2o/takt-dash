import type { Page } from "@playwright/test";
import { test as base, expect } from "../support/test";
import { startStack, type Stack } from "../support/stack";
import { buildMockState, SERVER_ID, SERVER_NAME } from "../support/fixture";

// issue #121 の判断待ちの項目の骨格（見出し行 → 本文 → 回答ボタン）を、実 Chrome の描画で測る。
// 本文の長さ・改行の数・選択肢の有無・滞留の有無が違う項目を並べ、見出し行・本文・回答ボタンの
// 位置がそろうこと、待ち時間が見出し行の右端に来ること、本文の上限が 80ch 前後であること、
// 改行が保たれること、サーバーの ID と表示名が項目に出ないことを boundingBox() で観測する。
const HEIGHT = 900;
const WIDTHS = [360, 390, 1280, 1920, 2560] as const;
// 待ち時間をプロジェクトと同じ行に置けることと、本文を 80ch の上限まで広げられることは、
// 見出し行と本文に十分な幅がある広い画面でだけ測る。狭い画面では折り返しと画面幅が正しい。
const WIDE_WIDTHS = [1280, 1920, 2560] as const;

const SHORT_BODY = "判断待ちの配置確認用の短い本文";
const MULTILINE_BODY = "判断待ちの配置確認用の改行を含む本文\n2行目の文です。\n3行目の文です。";
const LONG_BODY =
  "判断待ちの配置確認用の長い本文です。本文が長くても見出し行と回答ボタンの位置が変わらないことを確かめるため、この文は意図的に長くして、80ch の上限で何行にも折り返させます。".repeat(3);
const OPTIONS_BODY = "判断待ちの配置確認用の選択肢を持つ本文";

type Box = { x: number; y: number; width: number; height: number };

type ItemGeometry = {
  childClasses: string[];
  head: Box;
  proj: Box;
  wait: Box;
  body: Box;
  actions: Box;
  text: string;
  bodyText: string;
  firstParagraph: Box | null;
  chWidth: number;
};

const inquiry = (id: string, body: string, createdAt: string, options: string[] = []): Record<string, unknown> => ({
  id,
  status: "pending",
  kind: "question",
  body,
  options,
  createdAt,
});

// 既定の fixture（マージ待ちのゴールと、ゴール所属の1行の質問）に、本文の形が違う単独の問い合わせを足す。
// 1件は滞留の閾値（240分）を超える時刻にして、見出し行に滞留の印が出る項目も並べる。
const decisionsLayoutStack = async ({}, use: (stack: Stack) => Promise<void>): Promise<void> => {
  const stack = await startStack({
    mockState: (now) => {
      const at = now.toISOString();
      const staleAt = new Date(now.getTime() - 300 * 60_000).toISOString();
      return {
        ...buildMockState(now),
        standaloneInquiries: [
          inquiry("e2e-layout-short", SHORT_BODY, at),
          inquiry("e2e-layout-multiline", MULTILINE_BODY, at),
          inquiry("e2e-layout-long", LONG_BODY, staleAt),
          inquiry("e2e-layout-options", OPTIONS_BODY, at, ["今週", "来週"]),
        ],
      };
    },
  });
  try {
    await use(stack);
  } finally {
    await stack.stop();
  }
};

const test = base.extend<{ decisionsLayoutStack: Stack }>({ decisionsLayoutStack });

const openDecisions = async (page: Page, url: string, width: number): Promise<void> => {
  await page.setViewportSize({ width, height: HEIGHT });
  await page.goto(`${url}/decisions`);
  await expect(page.getByRole("heading", { name: "判断待ち", level: 1 })).toBeVisible();
  const list = page.getByRole("region", { name: "判断待ちの一覧" });
  for (const body of [SHORT_BODY, OPTIONS_BODY]) {
    await expect(list.getByText(body, { exact: true })).toBeVisible();
  }
  // 足した4件と既定の質問1件は必ず並ぶ（マージ待ちの件数は fixture の判定に任せ、ここでは数えない）。
  expect(await list.locator("li.tray-item").count()).toBeGreaterThanOrEqual(5);
};

const itemGeometries = (page: Page): Promise<ItemGeometry[]> =>
  page.locator("li.tray-item").evaluateAll((items) => {
    const box = (element: Element | null | undefined) => {
      if (element === null || element === undefined) {
        return { x: Number.NaN, y: Number.NaN, width: Number.NaN, height: Number.NaN };
      }
      const rect = element.getBoundingClientRect();
      return { x: rect.x, y: rect.y, width: rect.width, height: rect.height };
    };
    return items.map((item) => {
      const children = Array.from(item.children);
      const head = children[0];
      const body = children[1];
      const actions = children[2];
      // 本文の行のフォントで 80ch の幅を測る。
      const probe = document.createElement("span");
      probe.style.cssText = "display:inline-block;width:80ch;height:0;";
      body?.appendChild(probe);
      const chWidth = probe.getBoundingClientRect().width;
      probe.remove();
      const firstParagraph = body?.querySelector(".desc p") ?? null;
      return {
        childClasses: children.map((child) => child.className),
        head: box(head),
        proj: box(head?.querySelector(".proj")),
        wait: box(head?.querySelector(".wait")),
        body: box(body),
        actions: box(actions),
        text: (item as HTMLElement).innerText,
        bodyText: (body as HTMLElement | undefined)?.innerText ?? "",
        firstParagraph: firstParagraph === null ? null : box(firstParagraph),
        chWidth,
      };
    });
  });

const findItem = (items: ItemGeometry[], needle: string): ItemGeometry => {
  const item = items.find((candidate) => candidate.bodyText.includes(needle));
  expect(item, `本文に「${needle}」を含む項目がある`).toBeDefined();
  return item!;
};

for (const width of WIDTHS) {
  test(`幅 ${width}px で判断待ちの各項目は見出し行・本文・回答ボタンの順に縦に並ぶ`, async ({ page, decisionsLayoutStack }) => {
    await openDecisions(page, decisionsLayoutStack.webUrl, width);
    const items = await itemGeometries(page);

    for (const item of items) {
      expect(item.childClasses.length, `項目の直下は3つ: ${item.text}`).toBe(3);
      expect(item.childClasses[0]).toMatch(/\bdecision-head\b/);
      expect(item.childClasses[1]).toMatch(/\bdecision-body\b/);
      expect(item.childClasses[2]).toMatch(/\b(actions|note)\b/);

      expect(item.head.y + item.head.height, "見出し行は本文より上").toBeLessThanOrEqual(item.body.y + 1);
      const gap = item.actions.y - (item.body.y + item.body.height);
      expect(gap, "回答ボタンは本文の下に重ならずに来る").toBeGreaterThanOrEqual(-1);
      expect(gap, "回答ボタンは本文のすぐ後ろに来る").toBeLessThanOrEqual(24);
    }
  });

  test(`幅 ${width}px で本文の長さが違っても、見出し行・本文・回答ボタンの左端がそろう`, async ({ page, decisionsLayoutStack }) => {
    await openDecisions(page, decisionsLayoutStack.webUrl, width);
    const items = await itemGeometries(page);
    const left = items[0]!.head.x;

    for (const item of items) {
      for (const [label, rect] of [
        ["見出し行", item.head],
        ["本文", item.body],
        ["回答ボタン", item.actions],
      ] as const) {
        expect(Math.abs(rect.x - left), `${label}の左端がそろう: ${item.bodyText.slice(0, 20)}`).toBeLessThanOrEqual(1);
      }
    }
  });

  test(`幅 ${width}px で判断待ちの項目にサーバーの ID も表示名も出ず、横にはみ出さない`, async ({ page, decisionsLayoutStack }) => {
    await openDecisions(page, decisionsLayoutStack.webUrl, width);
    const items = await itemGeometries(page);

    for (const item of items) {
      expect(item.text).not.toContain(SERVER_ID);
      expect(item.text).not.toContain(SERVER_NAME);
      expect(item.text).not.toContain("サーバー");
    }
    const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
    expect(overflow).toBeLessThanOrEqual(0);

    // 狭い画面では body が横のはみ出しを隠すため、scrollWidth が 0 でも中身は切れ得る。
    // 各項目の見出し行・本文・回答ボタンの右端が画面の幅を超えないことを測る。
    if (width <= 390) {
      for (const item of items) {
        for (const [label, rect] of [
          ["見出し行", item.head],
          ["本文", item.body],
          ["回答ボタン", item.actions],
        ] as const) {
          expect(rect.x, `${label}の左端が画面の中: ${item.bodyText.slice(0, 20)}`).toBeGreaterThanOrEqual(0);
          expect(
            rect.x + rect.width,
            `${label}の右端が画面の幅に収まる: ${item.bodyText.slice(0, 20)}`,
          ).toBeLessThanOrEqual(width + 0.5);
        }
      }
    }
  });
}

for (const width of WIDE_WIDTHS) {
  test(`幅 ${width}px で待ち時間と滞留の印は見出し行の右端に、プロジェクトと同じ行に並ぶ`, async ({ page, decisionsLayoutStack }) => {
    await openDecisions(page, decisionsLayoutStack.webUrl, width);
    const items = await itemGeometries(page);

    for (const item of items) {
      const headRight = item.head.x + item.head.width;
      const waitRight = item.wait.x + item.wait.width;
      expect(Math.abs(headRight - waitRight), "待ち時間の右端は見出し行の右端").toBeLessThanOrEqual(1);
      expect(item.wait.x, "待ち時間はプロジェクトの群より右").toBeGreaterThanOrEqual(item.proj.x + item.proj.width);
      const sameRow = item.wait.y < item.proj.y + item.proj.height && item.proj.y < item.wait.y + item.wait.height;
      expect(sameRow, "待ち時間はプロジェクトと同じ行に並ぶ").toBe(true);
    }

    const stale = page.locator("li.tray-item.stale .decision-head .wait");
    await expect(stale.first()).toContainText("滞留しています。");
  });

  test(`幅 ${width}px で本文の1行の上限は 80ch 前後になり、改行が保たれる`, async ({ page, decisionsLayoutStack }) => {
    await openDecisions(page, decisionsLayoutStack.webUrl, width);
    const items = await itemGeometries(page);

    for (const item of items) {
      expect(item.body.width, "本文の幅は 80ch 前後を超えない").toBeLessThanOrEqual(item.chWidth * 1.125);
    }
    const long = findItem(items, "長い本文");
    expect(long.body.width, "長い本文は上限まで広がる").toBeGreaterThanOrEqual(long.chWidth * 0.875);

    const short = findItem(items, SHORT_BODY);
    const multiline = findItem(items, "改行を含む本文");
    expect(short.firstParagraph).not.toBeNull();
    expect(multiline.firstParagraph).not.toBeNull();
    expect(
      multiline.firstParagraph!.height,
      "3行の本文は改行が保たれて1行の本文の3行分の高さになる",
    ).toBeGreaterThanOrEqual(short.firstParagraph!.height * 2.5);
  });
}

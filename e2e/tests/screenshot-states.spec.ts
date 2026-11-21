import type { Page } from "@playwright/test";
import { test as base, expect } from "../support/test";
import { startStack, type Stack } from "../support/stack";
import { SCREENSHOT_STATES, type ScreenshotState } from "../screenshot/states";

// issue #273 の撮影用の状態。撮影の手順（e2e/screenshot）では、概要のプロジェクト欄で
// カードが複数の列へ折り返し、「ほか M 件のプロジェクトを見る」が /projects を指す様子を
// 画像で目視できる状態が必要になる。ここではその状態が撮影用の状態一覧に含まれることと、
// その状態のプロジェクト（設定由来）を実スタックで描画した概要が折り返しとリンクを出すことを観測する。
const HEIGHT = 900;
const WIDTH = 1280;
const MIN_PROJECTS = 7;

const moreProjectsState = (): ScreenshotState | undefined =>
  SCREENSHOT_STATES.find((state) => state.projects.length >= MIN_PROJECTS);

const startStateStack = async ({}, use: (stack: Stack) => Promise<void>): Promise<void> => {
  const state = moreProjectsState();
  if (state === undefined) {
    throw new Error(`プロジェクトが ${MIN_PROJECTS} 件以上の撮影用の状態がありません。`);
  }
  const stack = await startStack({ projects: state.projects, mockState: state.mockState });
  try {
    await use(stack);
  } finally {
    await stack.stop();
  }
};

const test = base.extend<{ stateStack: Stack }>({ stateStack: startStateStack });

const openOverview = async (page: Page, url: string): Promise<void> => {
  await page.setViewportSize({ width: WIDTH, height: HEIGHT });
  await page.goto(url);
  await expect(page.getByRole("heading", { name: "概要", level: 1 })).toBeVisible();
  await expect(page.locator("section[aria-labelledby='h-projects'] .overview-cards > li").first()).toBeVisible();
};

test("撮影用の状態にプロジェクトが7件以上の状態がある", () => {
  const state = moreProjectsState();
  expect(state, `プロジェクトが ${MIN_PROJECTS} 件以上の状態が必要`).toBeDefined();
  expect(state!.projects.length).toBeGreaterThanOrEqual(MIN_PROJECTS);
});

test("7件以上の状態の概要はカードが複数の行へ折り返し、「ほか M 件」のリンクが /projects を指す", async ({
  page,
  stateStack,
}) => {
  await openOverview(page, stateStack.webUrl);

  const section = page.locator("section[aria-labelledby='h-projects']");
  const cards = section.locator(".overview-cards > li");
  const visible = await cards.count();
  expect(visible).toBeGreaterThanOrEqual(2);

  // カードが折り返していることを、上端の種類が2つ以上あることで観測する。
  const tops = new Set<number>();
  for (let index = 0; index < visible; index += 1) {
    const box = await cards.nth(index).boundingBox();
    expect(box, "カードが描画されている").not.toBeNull();
    tops.add(Math.round(box!.y));
  }
  expect(tops.size, "カードは複数の行へ折り返す").toBeGreaterThanOrEqual(2);

  const total = moreProjectsState()!.projects.length;

  // 上限を超えた分は /projects への「ほか M 件」で示す。M は表示しなかった件数。
  const more = section.getByRole("link", { name: /ほか\s*\d+\s*件のプロジェクトを見る/ });
  await expect(more).toBeVisible();
  await expect(more).toHaveAttribute("href", "/projects");
  const label = ((await more.textContent()) ?? "").replace(/\s+/g, " ");
  const match = /ほか (\d+) 件/.exec(label);
  expect(match, "リンクの文に「ほか M 件」がある").not.toBeNull();
  expect(Number(match![1])).toBe(total - visible);
});

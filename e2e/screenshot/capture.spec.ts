// 撮影の手順。4つのデータ状態ごとに E2E の土台（startStack）でスタックを1つ起動し、
// 全ページを3つの幅でページ全体まで撮る。画素の差分による判定はしない（目で見比べるための画像を作るだけ）。
//
// 出力先は TAKTDASH_SCREENSHOTS_DIR（既定は .gitignore で除外した e2e/screenshots）の下の
// TAKTDASH_SCREENSHOTS_LABEL（既定は current）のディレクトリ。ファイル名は
// `<ページ>--w<幅>--<状態>.png` で、変更の前後を同じ名前で見比べられる。
import path from "node:path";
import { expect, test } from "@playwright/test";
import { startStack, type Stack } from "../support/stack";
import { SCREENSHOT_PAGES } from "./pages";
import { SCREENSHOT_STATES } from "./states";

const WIDTHS = [1280, 1920, 2560] as const;
// fullPage で撮るため、高さはページの長さに関係なく最初の表示領域を決めるだけの値。
const VIEWPORT_HEIGHT = 1080;

const outputDir = path.resolve(
  process.env.TAKTDASH_SCREENSHOTS_DIR ?? path.join(__dirname, "..", "screenshots"),
  process.env.TAKTDASH_SCREENSHOTS_LABEL ?? "current",
);

for (const state of SCREENSHOT_STATES) {
  test.describe(`データ状態 ${state.name}`, () => {
    let stack: Stack | undefined;

    test.beforeAll(async () => {
      stack = await startStack({ projects: state.projects, mockState: state.mockState });
    });

    test.afterAll(async () => {
      await stack?.stop();
    });

    for (const target of SCREENSHOT_PAGES) {
      test(`${target.name} を3つの幅で撮る`, async ({ page }) => {
        if (stack === undefined) {
          throw new Error("スタックが起動していません。");
        }
        for (const width of WIDTHS) {
          await page.setViewportSize({ width, height: VIEWPORT_HEIGHT });
          const response = await page.goto(`${stack.webUrl}${target.path(state)}`);
          if (target.detail && !state.detail.exists) {
            expect(response?.status()).toBe(404);
          } else {
            expect(response?.status()).toBe(200);
            await expect(page.getByRole("heading", { name: target.heading(state), level: 1 })).toBeVisible();
          }
          await page.screenshot({
            path: path.join(outputDir, `${target.name}--w${width}--${state.name}.png`),
            fullPage: true,
            animations: "disabled",
          });
        }
      });
    }
  });
}

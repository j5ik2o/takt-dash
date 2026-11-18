import { defineConfig } from "@playwright/test";

// 撮影の手順だけを回す設定。通常の E2E（playwright.config.ts の ./tests）とは別の入口にし、
// make e2e の対象・所要時間・合否に影響させない。実行の条件は通常の E2E と同じにする。
export default defineConfig({
  testDir: "./screenshot",
  // 失敗時の成果物も、.gitignore で除外した撮影の出力先の下へ置く。
  outputDir: "./screenshots/.playwright",
  fullyParallel: false,
  workers: 1,
  retries: 0,
  timeout: 120_000,
  reporter: "line",
  use: {
    channel: "chrome",
    headless: true,
    trace: "off",
  },
});

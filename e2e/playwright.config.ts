import { defineConfig } from "@playwright/test";

// 実 Chrome を使い、並行実行での取り合いを避けるためワーカー1・再試行0で回す。
// スタックは各ワーカーの worker-scoped fixture が起動・停止する。
export default defineConfig({
  testDir: "./tests",
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

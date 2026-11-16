import { defineConfig } from "@playwright/test";

// 展開済みの本番へ読み取り専用で接続する。スタック起動や空きポート確保は行わない。
// 接続先の解決はこの設定ファイルの境界で1回だけ行う。
const prodBaseUrl = process.env.TAKTDASH_PROD_BASE_URL ?? "http://localhost:3000";

export default defineConfig({
  testDir: "./smoke",
  fullyParallel: false,
  workers: 1,
  retries: 0,
  timeout: 120_000,
  reporter: "line",
  use: {
    baseURL: prodBaseUrl,
    channel: "chrome",
    headless: true,
    trace: "off",
  },
});

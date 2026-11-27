import { readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import type { Page, Response as PlaywrightResponse } from "@playwright/test";
import { GOAL_TEXT, RUN_SLUG } from "../support/fixture";
import { expect, test } from "../support/test";

// issue #430: エラー画面の「もう一度読み込む」が、サーバーの再取得まで行って復帰するか。
//
// run 詳細の取得を一度だけ失敗させて error.tsx を出し、ボタンを1回押したときに
// GetRunDetail の再取得が走り、成功したら元の run 詳細へ戻ることを実ブラウザで観測する。
// 取得の失敗は、backend が run の保存場所を読む meta.json の status を壊して INTERNAL にし、
// 成功は元の meta.json へ戻して作る（backend も Web も本番のコードのまま）。
// あわせて、失敗が続くときに1回のクリックで再取得が1回を超えないこと（無限ループ・
// 二重送信が無いこと）を同じファイルで確かめる。
//
// 状態ストリーム（/api/state）は受信のたびに router.refresh を呼び、ボタンを押さなくても
// /runs/<slug> への要求を増やす。要求の数を数えるあいだは、issue #307 と同じ方法で
// /api/state を失敗させ、開いている EventSource を閉じて通知を止める。
const RUN_URL_PATH = `/runs/${RUN_SLUG}`;
const SSE_PATH = /\/api\/state(\?|$)/;
const metaPath = (projectDir: string): string => path.join(projectDir, ".takt", "runs", RUN_SLUG, "meta.json");

// 開いた EventSource を控え、テストから閉じられるようにする（issue #307 と同じ）。
const recordEventSources = (): void => {
  const opened: EventSource[] = [];
  const Original = window.EventSource;
  window.EventSource = class extends Original {
    constructor(url: string | URL, init?: EventSourceInit) {
      super(url, init);
      opened.push(this);
    }
  };
  (window as unknown as { __disconnectStateStream: () => void }).__disconnectStateStream = () => {
    for (const source of opened.splice(0)) {
      source.close();
      source.onerror?.(new Event("error"));
    }
  };
};

// 画面を開く前に /api/state を失敗させ、開いた EventSource も閉じる。以降は状態の通知が
// 届かないため、router.refresh による要求がボタンの押下と混ざらない。
const stopStateStream = async (page: Page, url: string): Promise<PlaywrightResponse | null> => {
  await page.addInitScript(recordEventSources);
  await page.route(SSE_PATH, (route) => route.abort());
  const response = await page.goto(url);
  await page.evaluate(() => (window as unknown as { __disconnectStateStream: () => void }).__disconnectStateStream());
  return response;
};

// status は RunStatus として解釈できない値にし、backend の RunReadFailure::InvalidMeta から
// INTERNAL を返させる。文言やその他の欄は元のまま残す。
const corruptMeta = async (projectDir: string): Promise<Record<string, unknown>> => {
  const file = metaPath(projectDir);
  const original = JSON.parse(await readFile(file, "utf8")) as Record<string, unknown>;
  await writeFile(file, JSON.stringify({ ...original, status: "issue-430-broken" }), "utf8");
  return original;
};

const restoreMeta = async (projectDir: string, meta: Record<string, unknown>): Promise<void> => {
  await writeFile(metaPath(projectDir), JSON.stringify(meta), "utf8");
};

// エラー画面から押すボタン。文言は app/error.tsx のもの。
const reloadButton = (page: Page) => page.getByRole("button", { name: "もう一度読み込む" });

const isRunRequest = (url: string): boolean => new URL(url).pathname === RUN_URL_PATH;

// run 詳細の URL への要求（初回の文書と、reset の先で起きる RSC の再取得の両方）を数える。
const countRunRequests = (page: Page): (() => number) => {
  let count = 0;
  page.on("request", (request) => {
    if (isRunRequest(request.url())) {
      count += 1;
    }
  });
  return () => count;
};

test("もう一度読み込むを1回押すと、run 詳細を取り直して元の画面に戻る", async ({ page, isolatedStack: stack }) => {
  const original = await corruptMeta(stack.projectDir);

  const response = await stopStateStream(page, `${stack.webUrl}${RUN_URL_PATH}`);
  // 壊した meta は RunNotFoundError ではなく run-detail-loader が rethrow する失敗なので、
  // Next は 500 と error.tsx を返す。状態ストリームの要求が混ざるかとは別に、最初の遷移だけで確かめる。
  expect(response?.status()).toBe(500);
  await expect(page.getByRole("heading", { name: "画面を表示できませんでした", level: 1 })).toBeVisible();
  await expect(reloadButton(page)).toBeVisible();

  // 取得が成功する状態へ戻す。ここで押した1回が再取得を起こせば、run 詳細が出る。
  await restoreMeta(stack.projectDir, original);
  const runRequests = countRunRequests(page);
  const refetchRequest = page.waitForRequest((request) => isRunRequest(request.url()), { timeout: 10_000 });
  const refetchResponse = page.waitForResponse((response) => isRunRequest(response.url()), { timeout: 10_000 });

  await reloadButton(page).click();
  await refetchRequest;
  await refetchResponse;

  expect(runRequests()).toBe(1);
  await expect(page.getByRole("heading", { name: GOAL_TEXT, level: 1 })).toBeVisible();
  await expect(page.getByRole("heading", { name: "画面を表示できませんでした", level: 1 })).toHaveCount(0);
});

test("失敗が続くときも、1回のクリックで再取得は1回までで、無限ループにも二重送信にもならない", async ({
  page,
  isolatedStack: stack,
}) => {
  await corruptMeta(stack.projectDir);

  await stopStateStream(page, `${stack.webUrl}${RUN_URL_PATH}`);
  await expect(page.getByRole("heading", { name: "画面を表示できませんでした", level: 1 })).toBeVisible();

  // 壊れたまま押す。1回の再取得の要求と応答を待ってから、追加の要求が無いことを見る。
  // 固定の待ち時間だけで判定すると、応答が遅いときにそのあとの二重送信を見逃す。
  const runRequests = countRunRequests(page);
  const refetchRequest = page.waitForRequest((request) => isRunRequest(request.url()), { timeout: 10_000 });
  const refetchResponse = page.waitForResponse((response) => isRunRequest(response.url()), { timeout: 10_000 });

  await reloadButton(page).click();
  await refetchRequest;
  await refetchResponse;

  expect(runRequests()).toBe(1);
  // 応答のあとに遅れて届く二重送信も見る。
  await page.waitForTimeout(1_000);
  expect(runRequests()).toBe(1);
  await expect(page.getByRole("heading", { name: "画面を表示できませんでした", level: 1 })).toHaveCount(1);
});

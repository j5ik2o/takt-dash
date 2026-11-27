import type { Page } from "@playwright/test";
import { test, expect } from "../support/test";
import { PROJECT_NAME } from "../support/fixture";

// issue #429（docs/design/issue-429.md）の確かめ。
// 存在しない run・ゴール・タスクの詳細 URL が HTTP 200 ではなく 404 を返すことを固定する。
// いまの本番はルートの loading.tsx が存在チェックより先に fallback を流すため 200 になる（red）。
const targets = [
  "/runs/no-such-run-429",
  "/goals/00000000-0000-4000-8000-000000004290",
  `/tasks/${encodeURIComponent(PROJECT_NAME)}?task=no-such-task-429`,
];

const LOADING_TEXT = "読み込んでいます…";
// 遷移をまたいで印を残すため、document ではなく sessionStorage に置く。
// addInitScript は document ごとに走り直すので、window 上の印では遷移のたびに消えてしまう。
const LOADING_TEXT_SEEN_KEY = "__issue429_loading_text_seen";

const readLoadingTextSeen = async (page: Page): Promise<string[]> =>
  page.evaluate((key) => {
    const raw = sessionStorage.getItem(key);
    return raw === null ? [] : (JSON.parse(raw) as string[]);
  }, LOADING_TEXT_SEEN_KEY);

test("存在しない run・ゴール・タスクの詳細は 404 を返し、読み込み中の文言を出さない", async ({ page, stack }) => {
  // 404 の見出しが表示されてから文言の有無を見るだけでは、遷移の途中で一瞬だけ出る
  // 「読み込んでいます…」を取り逃す。DOM が組み立てられる前から変化を監視し、
  // 文言が一度でも描画された時点で、その URL を sessionStorage に記録する。
  await page.addInitScript((text: string) => {
    const key = "__issue429_loading_text_seen";
    const ignoredTags = new Set(["SCRIPT", "STYLE", "TEMPLATE", "NOSCRIPT"]);
    let recorded = false;
    const record = (): void => {
      if (recorded) {
        return;
      }
      recorded = true;
      const raw = sessionStorage.getItem(key);
      const seen: string[] = raw === null ? [] : (JSON.parse(raw) as string[]);
      seen.push(`${location.pathname}${location.search}`);
      sessionStorage.setItem(key, JSON.stringify(seen));
    };
    // 表示中の本文に文言があるか。描画された後の状態を見る。
    const visible = (): boolean => document.body !== null && document.body.innerText.includes(text);
    // 追加されたノードは、同じタスクのうちに外されても textContent が残る。
    // 一瞬で消える場合も取り逃さないよう、script などの非表示要素を除いて中身を見る。
    const mentionsText = (root: Node): boolean => {
      const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
      for (let node = walker.nextNode(); node !== null; node = walker.nextNode()) {
        const parent = node.parentElement;
        if ((parent === null || !ignoredTags.has(parent.tagName)) && (node.textContent ?? "").includes(text)) {
          return true;
        }
      }
      return false;
    };
    const remember = (records: MutationRecord[]): void => {
      const added = records.some((record) => Array.from(record.addedNodes).some(mentionsText));
      if (visible() || added) {
        record();
      }
    };
    new MutationObserver(remember).observe(document, { childList: true, subtree: true, characterData: true });
    document.addEventListener("DOMContentLoaded", () => remember([]));
    window.addEventListener("load", () => remember([]));
    remember([]);
  }, LOADING_TEXT);

  const expectNoLoadingTextSeen = async (where: string): Promise<void> => {
    expect(
      await readLoadingTextSeen(page),
      `${where} の遷移中に「${LOADING_TEXT}」が一度でも表示された`,
    ).toEqual([]);
  };

  for (const path of targets) {
    const response = await page.goto(`${stack.webUrl}${path}`);
    expect(response?.status(), path).toBe(404);
    await expect(page.getByRole("heading", { name: "ページが見つかりません" })).toBeVisible();
    await expect(page.getByRole("link", { name: "概要に戻る" })).toBeVisible();
    await expect(page.getByText(LOADING_TEXT, { exact: true })).toHaveCount(0);
    // 見出しが出た後だけでなく、遷移の間に出たかどうかも確かめる。
    await expectNoLoadingTextSeen(path);
  }

  // 最後の遷移の後も、記録が残っていないことを確かめる。
  await expectNoLoadingTextSeen("テスト終了時");
});

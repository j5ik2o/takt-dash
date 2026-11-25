import { expect, test } from "../support/test";

// issue #307: 切断中は、最後に読めた判断待ちを残したまま回答の操作を止め、再接続すると戻す。
// ブラウザのオフライン化ではループバックの SSE が切れないため、/api/state への接続を
// 失敗させたうえで、開いている EventSource を閉じてエラーを伝える。経路を戻すと、画面の
// 再接続の予約が次の接続を開き、状態を受け取って切断の表示が消える。
const SSE_PATH = "**/api/state";

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

test("切断中は判断待ちの回答ボタンを押せず、再接続すると押せて確認を閉じたトリガーへフォーカスが戻る", async ({
  page,
  isolatedStack: stack,
}) => {
  await page.addInitScript(recordEventSources);
  await page.goto(`${stack.webUrl}/decisions`);

  const item = page
    .getByRole("region", { name: "判断待ちの一覧" })
    .getByRole("listitem")
    .filter({ hasText: "人のマージ待ち" });
  const trigger = item.getByRole("button", { name: "マージを承認" });
  await expect(trigger).toBeEnabled();
  await expect(page.getByText("再接続するまで操作できません。")).toHaveCount(0);

  await trigger.click();
  const dialog = page.getByRole("dialog", { name: "マージの承認" });
  await expect(dialog).toBeVisible();
  const confirm = dialog.getByRole("button", { name: "マージする" });
  await confirm.focus();
  await expect(confirm).toBeFocused();

  await page.route(SSE_PATH, (route) => route.abort());
  await page.evaluate(() => (window as unknown as { __disconnectStateStream: () => void }).__disconnectStateStream());

  await expect(confirm).toBeDisabled();
  // 無効になった確定ボタンからフォーカスが落ちても、ダイアログの見出しに留まる。
  await expect(dialog.getByRole("heading", { name: "マージの承認" })).toBeFocused();
  await expect(dialog).toContainText("再接続するまで操作できません。");
  await dialog.getByRole("button", { name: "やめる" }).click();
  await expect(dialog).toBeHidden();
  await expect(trigger).toBeDisabled();
  await expect(item).toContainText("再接続するまで操作できません。");

  await page.unroute(SSE_PATH);

  await expect(trigger).toBeEnabled({ timeout: 60_000 });
  await expect(trigger).toBeFocused();
  await expect(page.getByText("再接続するまで操作できません。")).toHaveCount(0);
  expect((await stack.readCalls()).filter((call) => call.name === "takt_merge_goal_pr")).toHaveLength(0);
});

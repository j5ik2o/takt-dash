import { expect, test } from "../support/test";

// issue #383: SSE の接続が半開きになり、onerror も受信も来なくなった状態を再現する。
// 開いている EventSource の onmessage と onerror を握りつぶし、ブラウザの時計を 40 秒
// 進めると、画面は無音の見張りで切断として扱う。最後に読めた判断待ちは残したまま回答の
// 操作を止め（#378、#381）、開き直した接続の受信で操作を戻す。
const installHalfOpenSwitch = (): void => {
  const opened: Array<{ muted: boolean }> = [];
  const Original = window.EventSource;
  const delivered = { messages: 0 };
  (window as unknown as { __stateStreamDelivered: typeof delivered }).__stateStreamDelivered = delivered;
  const mute = <E,>(owner: { muted: boolean }, handler: ((event: E) => void) | null, count = false) =>
    handler === null
      ? null
      : (event: E) => {
          if (owner.muted) return;
          handler(event);
          if (count) delivered.messages += 1;
        };
  window.EventSource = class extends Original {
    muted = false;
    constructor(url: string | URL, init?: EventSourceInit) {
      super(url, init);
      opened.push(this);
    }
    override set onmessage(handler: ((event: MessageEvent) => void) | null) {
      super.onmessage = mute(this, handler, true);
    }
    override get onmessage() {
      return super.onmessage;
    }
    override set onerror(handler: ((event: Event) => void) | null) {
      super.onerror = mute(this, handler);
    }
    override get onerror() {
      return super.onerror;
    }
  };
  (window as unknown as { __halfOpenStateStream: () => void }).__halfOpenStateStream = () => {
    for (const source of opened.splice(0)) {
      source.muted = true;
    }
  };
};

test("半開きで無音になった接続を 40 秒で切断として扱い、判断待ちを残して操作を止め、開き直すと戻す", async ({
  page,
  isolatedStack: stack,
}) => {
  await page.clock.install();
  await page.addInitScript(installHalfOpenSwitch);
  await page.goto(`${stack.webUrl}/decisions`);

  const item = page
    .getByRole("region", { name: "判断待ちの一覧" })
    .getByRole("listitem")
    .filter({ hasText: "人のマージ待ち" });
  const trigger = item.getByRole("button", { name: "マージを承認" });
  await expect(trigger).toBeEnabled();
  await expect(page.getByText("再接続するまで操作できません。")).toHaveCount(0);
  // サーバー描画の初期状態でもボタンは有効なため、SSE の最初の受信が画面へ届いてから
  // 半開きにする。受信済みの接続が黙る経路を確かめる。
  await page.waitForFunction(
    () => (window as unknown as { __stateStreamDelivered: { messages: number } }).__stateStreamDelivered.messages > 0,
  );

  // 時計を止めてから 40 秒進める。止めずに進めると、テスト側が遅れた間に再接続の 1 秒が
  // 過ぎて新しい接続が受信し、切断中の画面を観測できないことがある。
  await page.clock.pauseAt(await page.evaluate(() => Date.now() + 1));
  await page.evaluate(() => (window as unknown as { __halfOpenStateStream: () => void }).__halfOpenStateStream());
  await page.clock.fastForward(40_000);

  await expect(trigger).toBeDisabled();
  await expect(item).toContainText("人のマージ待ち");
  await expect(item).toContainText("再接続するまで操作できません。");
  await expect(page.getByRole("alert").first()).toBeVisible();

  // 再接続の予約（1 秒）を進めると、新しい接続が開き、最初の受信で操作が戻る。
  await page.clock.fastForward(1_000);
  await page.clock.resume();
  await expect(trigger).toBeEnabled({ timeout: 30_000 });
  await expect(page.getByText("再接続するまで操作できません。")).toHaveCount(0);
});

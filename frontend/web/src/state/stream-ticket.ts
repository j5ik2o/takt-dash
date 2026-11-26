// 状態の配信（/api/state）の URL に載せる、画面の識別子と試行の番号（issue #385）。
//
// 画面は openStateStream 1 回につき識別子を 1 つ作り、開くたびに番号を 1 増やす。サーバーは
// 同じ画面の新しい試行が届いたら古い接続を閉じ、遅れて届いた古い試行は断る。画面と
// サーバーが同じ形を使うよう、作る側と読む側をここに置く。

export type StreamTicket = { screen: string; attempt: number };

const SCREEN_ID_BYTES = 16;
const SCREEN_PATTERN = /^[0-9a-f]{32}$/;
const ATTEMPT_PATTERN = /^[1-9][0-9]{0,15}$/;

/**
 * 画面の識別子（16 バイトの乱数の 16 進 32 文字）。LAN の http:// は安全なコンテキストでは
 * なく crypto.randomUUID が無いため、どこでも使える getRandomValues で作る。
 */
export function createScreenId(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(SCREEN_ID_BYTES));
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

export function withStreamTicket(url: string, ticket: StreamTicket): string {
  const separator = url.includes("?") ? "&" : "?";
  return `${url}${separator}screen=${ticket.screen}&attempt=${ticket.attempt}`;
}

/** 形の合う識別子と番号だけを読む。無いか形が違えば undefined（付け替えの対象にしない）。 */
export function parseStreamTicket(params: URLSearchParams): StreamTicket | undefined {
  const screen = params.get("screen");
  const attemptText = params.get("attempt");
  if (screen === null || attemptText === null) {
    return undefined;
  }
  if (!SCREEN_PATTERN.test(screen) || !ATTEMPT_PATTERN.test(attemptText)) {
    return undefined;
  }
  const attempt = Number(attemptText);
  return Number.isSafeInteger(attempt) ? { screen, attempt } : undefined;
}

import { parseStreamTicket, type StreamTicket } from "../../src/state/stream-ticket.ts";

// 画面が開いた状態の配信の URL を、パスと、識別子・試行の番号に分ける（issue #385）。
export const parseStreamUrl = (url: string): { path: string; ticket: StreamTicket | undefined } => {
  const parsed = new URL(url, "http://localhost");
  return { path: parsed.pathname, ticket: parseStreamTicket(parsed.searchParams) };
};

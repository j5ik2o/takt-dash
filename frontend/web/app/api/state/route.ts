import type { NextRequest } from "next/server";
import { DashboardState } from "../../../src/proto/taktdash";
import { getBackendClient, markBackendResponded } from "../../../src/grpc/client";
import { createScreenStreams } from "../../../src/server/screen-streams";
import { relayStateStream } from "../../../src/server/state-relay";
import { parseStreamTicket } from "../../../src/state/stream-ticket";

export const dynamic = "force-dynamic";

const SSE_HEADERS = {
  "Content-Type": "text/event-stream; charset=utf-8",
  "Cache-Control": "no-cache, no-transform",
  Connection: "keep-alive",
} as const;

const toSseEvent = (state: DashboardState): Uint8Array =>
  new TextEncoder().encode(`data: ${JSON.stringify(DashboardState.toJSON(state))}\n\n`);

// 画面ごとに開いている接続の登録簿。プロセスで 1 つ（issue #385）。
const screenStreams = createScreenStreams();

export async function GET(request: NextRequest): Promise<Response> {
  const ticket = parseStreamTicket(request.nextUrl.searchParams);
  const body = relayStateStream({
    // 共有クライアントはストリームを閉じても破棄しない。SSE ごとに作成・close しない。
    openStream: () => getBackendClient().watchState(),
    signal: request.signal,
    admit: ticket === undefined ? undefined : (close) => screenStreams.admit(ticket, close),
    encode: toSseEvent,
    onState: markBackendResponded,
  });
  if (body === undefined) {
    // 同じ画面のもっと新しい試行が開いている。この要求の EventSource は画面がもう閉じている。
    return new Response(null, { status: 204, headers: { "Cache-Control": "no-cache, no-transform" } });
  }
  return new Response(body, { headers: SSE_HEADERS });
}

// Next は HEAD が無いと GET で代わりに処理する。HEAD で配信中の接続を付け替えたり、読まれない
// 本文のために backend を購読したりしないよう、本文を持たない応答を返す（PR #389 のレビュー）。
export function HEAD(): Response {
  return new Response(null, { headers: SSE_HEADERS });
}

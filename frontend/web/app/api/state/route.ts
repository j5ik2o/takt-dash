import type { NextRequest } from "next/server";
import { DashboardState } from "../../../src/proto/taktdash";
import { createBackendClient } from "../../../src/grpc/client";

export const dynamic = "force-dynamic";

const SSE_HEADERS = {
  "Content-Type": "text/event-stream; charset=utf-8",
  "Cache-Control": "no-cache, no-transform",
  Connection: "keep-alive",
} as const;

const toSseEvent = (state: DashboardState): Uint8Array =>
  new TextEncoder().encode(`data: ${JSON.stringify(DashboardState.toJSON(state))}\n\n`);

export async function GET(request: NextRequest): Promise<Response> {
  const client = createBackendClient();
  const stream = client.watchState();
  const signal = request.signal;

  const body = new ReadableStream<Uint8Array>({
    start(controller) {
      let finished = false;
      const finish = () => {
        if (finished) {
          return;
        }
        finished = true;
        signal.removeEventListener("abort", finish);
        stream.cancel();
        stream.destroy();
        void client.close();
        try {
          controller.close();
        } catch {
          // 既に閉じている場合は何もしない。
        }
      };

      if (signal.aborted) {
        finish();
        return;
      }

      signal.addEventListener("abort", finish);
      stream.on("data", (state: DashboardState) => {
        if (finished) {
          return;
        }
        try {
          controller.enqueue(toSseEvent(state));
        } catch {
          finish();
        }
      });
      stream.on("error", finish);
      stream.on("end", () => {
        if (finished) {
          return;
        }
        finished = true;
        void client.close();
        try {
          controller.close();
        } catch {
          // 既に閉じている場合は何もしない。
        }
      });
    },
    cancel() {
      stream.cancel();
      stream.destroy();
      void client.close();
    },
  });

  return new Response(body, { headers: SSE_HEADERS });
}

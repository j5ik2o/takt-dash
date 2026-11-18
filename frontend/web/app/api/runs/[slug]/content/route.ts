import type { NextRequest } from "next/server";
import { getBackendClient, RunContentNotFoundError, type RunContentTarget } from "../../../../../src/grpc/client";
import { GetRunContentResponse } from "../../../../../src/proto/taktdash";
import {
  DomainPrimitiveError,
  toInstructionId,
  toReportFilename,
  toRunSlug,
  type RunSlug,
} from "../../../../../src/state/domain-primitives";
import { toBackendUnavailableResponse } from "../../../error-response";

export const dynamic = "force-dynamic";

type RunContentRouteContext = {
  params: Promise<{ slug: string }>;
};

const NO_STORE = { "Cache-Control": "no-store" };

const errorResponse = (message: string, status: number): Response =>
  Response.json({ error: message }, { status, headers: NO_STORE });

// ?report= と ?instruction= のどちらか一方だけを受ける。指示書の ID は 10 進の正の整数に限る。
const parseTarget = (params: URLSearchParams): RunContentTarget | undefined => {
  const report = params.get("report");
  const instruction = params.get("instruction");
  if ((report === null) === (instruction === null)) {
    return undefined;
  }
  if (report !== null) {
    return { report: toReportFilename(report) };
  }
  if (instruction === null || !/^\d+$/.test(instruction)) {
    return undefined;
  }
  return { instruction: toInstructionId(Number(instruction)) };
};

const parseRequest = (slug: string, params: URLSearchParams): { runSlug: RunSlug; target: RunContentTarget } | undefined => {
  try {
    const runSlug = toRunSlug(slug);
    const target = parseTarget(params);
    return target === undefined ? undefined : { runSlug, target };
  } catch (error) {
    if (error instanceof DomainPrimitiveError) {
      return undefined;
    }
    throw error;
  }
};

/** run のレポートか指示書の本文を、開いたときに1件だけ返す。 */
export async function GET(request: NextRequest, context: RunContentRouteContext): Promise<Response> {
  const { slug } = await context.params;
  const parsed = parseRequest(slug, request.nextUrl.searchParams);
  if (parsed === undefined) {
    return errorResponse("本文の対象は report か instruction のどちらか一方を正しく指定してください。", 400);
  }
  try {
    const content = await getBackendClient().getRunContent(parsed.runSlug, parsed.target);
    return Response.json(GetRunContentResponse.toJSON(content), { headers: NO_STORE });
  } catch (error) {
    if (error instanceof RunContentNotFoundError) {
      return errorResponse("本文が見つかりません。", 404);
    }
    return toBackendUnavailableResponse(error);
  }
}

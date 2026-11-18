import type { NextRequest } from "next/server";
import { getBackendClient, RunNotFoundError } from "../../../../../src/grpc/client";
import { GetRunUpdatesResponse } from "../../../../../src/proto/taktdash";
import { DomainPrimitiveError, toRunSlug, type RunSlug } from "../../../../../src/state/domain-primitives";
import type { RunLogCursor } from "../../../../../src/state/run-log-stream";
import { toBackendUnavailableResponse } from "../../../error-response";

export const dynamic = "force-dynamic";

type RunUpdatesRouteContext = {
  params: Promise<{ slug: string }>;
};

const NO_STORE = { "Cache-Control": "no-store" };

const errorResponse = (message: string, status: number): Response =>
  Response.json({ error: message }, { status, headers: NO_STORE });

const DECIMAL = /^\d+$/;

// ?epoch= と ?after= は両方あるか両方無いかのどちらかで、どちらも 10 進の負でない整数に限る。
// 位置が無いことは undefined、指定の誤りは null で返す。
const parseCursor = (params: URLSearchParams): RunLogCursor | undefined | null => {
  const epoch = params.get("epoch");
  const after = params.get("after");
  if (epoch === null && after === null) {
    return undefined;
  }
  if (epoch === null || after === null || !DECIMAL.test(epoch) || !DECIMAL.test(after)) {
    return null;
  }
  const cursor = { epoch: Number(epoch), sequence: Number(after) };
  return Number.isSafeInteger(cursor.epoch) && Number.isSafeInteger(cursor.sequence) ? cursor : null;
};

const parseRunSlug = (slug: string): RunSlug | undefined => {
  try {
    return toRunSlug(slug);
  } catch (error) {
    if (error instanceof DomainPrimitiveError) {
      return undefined;
    }
    throw error;
  }
};

/** 開いている run 詳細のログの追記分を、前回の続きの位置から返す。 */
export async function GET(request: NextRequest, context: RunUpdatesRouteContext): Promise<Response> {
  const { slug } = await context.params;
  const runSlug = parseRunSlug(slug);
  const cursor = parseCursor(request.nextUrl.searchParams);
  if (runSlug === undefined || cursor === null) {
    return errorResponse("run と続きの位置（epoch と after の両方か、どちらも無し）を正しく指定してください。", 400);
  }
  try {
    const updates = await getBackendClient().getRunUpdates(runSlug, cursor);
    return Response.json(GetRunUpdatesResponse.toJSON(updates), { headers: NO_STORE });
  } catch (error) {
    if (error instanceof RunNotFoundError) {
      return errorResponse("run が見つかりません。", 404);
    }
    return toBackendUnavailableResponse(error);
  }
}

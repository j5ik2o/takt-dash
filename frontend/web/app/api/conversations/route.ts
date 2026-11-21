import type { NextRequest } from "next/server";
import { BackendUnavailableError, getBackendClient } from "../../../src/grpc/client";
import { GetConversationResponse } from "../../../src/proto/taktdash";

export const dynamic = "force-dynamic";

const DEFAULT_AFTER_SEQ = 0;
const DEFAULT_LIMIT = 200;

class BadRequestError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "BadRequestError";
  }
}

const integerParam = (raw: string | null, name: string, fallback: number, minimum: number): number => {
  if (raw === null || raw === "") {
    return fallback;
  }
  if (!/^-?\d+$/.test(raw)) {
    throw new BadRequestError(`${name} は整数で指定してください。`);
  }
  const value = Number(raw);
  if (!Number.isSafeInteger(value) || value < minimum) {
    throw new BadRequestError(`${name} は ${minimum} 以上の整数で指定してください。`);
  }
  return value;
};

const errorResponse = (message: string, status: number): Response =>
  Response.json({ error: message }, { status, headers: { "Cache-Control": "no-store" } });

export async function GET(request: NextRequest): Promise<Response> {
  const params = request.nextUrl.searchParams;
  const project = (params.get("project") ?? "").trim();
  if (project === "") {
    return errorResponse("project は必須です。", 400);
  }

  let afterSeq: number;
  let limit: number;
  try {
    afterSeq = integerParam(params.get("after_seq"), "after_seq", DEFAULT_AFTER_SEQ, 0);
    limit = integerParam(params.get("limit"), "limit", DEFAULT_LIMIT, 1);
  } catch (error) {
    if (error instanceof BadRequestError) {
      return errorResponse(error.message, 400);
    }
    throw error;
  }

  const sessionParam = params.get("session_id");
  const sessionId = sessionParam !== null && sessionParam !== "" ? sessionParam : undefined;

  const client = getBackendClient();
  try {
    const response = await client.getConversation({ project, sessionId, afterSeq, limit });
    // toJSON は既定値（false・0）のフィールドを省略する。画面の差分取得は has_more と
    // next_after_seq を毎回読むため、応答の JSON では省略せずに返す。
    const json = GetConversationResponse.toJSON(response) as Record<string, unknown>;
    json.hasMore = response.hasMore;
    json.nextAfterSeq = response.nextAfterSeq;
    return Response.json(json, { headers: { "Cache-Control": "no-store" } });
  } catch (error) {
    if (error instanceof BackendUnavailableError) {
      return errorResponse(error.message, 503);
    }
    throw error;
  }
}

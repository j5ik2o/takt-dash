// 判断待ちの書き込み Route Handler の共通処理。
//
// 入力の形だけを検証し、値はそのまま gRPC の書き込み API へ渡す。業務的な検証
// （ゴールの所属、pull_request の有無など）は backend に任せ、ここでは重複しない。
import * as grpc from "@grpc/grpc-js";
import { BackendUnavailableError, createBackendClient, type BackendClient } from "../../../src/grpc/client";

const JSON_HEADERS = { "content-type": "application/json; charset=utf-8" } as const;

const jsonResponse = (status: number, body: unknown): Response =>
  new Response(JSON.stringify(body), { status, headers: JSON_HEADERS });

export const badRequest = (message: string): Response => jsonResponse(400, { ok: false, error: message });

// 本文を読む前の境界検証。ブラウザの別オリジンや cross-site からの書き込み、および
// JSON でない本文を受け付けない。ヘッダの無い CLI 等の既存利用は許可する。
const unsupportedMediaType = (): Response =>
  jsonResponse(415, { ok: false, error: "Content-Type は application/json で指定してください。" });

const forbiddenOrigin = (): Response =>
  jsonResponse(403, { ok: false, error: "このオリジンからは操作できません。" });

const contentTypeIsJson = (request: Request): boolean => {
  const header = request.headers.get("content-type");
  if (header === null) {
    return false;
  }
  const [mediaType] = header.split(";");
  return mediaType.trim().toLowerCase() === "application/json";
};

const requestOriginIsAllowed = (request: Request): boolean => {
  if (request.headers.get("sec-fetch-site") === "cross-site") {
    return false;
  }
  const origin = request.headers.get("origin");
  if (origin === null) {
    return true;
  }
  try {
    return new URL(origin).origin === new URL(request.url).origin;
  } catch {
    return false;
  }
};

export type ReadJsonObjectResult =
  | { kind: "ok"; body: Record<string, unknown> }
  | { kind: "error"; response: Response };

export async function readJsonObject(request: Request): Promise<ReadJsonObjectResult> {
  if (!contentTypeIsJson(request)) {
    return { kind: "error", response: unsupportedMediaType() };
  }
  if (!requestOriginIsAllowed(request)) {
    return { kind: "error", response: forbiddenOrigin() };
  }
  try {
    const value: unknown = await request.json();
    if (typeof value !== "object" || value === null || Array.isArray(value)) {
      return {
        kind: "error",
        response: badRequest("リクエストの本文が JSON オブジェクトではありません。"),
      };
    }
    return { kind: "ok", body: value as Record<string, unknown> };
  } catch {
    return {
      kind: "error",
      response: badRequest("リクエストの本文が JSON オブジェクトではありません。"),
    };
  }
}

export const isNonEmptyString = (value: unknown): value is string =>
  typeof value === "string" && value.trim() !== "";

export const isPositiveInteger = (value: unknown): value is number =>
  typeof value === "number" && Number.isInteger(value) && value > 0;

// 書き込みを実行し、成功・入力不正・接続不能を HTTP の応答へ一度だけ変換する。
export async function runWrite(call: (client: BackendClient) => Promise<unknown>): Promise<Response> {
  const client = createBackendClient();
  try {
    await call(client);
    return jsonResponse(200, { ok: true });
  } catch (error) {
    if (error instanceof BackendUnavailableError) {
      // backend が応答の中で UNAVAILABLE を返した場合は、接続障害ではないため
      // 接続できない旨に置き換えず、backend が返した理由を伝える。
      if (error.kind === "server") {
        return jsonResponse(502, {
          ok: false,
          error: error.details !== "" ? error.details : "backend が操作を受け付けられませんでした。",
        });
      }
      return jsonResponse(503, { ok: false, error: "backend に接続できません。" });
    }
    const serviceError = error as grpc.ServiceError;
    if (serviceError.code === grpc.status.INVALID_ARGUMENT) {
      return jsonResponse(400, { ok: false, error: serviceError.details || "入力が正しくありません。" });
    }
    return jsonResponse(500, { ok: false, error: "操作に失敗しました。" });
  } finally {
    await client.close();
  }
}

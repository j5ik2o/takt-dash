import { BackendUnavailableError } from "../../src/grpc/client";

// BFF の Route Handler が共通で使う、backend へ接続できない場合の 503 応答。
// 対象外の例外は呼び出し元のハンドラへそのまま伝え、404 などの固有分岐は
// 各ハンドラに残す。
export function toBackendUnavailableResponse(error: unknown): Response {
  if (error instanceof BackendUnavailableError) {
    return Response.json({ error: "backend に接続できません。" }, { status: 503 });
  }
  throw error;
}

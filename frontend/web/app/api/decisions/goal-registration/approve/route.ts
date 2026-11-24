import type { NextRequest } from "next/server";
import { badRequest, isNonEmptyString, isPositiveInteger, readIdempotencyKey, readJsonObject, runWrite } from "../../_shared";

// ゴール登録の承認。revision は判断待ち一覧の項目が持つ値をそのまま渡す。
export async function POST(request: NextRequest): Promise<Response> {
  const read = await readJsonObject(request);
  if (read.kind === "error") {
    return read.response;
  }
  const body = read.body;

  const { serverId, project, revision } = body;
  if (!isNonEmptyString(serverId)) {
    return badRequest("serverId がありません。");
  }
  if (!isNonEmptyString(project)) {
    return badRequest("project がありません。");
  }
  if (!isPositiveInteger(revision)) {
    return badRequest("revision は正の整数で指定してください。");
  }

  const key = readIdempotencyKey(body);
  if (key.kind === "error") {
    return key.response;
  }
  const { idempotencyKey } = key;

  return runWrite((client) => client.approveGoalRegistration({ serverId, project, revision, idempotencyKey }));
}

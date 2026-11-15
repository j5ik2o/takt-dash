import type { NextRequest } from "next/server";
import { badRequest, isNonEmptyString, isPositiveInteger, readJsonObject, runWrite } from "../../_shared";

// ゴール登録の却下。revision は判断待ち一覧の項目が持つ値をそのまま渡す。
export async function POST(request: NextRequest): Promise<Response> {
  const body = await readJsonObject(request);
  if (body === undefined) {
    return badRequest("リクエストの本文が JSON オブジェクトではありません。");
  }

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

  return runWrite((client) => client.rejectGoalRegistration({ serverId, project, revision }));
}

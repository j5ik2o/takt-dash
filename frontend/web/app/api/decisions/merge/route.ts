import type { NextRequest } from "next/server";
import { badRequest, isNonEmptyString, isPositiveInteger, readJsonObject, runWrite } from "../_shared";

// マージの承認。until は Web の操作契約として "merge" に固定する。
export async function POST(request: NextRequest): Promise<Response> {
  const body = await readJsonObject(request);
  if (body === undefined) {
    return badRequest("リクエストの本文が JSON オブジェクトではありません。");
  }

  const { serverId, project, goalId, prNumber } = body;
  if (!isNonEmptyString(serverId)) {
    return badRequest("serverId がありません。");
  }
  if (!isNonEmptyString(project)) {
    return badRequest("project がありません。");
  }
  if (!isNonEmptyString(goalId)) {
    return badRequest("goalId がありません。");
  }
  if (!isPositiveInteger(prNumber)) {
    return badRequest("prNumber は正の整数で指定してください。");
  }

  return runWrite((client) =>
    client.approveGoalMerge({ serverId, project, goalId, prNumber, until: "merge" }),
  );
}

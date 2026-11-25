import type { NextRequest } from "next/server";
import { getBackendClient, TaskNotFoundError } from "../../../../src/grpc/client";
import { GetTaskDetailResponse } from "../../../../src/proto/taktdash";
import { lastTaskDetails, taskDetailKey } from "../../../../src/state/last-loaded";
import { toBackendUnavailableResponse } from "../../error-response";

export const dynamic = "force-dynamic";

type TaskDetailRouteContext = {
  params: Promise<{ project: string }>;
};

const notFound = (): Response => Response.json({ error: "タスクが見つかりません。" }, { status: 404 });
const badRequest = (): Response => Response.json({ error: "タスク名が指定されていません。" }, { status: 400 });

export async function GET(request: NextRequest, context: TaskDetailRouteContext): Promise<Response> {
  const { project } = await context.params;
  const taskName = request.nextUrl.searchParams.get("task");
  if (taskName === null || taskName.trim() === "") {
    return badRequest();
  }
  const goalId = request.nextUrl.searchParams.get("goal") ?? undefined;
  const client = getBackendClient();
  try {
    const response = await client.getTaskDetail(project, taskName, goalId);
    // 切断中にページを開き直したとき、この更新で読めた内容を出す（issue #309）。
    lastTaskDetails.remember(taskDetailKey(project, taskName, goalId), response);
    return Response.json(GetTaskDetailResponse.toJSON(response));
  } catch (error) {
    if (error instanceof TaskNotFoundError) {
      return notFound();
    }
    return toBackendUnavailableResponse(error);
  }
}

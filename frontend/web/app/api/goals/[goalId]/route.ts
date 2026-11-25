import { getBackendClient, GoalNotFoundError } from "../../../../src/grpc/client";
import { GetGoalDetailResponse } from "../../../../src/proto/taktdash";
import { lastGoalDetails } from "../../../../src/state/last-loaded";
import { toBackendUnavailableResponse } from "../../error-response";

export const dynamic = "force-dynamic";

type GoalDetailRouteContext = {
  params: Promise<{ goalId: string }>;
};

const notFound = (): Response => Response.json({ error: "ゴールが見つかりません。" }, { status: 404 });

export async function GET(_request: Request, context: GoalDetailRouteContext): Promise<Response> {
  const { goalId } = await context.params;
  const client = getBackendClient();
  try {
    const response = await client.getGoalDetail(goalId);
    // 切断中にページを開き直したとき、この更新で読めた内容を出す（issue #309）。
    lastGoalDetails.remember(goalId, response);
    return Response.json(GetGoalDetailResponse.toJSON(response));
  } catch (error) {
    if (error instanceof GoalNotFoundError) {
      return notFound();
    }
    return toBackendUnavailableResponse(error);
  }
}

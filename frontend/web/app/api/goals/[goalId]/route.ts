import { getBackendClient, GoalNotFoundError } from "../../../../src/grpc/client";
import { GetGoalDetailResponse } from "../../../../src/proto/taktdash";
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
    return Response.json(GetGoalDetailResponse.toJSON(response));
  } catch (error) {
    if (error instanceof GoalNotFoundError) {
      return notFound();
    }
    return toBackendUnavailableResponse(error);
  }
}

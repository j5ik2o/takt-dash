import type { ReactElement } from "react";
import { notFound } from "next/navigation";
import {
  BackendUnavailableError,
  backendUnavailableSituation,
  createBackendClient,
  GoalNotFoundError,
} from "../grpc/client";
import type { GetGoalDetailResponse } from "../proto/taktdash";
import { BackendUnavailableNotice } from "./backend-unavailable-notice";
import { LiveGoalDetail } from "./live-goal-detail";

type GoalDetailLoaderProps = {
  params: Promise<{ goalId: string }>;
};

type LoadResult =
  | { kind: "found"; detail: GetGoalDetailResponse }
  | { kind: "unavailable"; error: BackendUnavailableError };

// ゴールが見つからないときは 404 を返す。gRPC の NOT_FOUND と応答の goalId 不一致は、
// どちらも GoalNotFoundError として扱う。backend 自体に接続できないときはページを
// 落とさず、接続できない旨を表示する。
export async function GoalDetailLoader({ params }: GoalDetailLoaderProps): Promise<ReactElement> {
  const { goalId } = await params;
  const client = createBackendClient();
  let result: LoadResult;
  try {
    result = { kind: "found", detail: await client.getGoalDetail(goalId) };
  } catch (error) {
    if (error instanceof GoalNotFoundError) {
      notFound();
    }
    if (error instanceof BackendUnavailableError) {
      result = { kind: "unavailable", error };
    } else {
      throw error;
    }
  } finally {
    await client.close();
  }
  if (result.kind === "unavailable") {
    return <BackendUnavailableNotice situation={backendUnavailableSituation(result.error)} />;
  }
  return <LiveGoalDetail initialDetail={result.detail} goalId={goalId} />;
}

import type { ReactElement } from "react";
import { notFound } from "next/navigation";
import {
  BackendUnavailableError,
  backendUnavailableSituation,
  getBackendClient,
  GoalNotFoundError,
} from "../grpc/client";
import type { GetGoalDetailResponse } from "../proto/taktdash";
import { lastGoalDetails } from "../state/last-loaded";
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
// 落とさず、接続できない旨を表示する。前に読めたゴールなら、見出しと内容を残して
// その上に切断の通知を出す（issue #309）。通知は 1 秒更新の表示が持ち、次の取得が
// 成功したら消す。

export async function GoalDetailLoader({ params }: GoalDetailLoaderProps): Promise<ReactElement> {
  const { goalId } = await params;
  const client = getBackendClient();
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
  }
  if (result.kind === "unavailable") {
    const last = lastGoalDetails.recall(goalId);
    if (last === undefined) {
      return <BackendUnavailableNotice situation={backendUnavailableSituation(result.error)} />;
    }
    return <LiveGoalDetail initialDetail={last} goalId={goalId} initialDisconnected />;
  }
  lastGoalDetails.remember(goalId, result.detail);
  return <LiveGoalDetail initialDetail={result.detail} goalId={goalId} />;
}

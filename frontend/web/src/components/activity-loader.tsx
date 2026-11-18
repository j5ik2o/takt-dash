import type { ReactElement } from "react";
import { BackendUnavailableError, backendUnavailableSituation, createBackendClient } from "../grpc/client";
import type { GetActivityResponse } from "../proto/taktdash";
import { BackendUnavailableNotice } from "./backend-unavailable-notice";
import { ActivityContent } from "./activity-content";

// アクティビティは出来事の一覧と日次集計を GetActivity から取得する。自動更新の状態
// （DashboardState）には含まれないため、サーバー側で取得して表示する。backend に
// 接続できないときはページを落とさず、接続できない旨を表示する（order.md:21）。
export async function ActivityLoader(): Promise<ReactElement> {
  const client = createBackendClient();
  let result:
    | { kind: "ok"; response: GetActivityResponse }
    | { kind: "unavailable"; error: BackendUnavailableError };
  try {
    result = { kind: "ok", response: await client.getActivity() };
  } catch (error) {
    if (!(error instanceof BackendUnavailableError)) {
      throw error;
    }
    result = { kind: "unavailable", error };
  } finally {
    await client.close();
  }
  if (result.kind === "unavailable") {
    return <BackendUnavailableNotice situation={backendUnavailableSituation(result.error)} />;
  }
  return <ActivityContent response={result.response} now={new Date()} />;
}

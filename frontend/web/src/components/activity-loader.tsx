import type { ReactElement } from "react";
import { BackendUnavailableError, getBackendClient } from "../grpc/client";
import type { GetActivityResponse } from "../proto/taktdash";
import { loadServerState } from "../state/server-state";
import { measurePage } from "../telemetry/timing";
import { BackendUnavailableNotice } from "./backend-unavailable-notice";
import { ActivityContent } from "./activity-content";

type ActivityResult = { kind: "ok"; response: GetActivityResponse } | { kind: "unavailable" };

// アクティビティは出来事の一覧と日次集計を GetActivity から取得し、サーバー名の解決には
// 要求内で共有する状態を使う。backend に接続できないときはページを落とさず、接続できない
// 旨を表示する（order.md:21）。
export async function ActivityLoader(): Promise<ReactElement> {
  return measurePage("/activity", async (timing) => {
    const shared = await loadServerState();
    let result: ActivityResult;
    try {
      result = { kind: "ok", response: await getBackendClient().getActivity(timing) };
    } catch (error) {
      if (!(error instanceof BackendUnavailableError)) {
        throw error;
      }
      result = { kind: "unavailable" };
    }
    if (shared.unavailable || result.kind === "unavailable") {
      return <BackendUnavailableNotice />;
    }
    return <ActivityContent state={shared.state} response={result.response} now={new Date()} />;
  });
}

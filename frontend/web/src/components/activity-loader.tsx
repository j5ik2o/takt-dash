import type { ReactElement } from "react";
import { BackendUnavailableError, backendUnavailableSituation, getBackendClient } from "../grpc/client";
import type { GetActivityResponse } from "../proto/taktdash";
import { createLastLoaded } from "../state/last-loaded";
import { loadServerState } from "../state/server-state";
import { measurePage } from "../telemetry/timing";
import { BackendUnavailableOver } from "./backend-unavailable-notice";
import { ActivityContent } from "./activity-content";

const lastActivity = createLastLoaded<GetActivityResponse>(1);
const ACTIVITY_KEY = "activity";

// アクティビティは出来事の一覧と日次集計を GetActivity から取得し、サーバー名の解決には
// 要求内で共有する状態を使う。backend に接続できないときはページを落とさず、接続できない
// 旨を表示する（order.md:21）。前に読めた内容があれば、見出しと内容を残してその上に
// 通知を出す（issue #309）。
export async function ActivityLoader(): Promise<ReactElement> {
  return measurePage("/activity", async (timing) => {
    const shared = await loadServerState();
    const render = (response: GetActivityResponse): ReactElement => (
      <ActivityContent state={shared.state} response={response} now={new Date()} />
    );
    const withNotice = (error: BackendUnavailableError, response: GetActivityResponse | undefined): ReactElement => (
      <BackendUnavailableOver situation={backendUnavailableSituation(error)}>
        {shared.snapshotLoaded && response !== undefined ? render(response) : null}
      </BackendUnavailableOver>
    );
    let response: GetActivityResponse;
    try {
      response = await getBackendClient().getActivity(timing);
    } catch (error) {
      if (!(error instanceof BackendUnavailableError)) {
        throw error;
      }
      return withNotice(shared.error ?? error, lastActivity.recall(ACTIVITY_KEY));
    }
    lastActivity.remember(ACTIVITY_KEY, response);
    return shared.error === undefined ? render(response) : withNotice(shared.error, response);
  });
}

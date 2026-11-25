import type { ReactElement } from "react";
import { BackendUnavailableError, backendUnavailableSituation, getBackendClient } from "../grpc/client";
import type { GetActivityResponse } from "../proto/taktdash";
import { createLastLoaded } from "../state/last-loaded";
import { loadServerState } from "../state/server-state";
import { measurePage } from "../telemetry/timing";
import { BackendUnavailableOver } from "./backend-unavailable-notice";
import { ActivityContent } from "./activity-content";

// 読めた時刻も一緒に覚え、切断中は最終更新にその時刻を出す。
const lastActivity = createLastLoaded<{ response: GetActivityResponse; readAt: Date }>(1);
const ACTIVITY_KEY = "activity";

// アクティビティは出来事の一覧と日次集計を GetActivity から取得し、サーバー名の解決には
// 要求内で共有する状態を使う。backend に接続できないときはページを落とさず、接続できない
// 旨を表示する（order.md:21）。前に読めた内容があれば、見出しと内容を残してその上に
// 通知を出す（issue #309）。
export async function ActivityLoader(): Promise<ReactElement> {
  return measurePage("/activity", async (timing) => {
    const shared = await loadServerState();
    const render = (
      error: BackendUnavailableError | undefined,
      loaded: { response: GetActivityResponse; readAt: Date } | undefined,
    ): ReactElement => (
      <BackendUnavailableOver situation={error === undefined ? undefined : backendUnavailableSituation(error)}>
        {loaded !== undefined && (error === undefined || shared.snapshotLoaded) ? (
          <ActivityContent state={shared.state} response={loaded.response} now={loaded.readAt} />
        ) : null}
      </BackendUnavailableOver>
    );
    let response: GetActivityResponse;
    try {
      response = await getBackendClient().getActivity(timing);
    } catch (error) {
      if (!(error instanceof BackendUnavailableError)) {
        throw error;
      }
      return render(shared.error ?? error, lastActivity.recall(ACTIVITY_KEY));
    }
    const loaded = { response, readAt: new Date() };
    lastActivity.remember(ACTIVITY_KEY, loaded);
    return render(shared.error, loaded);
  });
}

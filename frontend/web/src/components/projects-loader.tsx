import type { ReactElement } from "react";
import { backendUnavailableSituation } from "../grpc/client";
import { loadServerState } from "../state/server-state";
import { measurePage } from "../telemetry/timing";
import { BackendUnavailableOver } from "./backend-unavailable-notice";
import { ProjectsContent } from "./projects-content";

// プロジェクトは要求内で共有する状態から描画する。layout の GetOverview を再利用するため、
// ページ自身は RPC を発行しない（AC72F-1）。backend に接続できないときは、layout と同じ
// 不通の旨を表示する（order.md:21）。前に読めた状態があれば、見出しと内容を残して
// その上に通知を出す（issue #309）。
export async function ProjectsLoader(): Promise<ReactElement> {
  return measurePage("/projects", async () => {
    const { state, snapshotLoaded, error } = await loadServerState();
    return (
      <BackendUnavailableOver situation={error === undefined ? undefined : backendUnavailableSituation(error)}>
        {error === undefined || snapshotLoaded ? <ProjectsContent state={state} now={new Date()} /> : null}
      </BackendUnavailableOver>
    );
  });
}

import type { ReactElement } from "react";
import { loadServerState } from "../state/server-state";
import { measurePage } from "../telemetry/timing";
import { BackendUnavailableNotice } from "./backend-unavailable-notice";
import { ProjectsContent } from "./projects-content";

// プロジェクトは要求内で共有する状態から描画する。layout の GetOverview を再利用するため、
// ページ自身は RPC を発行しない（AC72F-1）。backend に接続できないときは、layout と同じ
// 不通の旨を表示する（order.md:21）。
export async function ProjectsLoader(): Promise<ReactElement> {
  return measurePage("/projects", async () => {
    const { state, unavailable } = await loadServerState();
    if (unavailable) {
      return <BackendUnavailableNotice />;
    }
    return <ProjectsContent state={state} now={new Date()} />;
  });
}

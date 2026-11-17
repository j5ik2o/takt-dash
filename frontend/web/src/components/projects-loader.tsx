import type { ReactElement } from "react";
import { BackendUnavailableError, backendUnavailableSituation, createBackendClient } from "../grpc/client";
import type { GetProjectsResponse } from "../proto/taktdash";
import { BackendUnavailableNotice } from "./backend-unavailable-notice";
import { ProjectsContent } from "./projects-content";

// プロジェクトは直近の出来事を含む GetProjects を使うため、サーバー側で取得して表示する。
// backend に接続できないときはページを落とさず、接続できない旨を表示する（order.md:21）。
export async function ProjectsLoader(): Promise<ReactElement> {
  const client = createBackendClient();
  let result:
    | { kind: "ok"; response: GetProjectsResponse }
    | { kind: "unavailable"; error: BackendUnavailableError };
  try {
    result = { kind: "ok", response: await client.getProjects() };
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
  return <ProjectsContent response={result.response} now={new Date()} />;
}

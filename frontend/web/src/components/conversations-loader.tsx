import type { ReactElement } from "react";
import { BackendUnavailableError, backendUnavailableSituation, getBackendClient } from "../grpc/client";
import { GetConversationResponse } from "../proto/taktdash";
import { restoreConversationResponse, type ConversationPage } from "../state/conversation";
import { BackendUnavailableNotice } from "./backend-unavailable-notice";
import { LiveConversation } from "./live-conversation";

type LoadResult =
  | { kind: "ok"; projects: string[]; page: ConversationPage }
  | { kind: "unavailable"; error: BackendUnavailableError }
  | { kind: "empty" };

// プロジェクト一覧から先頭の会話を取得し、1秒更新の表示へ渡す。
// backend に接続できないときはページを落とさず、接続できない旨を表示する。
export async function ConversationsLoader(): Promise<ReactElement> {
  const client = getBackendClient();
  let result: LoadResult;
  try {
    const overview = await client.getOverview();
    const projects = overview.projects.map((project) => project.name);
    if (projects.length === 0) {
      result = { kind: "empty" };
    } else {
      const project = projects[0]!;
      const response = await client.getConversation({ project, afterSeq: 0, limit: 200 });
      result = { kind: "ok", projects, page: restoreConversationResponse(GetConversationResponse.toJSON(response)) };
    }
  } catch (error) {
    if (!(error instanceof BackendUnavailableError)) {
      throw error;
    }
    result = { kind: "unavailable", error };
  }

  switch (result.kind) {
    case "unavailable":
      return <BackendUnavailableNotice situation={backendUnavailableSituation(result.error)} />;
    case "empty":
      return (
        <main className="page">
          <p className="note">プロジェクトがありません。</p>
        </main>
      );
    case "ok":
      return (
        <LiveConversation projects={result.projects} initialProject={result.projects[0]!} initialPage={result.page} />
      );
  }
}

import type { ReactElement } from "react";
import { BackendUnavailableError, backendUnavailableSituation, getBackendClient } from "../grpc/client";
import { GetConversationResponse } from "../proto/taktdash";
import { restoreConversationResponse, type ConversationPage } from "../state/conversation";
import { createLastLoaded } from "../state/last-loaded";
import { BackendUnavailableNotice, backendUnavailableMessage } from "./backend-unavailable-notice";
import { LiveConversation } from "./live-conversation";

type LoadedConversation = { projects: string[]; page: ConversationPage };

type LoadResult =
  | ({ kind: "ok" } & LoadedConversation)
  | { kind: "unavailable"; error: BackendUnavailableError }
  | { kind: "empty" };

// プロジェクト一覧から先頭の会話を取得し、1秒更新の表示へ渡す。
// backend に接続できないときはページを落とさず、接続できない旨を表示する。前に読めた
// 会話があれば、見出しと内容を残して会話の取得失敗の通知を出す（issue #309）。通知は
// 1 秒更新の表示が持ち、次の取得が成功したら消す。
const lastConversation = createLastLoaded<LoadedConversation>(1);
const CONVERSATION_KEY = "conversations";

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
    case "unavailable": {
      const situation = backendUnavailableSituation(result.error);
      const last = lastConversation.recall(CONVERSATION_KEY);
      if (last === undefined) {
        return <BackendUnavailableNotice situation={situation} />;
      }
      return (
        <LiveConversation
          projects={last.projects}
          initialProject={last.projects[0]!}
          initialPage={last.page}
          initialError={backendUnavailableMessage(situation)}
        />
      );
    }
    case "empty":
      return (
        <main className="page">
          <p className="note">プロジェクトがありません。</p>
        </main>
      );
    case "ok":
      lastConversation.remember(CONVERSATION_KEY, { projects: result.projects, page: result.page });
      return (
        <LiveConversation projects={result.projects} initialProject={result.projects[0]!} initialPage={result.page} />
      );
  }
}

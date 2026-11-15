"use client";

import { useEffect, useState, type ReactElement } from "react";
import {
  beginConversationSwitch,
  conversationFailed,
  conversationLoaded,
  restoreConversationResponse,
  type ConversationPage,
  type ConversationRequest,
  type ConversationView,
} from "../state/conversation";
import { createConversationFollow } from "../state/conversation-follow";
import { ConversationContent } from "./conversation-content";

type LiveConversationProps = {
  projects: string[];
  initialProject: string;
  initialPage: ConversationPage;
};

// 会話ページ専用の1秒更新。BFF の Route Handler を経由して差分を取り、
// 同じ画面の状態へ新しい発言だけを足す。セッションは最初の取得の値に固定する。
const requestPage = async (request: ConversationRequest): Promise<ConversationPage> => {
  const params = new URLSearchParams({
    project: request.project,
    after_seq: String(request.afterSeq),
    limit: String(request.limit),
  });
  if (request.sessionId !== undefined && request.sessionId !== "") {
    params.set("session_id", request.sessionId);
  }
  const response = await fetch(`/api/conversations?${params.toString()}`, { cache: "no-store" });
  if (!response.ok) {
    const body = (await response.json().catch(() => ({}))) as { error?: string };
    throw new Error(body.error ?? `会話を取得できません（${response.status}）。`);
  }
  return restoreConversationResponse(await response.json());
};

export function LiveConversation({ projects, initialProject, initialPage }: LiveConversationProps): ReactElement {
  const [view, setView] = useState<ConversationView>({
    project: initialProject,
    page: initialPage,
    loading: false,
  });
  const [follow] = useState(() => createConversationFollow({ initialProject, initialPage, requestPage }));

  // 対象プロジェクトが変わるたびに、その会話を先頭から取得し直してから追従する。
  // 初回取得が失敗しても更新ループは止めず、失敗を警告として残したまま再試行する。
  useEffect(() => {
    let active = true;
    const follower = follow.followerFor(view.project);

    const poll = async (): Promise<void> => {
      const result = await follower.poll();
      if (!active || result === undefined) {
        return;
      }
      if (result.kind === "loaded") {
        setView((previous) => conversationLoaded(previous, result.page));
      } else {
        setView((previous) => conversationFailed(previous, result.error));
      }
    };

    void poll();
    const timer = setInterval(() => {
      void poll();
    }, 1_000);

    return () => {
      active = false;
      clearInterval(timer);
    };
  }, [follow, view.project]);

  return (
    <>
      <div className="conversation-picker">
        <label htmlFor="conversation-project">プロジェクト</label>
        <select
          id="conversation-project"
          value={view.project}
          onChange={(event) => {
            setView(beginConversationSwitch(event.target.value));
          }}
        >
          {projects.map((name) => (
            <option key={name} value={name}>
              {name}
            </option>
          ))}
        </select>
      </div>
      <ConversationContent project={view.project} page={view.page} loading={view.loading} error={view.error} />
    </>
  );
}

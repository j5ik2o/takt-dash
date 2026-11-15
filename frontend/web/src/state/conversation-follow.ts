// 会話ページの差分追随の契約。
//
// 初期表示で取得済みのページは、そのプロジェクトの初回だけ引き継いで先頭200行の
// 再取得を避ける。ポーリングは前回の完了まで次を開始せず、失敗してもカーソルと
// セッションを保って同じ位置から再試行する。
import {
  fetchConversationPages,
  mergeConversationPages,
  type ConversationPage,
  type ConversationRequest,
} from "./conversation";

export type ConversationFollowResult = { kind: "loaded"; page: ConversationPage } | { kind: "failed"; error: string };

export type ConversationFollower = {
  poll: () => Promise<ConversationFollowResult | undefined>;
};

export type ConversationFollow = {
  followerFor: (project: string) => ConversationFollower;
};

export type CreateConversationFollowOptions = {
  initialProject: string;
  initialPage: ConversationPage;
  requestPage: (request: ConversationRequest) => Promise<ConversationPage>;
};

const createFollower = (
  project: string,
  initialPage: ConversationPage | undefined,
  requestPage: (request: ConversationRequest) => Promise<ConversationPage>,
): ConversationFollower => {
  let current = initialPage;
  let sessionId = initialPage?.sessionId;
  let inFlight = false;

  const poll = async (): Promise<ConversationFollowResult | undefined> => {
    if (inFlight) {
      return undefined;
    }
    inFlight = true;
    const base = current;
    try {
      const update = await fetchConversationPages(
        requestPage,
        project,
        base === undefined
          ? {}
          : { sessionId: sessionId === undefined || sessionId === "" ? undefined : sessionId, afterSeq: base.nextAfterSeq },
      );
      const page = base === undefined ? update.page : mergeConversationPages(base, update.page);
      current = page;
      if (sessionId === undefined || sessionId === "") {
        sessionId = update.sessionId;
      }
      return { kind: "loaded", page };
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      return { kind: "failed", error: message };
    } finally {
      inFlight = false;
    }
  };

  return { poll };
};

/**
 * 初期ページの引き継ぎと、プロジェクトごとの差分追随を作る。
 *
 * `initialPage` は `initialProject` の最初の `followerFor` だけで使い、以後の
 * プロジェクト切替では先頭から取得する。
 */
export function createConversationFollow({
  initialProject,
  initialPage,
  requestPage,
}: CreateConversationFollowOptions): ConversationFollow {
  let handedOff = false;

  return {
    followerFor: (project) => {
      if (!handedOff && project === initialProject) {
        handedOff = true;
        return createFollower(project, initialPage, requestPage);
      }
      return createFollower(project, undefined, requestPage);
    },
  };
}

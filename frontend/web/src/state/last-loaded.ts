// 最後に読めたページの内容を、要求をまたいでサーバー側に残す（issue #309）。
// backend に接続できないとき、各ページは前に読めた内容をバナーの下に残して描画する。
// 概要の状態を残す server-state.ts と同じ考え方を、キーごとの内容へ広げたもの。
// run やゴールは増え続けるため、最近使ったものから上限件数だけを残す。

import type { GetGoalDetailResponse, GetTaskDetailResponse } from "../proto/taktdash";
import { mergeConversationPages, type ConversationPage } from "./conversation";

export const LAST_LOADED_LIMIT = 100;

export type LastLoaded<T> = {
  remember: (key: string, value: T) => void;
  recall: (key: string) => T | undefined;
};

export function createLastLoaded<T>(limit = LAST_LOADED_LIMIT): LastLoaded<T> {
  const entries = new Map<string, T>();
  return {
    remember: (key, value) => {
      entries.delete(key);
      entries.set(key, value);
      while (entries.size > limit) {
        const oldest = entries.keys().next().value;
        if (oldest === undefined) {
          break;
        }
        entries.delete(oldest);
      }
    },
    recall: (key) => {
      const value = entries.get(key);
      if (value !== undefined) {
        entries.delete(key);
        entries.set(key, value);
      }
      return value;
    },
  };
}

// ゴールとタスクの詳細は、ページの初回取得（ローダー）と 1 秒更新（Route Handler）の
// 両方で覚える。切断中に開き直したとき、直前まで更新で読めていた内容を出すため。
export const lastGoalDetails = createLastLoaded<GetGoalDetailResponse>();
export const lastTaskDetails = createLastLoaded<GetTaskDetailResponse>();

/** タスクのキー。goalId の有無で別のタスクになるため、未指定を区別して含める。 */
export const taskDetailKey = (project: string, taskName: string, goalId: string | undefined): string =>
  JSON.stringify([project, taskName, goalId ?? null]);

// 会話ページは先頭のプロジェクトの会話を開く。初回取得の内容に、1 秒更新で届いた
// 差分を同じセッションの間だけ足していく。
export type LastConversation = { projects: string[]; page: ConversationPage };

const lastConversations = createLastLoaded<LastConversation>(1);
const CONVERSATION_KEY = "conversations";

export const rememberConversation = (loaded: LastConversation): void =>
  lastConversations.remember(CONVERSATION_KEY, loaded);

export const recallConversation = (): LastConversation | undefined => lastConversations.recall(CONVERSATION_KEY);

/**
 * 1 秒更新で届いた差分を、覚えている会話と同じプロジェクト・セッションのときだけ足す。
 * 別のタブが古い位置から読み直した差分では、続きの位置を戻さない。
 */
export const rememberConversationUpdate = (update: ConversationPage): void => {
  const last = recallConversation();
  if (
    last === undefined ||
    last.page.project !== update.project ||
    last.page.sessionId !== update.sessionId ||
    update.nextAfterSeq < last.page.nextAfterSeq
  ) {
    return;
  }
  rememberConversation({ projects: last.projects, page: mergeConversationPages(last.page, update) });
};

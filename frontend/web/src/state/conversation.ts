// 会話の復元・並び替え・差分取得の契約。
//
// backend が返す GetConversationResponse.toJSON の JSON は camelCase のフィールドと
// ISO 文字列の時刻を持つ。ブラウザ側は生成コードの fromJSON を値として使えないため、
// restoreConversationResponse が内部の型へ復元する。差分取得は after_seq と
// next_after_seq を使い、最初の応答の session_id を固定して同一の会話を追う。
import { Timestamp } from "../proto/google/protobuf/timestamp";
import { CONVERSATION_SPEAKER, enumFromWire } from "./proto-values";

export type ConversationMessageView = {
  seq: number;
  occurredAt?: Date;
  speaker: number;
  text: string;
};

export type ConversationSkippedView = {
  reason: string;
  count: number;
};

export type ConversationPage = {
  project: string;
  sessionId: string;
  provider: string;
  messages: ConversationMessageView[];
  nextAfterSeq: number;
  hasMore: boolean;
  skipped: ConversationSkippedView[];
  unavailableReason?: string;
};

/** 差分取得のために backend へ渡す値。session_id は初回だけ省略する。 */
export type ConversationRequest = {
  project: string;
  sessionId?: string;
  afterSeq: number;
  limit: number;
};

/**
 * 会話ページの表示状態。
 *
 * プロジェクト名と本文を同じ遷移で更新する。切替中や切替失敗時に、新しいプロジェクト名と
 * 旧プロジェクトの本文が同時に出ないよう、page の有無を遷移で決める。
 */
export type ConversationView = {
  project: string;
  page?: ConversationPage;
  loading: boolean;
  error?: string;
};

/** プロジェクト切替の開始。旧プロジェクトの本文を破棄し、先頭からの取得にする。 */
export function beginConversationSwitch(project: string): ConversationView {
  return { project, page: undefined, loading: true };
}

/** 取得成功。ヘッダのプロジェクトと本文を同じ応答で更新し、警告を解除する。 */
export function conversationLoaded(view: ConversationView, page: ConversationPage): ConversationView {
  return { project: view.project, page, loading: false };
}

/** 取得失敗。表示済みの本文は保ち、失敗を警告として残す。 */
export function conversationFailed(view: ConversationView, error: string): ConversationView {
  return { ...view, loading: false, error };
}

const toDate = (value: unknown): Date | undefined => {
  if (value === undefined || value === null) {
    return undefined;
  }
  const timestamp = Timestamp.fromJSON(value);
  return new Date(timestamp.seconds * 1_000 + Math.round(timestamp.nanos / 1_000_000));
};

const asNumber = (value: unknown): number => (typeof value === "number" ? value : Number(value ?? 0));

const toMessage = (raw: Record<string, unknown>): ConversationMessageView => ({
  seq: asNumber(raw.seq),
  occurredAt: toDate(raw.occurredAt),
  speaker: enumFromWire("ConversationSpeaker", raw.speaker),
  text: String(raw.text ?? ""),
});

const toSkipped = (raw: Record<string, unknown>): ConversationSkippedView => ({
  reason: String(raw.reason ?? ""),
  count: asNumber(raw.count),
});

// proto の JSON 表現（文字列の時刻を含む）を、画面が使う会話の型へ復元する。
export function restoreConversationResponse(payload: unknown): ConversationPage {
  const raw = (payload ?? {}) as Record<string, unknown>;
  const arrayOf = (value: unknown): Record<string, unknown>[] =>
    Array.isArray(value) ? value.map((item) => item as Record<string, unknown>) : [];
  return {
    project: String(raw.project ?? ""),
    sessionId: String(raw.sessionId ?? ""),
    provider: String(raw.provider ?? ""),
    messages: arrayOf(raw.messages).map(toMessage),
    nextAfterSeq: asNumber(raw.nextAfterSeq),
    hasMore: raw.hasMore === true,
    skipped: arrayOf(raw.skipped).map(toSkipped),
    unavailableReason: typeof raw.unavailableReason === "string" ? raw.unavailableReason : undefined,
  };
}

/** 発話者の数値を画面の表示名へ変換する。未知の値は「不明」にする。 */
export function conversationSpeakerLabel(speaker: number): string {
  switch (speaker) {
    case CONVERSATION_SPEAKER.USER:
      return "ユーザー";
    case CONVERSATION_SPEAKER.MANAGER:
      return "manager";
    case CONVERSATION_SPEAKER.SYSTEM:
      return "システム";
    default:
      return "不明";
  }
}

/** 会話を seq の昇順に整える。入力の配列は変更しない。 */
export function sortConversationMessages(messages: ConversationMessageView[]): ConversationMessageView[] {
  return [...messages].sort((left, right) => left.seq - right.seq);
}

const mergeSkipped = (
  base: ConversationSkippedView[],
  next: ConversationSkippedView[],
): ConversationSkippedView[] => {
  const counts = new Map<string, number>();
  for (const item of [...base, ...next]) {
    counts.set(item.reason, (counts.get(item.reason) ?? 0) + item.count);
  }
  return [...counts].map(([reason, count]) => ({ reason, count }));
};

/**
 * 既に表示している会話へ差分のページを足す。
 *
 * 同じ seq は重複させず、skipped は理由ごとに合算する。続きの位置と未読の有無は
 * 新しいページの値を使い、会話の識別（project・session_id・provider）は既存の値を保つ。
 */
export function mergeConversationPages(base: ConversationPage, next: ConversationPage): ConversationPage {
  const bySeq = new Map<number, ConversationMessageView>();
  for (const message of [...base.messages, ...next.messages]) {
    bySeq.set(message.seq, message);
  }
  return {
    project: base.project !== "" ? base.project : next.project,
    sessionId: base.sessionId !== "" ? base.sessionId : next.sessionId,
    provider: base.provider !== "" ? base.provider : next.provider,
    messages: sortConversationMessages([...bySeq.values()]),
    nextAfterSeq: next.nextAfterSeq,
    hasMore: next.hasMore,
    skipped: mergeSkipped(base.skipped, next.skipped),
    unavailableReason: next.unavailableReason,
  };
}

export type FetchConversationOptions = {
  sessionId?: string;
  afterSeq?: number;
  limit?: number;
};

export type FetchedConversation = {
  page: ConversationPage;
  sessionId: string;
  nextAfterSeq: number;
};

/**
 * has_more の間、next_after_seq を次の after_seq にして会話を取得する。
 *
 * 最初の応答の session_id を固定し、以降の要求で別のセッションが返っても混在させない。
 * カーソルが進まない応答では、同じページを取り続けないよう取得を止める。
 */
export async function fetchConversationPages(
  fetcher: (request: ConversationRequest) => Promise<ConversationPage>,
  project: string,
  options: FetchConversationOptions = {},
): Promise<FetchedConversation> {
  const limit = options.limit ?? 200;
  let afterSeq = options.afterSeq ?? 0;
  let sessionId = options.sessionId;

  let merged: ConversationPage | undefined;
  for (;;) {
    const page = await fetcher({ project, sessionId, afterSeq, limit });
    merged = merged === undefined ? page : mergeConversationPages(merged, page);
    if (sessionId === undefined) {
      sessionId = page.sessionId;
    }
    if (!page.hasMore || page.nextAfterSeq <= afterSeq) {
      break;
    }
    afterSeq = page.nextAfterSeq;
  }

  const page = merged as ConversationPage;
  return { page, sessionId: sessionId ?? page.sessionId, nextAfterSeq: page.nextAfterSeq };
}

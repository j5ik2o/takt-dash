import type { ActivityEvent } from "../proto/taktdash";
import { ACTIVITY_KIND } from "./proto-values";

// 通知の本文の改行の表れ方。元の文面では、改行が「\n」という文字の並び
// （エスケープされた形）で入っている場合と、実際の改行で入っている場合がある。
// どちらの形も改行として扱い、それ以外の文字列はそのまま残す。
const BODY_LINE_BREAK = /\\r\\n|\\n|\r\n|\r|\n/;

// 通知の本文を行に分ける。改行の生の「\n」と実際の改行のどちらも行の区切りになる。
export function activityBodyLines(body: string): string[] {
  return body.split(BODY_LINE_BREAK);
}

// 接続状態の変化を表す出来事。件数と期間を1行にまとめる対象です。
const isConnectionKind = (kind: number): boolean =>
  kind === ACTIVITY_KIND.CONNECTION_LOST || kind === ACTIVITY_KIND.CONNECTION_RESTORED;

export type ActivityGroup = {
  events: ActivityEvent[];
  count: number;
  newest: ActivityEvent;
  oldest: ActivityEvent;
};

// backend が返す最新順の配列を受け取り、隣接する同一サーバー・同一プロジェクト・同一種別の
// 接続イベントだけを1つのまとまりにします。切断と復帰は種別が違うためまとめません。
// 非接続の出来事は1件ずつのまとまりとして返します。
export function groupActivityEvents(events: readonly ActivityEvent[]): ActivityGroup[] {
  const groups: ActivityGroup[] = [];
  for (const event of events) {
    const current = groups.at(-1);
    const canMerge =
      current !== undefined &&
      current.events.length > 0 &&
      isConnectionKind(event.kind) &&
      event.kind === current.events[0]!.kind &&
      event.serverId === current.events[0]!.serverId &&
      event.project !== undefined &&
      event.project === current.events[0]!.project;
    if (canMerge) {
      current.events.push(event);
      current.count = current.events.length;
      current.oldest = event;
      continue;
    }
    groups.push({ events: [event], count: 1, newest: event, oldest: event });
  }
  return groups;
}

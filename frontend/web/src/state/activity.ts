import type { ActivityEvent } from "../proto/taktdash";
import { ACTIVITY_KIND } from "./proto-values";

// 接続状態の変化を表す出来事。件数と期間を1行にまとめる対象です。
const isConnectionKind = (kind: number): boolean =>
  kind === ACTIVITY_KIND.CONNECTION_LOST || kind === ACTIVITY_KIND.CONNECTION_RESTORED;

export type ActivityGroup = {
  events: ActivityEvent[];
  count: number;
  newest: ActivityEvent;
  oldest: ActivityEvent;
};

// backend が返す最新順の配列を受け取り、隣接する同一サーバー・同一プロジェクトの接続イベントだけを
// 1つのまとまりにします。非接続の出来事は1件ずつのまとまりとして返します。
export function groupActivityEvents(events: readonly ActivityEvent[]): ActivityGroup[] {
  const groups: ActivityGroup[] = [];
  for (const event of events) {
    const current = groups.at(-1);
    const canMerge =
      current !== undefined &&
      current.events.length > 0 &&
      isConnectionKind(event.kind) &&
      isConnectionKind(current.events[0]!.kind) &&
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

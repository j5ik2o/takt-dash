import type { ReactElement } from "react";
import type { ActivityEvent, ActivityKind, DashboardState, GetActivityResponse } from "../proto/taktdash";
import { ACTIVITY_KIND } from "../state/proto-values";
import { formatClock } from "../state/format";

type ActivityContentProps = {
  state: DashboardState;
  response: GetActivityResponse;
  now: Date;
};

const kindLabel = (kind: ActivityKind): string => {
  switch (kind) {
    case ACTIVITY_KIND.GOAL_CREATED:
      return "ゴール登録";
    case ACTIVITY_KIND.RUN_STARTED:
      return "run の開始";
    case ACTIVITY_KIND.TASK_COMPLETED:
      return "タスク完了";
    case ACTIVITY_KIND.INQUIRY:
      return "問い合わせ";
    case ACTIVITY_KIND.AWAITING_MERGE:
      return "人のマージ待ち";
    case ACTIVITY_KIND.PAUSED:
      return "一時停止";
    case ACTIVITY_KIND.BLOCKED:
      return "blocked 通知";
    case ACTIVITY_KIND.CONNECTION_LOST:
      return "接続の切断";
    default:
      return "その他";
  }
};

const isAttention = (kind: ActivityKind): boolean =>
  kind === ACTIVITY_KIND.INQUIRY ||
  kind === ACTIVITY_KIND.AWAITING_MERGE ||
  kind === ACTIVITY_KIND.BLOCKED ||
  kind === ACTIVITY_KIND.CONNECTION_LOST;

const eventTime = (event: ActivityEvent): number => (event.occurredAt ? event.occurredAt.getTime() : 0);

// 出来事は backend が新しい順で返す。ここでは同じ意味（新しい順）を保つだけで、順序を反転しない。
const toNewestFirst = (events: readonly ActivityEvent[]): ActivityEvent[] =>
  [...events].sort((a, b) => eventTime(b) - eventTime(a));

export function ActivityContent({ state, response, now }: ActivityContentProps): ReactElement {
  const serverNames = new Map(state.servers.map((server) => [server.serverId, server.name]));
  const events = toNewestFirst(response.events);

  return (
    <main className="page">
      <header className="page-head">
        <div>
          <h1>アクティビティ</h1>
          <p>全サーバーで起きた出来事を、新しいものから並べています。最終更新は {formatClock(now)} です。</p>
        </div>
      </header>

      <section aria-labelledby="h-daily">
        <h2 className="sec" id="h-daily">
          日ごとの出来事
        </h2>
        <ul className="daily">
          {response.dailySummaries.map((day) => (
            <li key={day.date} className={day.attention > 0 ? "warn" : ""}>
              <span className="day">{day.date}</span>
              <span className="total">{day.total}件</span>
              {day.attention > 0 ? <span className="attention">うち {day.attention}件</span> : null}
            </li>
          ))}
        </ul>
      </section>

      <section aria-labelledby="h-today">
        <h2 className="sec" id="h-today">
          今日の出来事（サーバー別）
        </h2>
        <ul className="counts">
          {response.todayCountsByServer.map((count) => (
            <li key={count.serverId}>
              <span>{serverNames.get(count.serverId) ?? count.serverId}</span>
              <b>{count.count}件</b>
            </li>
          ))}
        </ul>
      </section>

      <section aria-label="出来事の一覧">
        {events.length === 0 ? (
          <p className="note">出来事はありません。</p>
        ) : (
          <ul className="events">
            {events.map((event, index) => (
              <li className={`ev ${isAttention(event.kind) ? "warn" : ""}`} key={`${event.serverId}-${index}`}>
                <time>{event.occurredAt ? formatClock(event.occurredAt) : "—"}</time>
                <span className="kind">{kindLabel(event.kind)}</span>
                <span className="msg">
                  {event.project ? <b>{event.project}：</b> : null}
                  {event.body}
                </span>
                <span className="sv">{serverNames.get(event.serverId) ?? event.serverId}</span>
              </li>
            ))}
          </ul>
        )}
      </section>
    </main>
  );
}

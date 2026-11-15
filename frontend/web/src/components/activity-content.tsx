import type { ReactElement } from "react";
import type { ActivityEvent, ActivityKind, DailySummary, GetActivityResponse } from "../proto/taktdash";
import { groupActivityEvents } from "../state/activity";
import { ACTIVITY_KIND } from "../state/proto-values";
import { formatClock } from "../state/format";

type ActivityContentProps = {
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
    case ACTIVITY_KIND.CONNECTION_RESTORED:
      return "接続の復帰";
    default:
      return "その他";
  }
};

// 種類ごとの状態色。実行中・完了・復帰は live、人の判断や異常は warn、
// 登録・一時停止は idle にする。
const kindState = (kind: ActivityKind): "live" | "warn" | "idle" => {
  switch (kind) {
    case ACTIVITY_KIND.RUN_STARTED:
    case ACTIVITY_KIND.TASK_COMPLETED:
    case ACTIVITY_KIND.CONNECTION_RESTORED:
      return "live";
    case ACTIVITY_KIND.INQUIRY:
    case ACTIVITY_KIND.AWAITING_MERGE:
    case ACTIVITY_KIND.BLOCKED:
    case ACTIVITY_KIND.CONNECTION_LOST:
      return "warn";
    default:
      return "idle";
  }
};

const eventTime = (event: ActivityEvent): number => (event.occurredAt ? event.occurredAt.getTime() : 0);

// 出来事は backend が新しい順で返す。ここでは同じ意味（新しい順）を保つだけで、順序を反転しない。
const toNewestFirst = (events: readonly ActivityEvent[]): ActivityEvent[] =>
  [...events].sort((a, b) => eventTime(b) - eventTime(a));

// backend は日次集計を古い日から返すが、画面では新しい日を先頭に表示する。
const toNewestDayFirst = (days: readonly DailySummary[]): DailySummary[] =>
  [...days].sort((a, b) => b.date.localeCompare(a.date));

export function ActivityContent({ response, now }: ActivityContentProps): ReactElement {
  const state = response.state;
  const serverNames = new Map((state?.servers ?? []).map((server) => [server.serverId, server.name]));
  const events = toNewestFirst(response.events);
  const groups = groupActivityEvents(events);
  const dailySummaries = toNewestDayFirst(response.dailySummaries);

  return (
    <main className="page">
      <header className="page-head">
        <div>
          <h1>アクティビティ</h1>
          <p>全サーバーで起きた出来事を、新しいものから並べています。最終更新は {formatClock(now)} です。</p>
        </div>
      </header>

      <section aria-label="出来事の一覧">
        {groups.length === 0 ? (
          <p className="note">出来事はありません。</p>
        ) : (
          <ul className="events">
            {groups.map((group, index) => {
              const newest = group.newest;
              const single = group.count === 1;
              const rowState = kindState(newest.kind);
              const attention = rowState === "warn";
              const clockOf = (event: ActivityEvent): string =>
                event.occurredAt ? formatClock(event.occurredAt) : "—";
              const timeText = single ? clockOf(newest) : `${clockOf(group.oldest)} 〜 ${clockOf(newest)}`;
              return (
                <li className={`ev ${attention ? "warn" : ""}`} key={`${newest.serverId}-${index}`}>
                  <time>{timeText}</time>
                  <span className={attention ? "kl warn" : "kl"}>
                    <span className={`dot ${rowState}`} aria-hidden="true" />
                    {single ? kindLabel(newest.kind) : `${kindLabel(newest.kind)}（${group.count}件）`}
                  </span>
                  <span className="msg">
                    {newest.project ? <b>{newest.project}：</b> : null}
                    {single ? newest.body : "接続の状態が変わりました。"}
                  </span>
                  <span className="sv">{serverNames.get(newest.serverId) ?? newest.serverId}</span>
                </li>
              );
            })}
          </ul>
        )}
      </section>

      <section aria-labelledby="h-daily">
        <h2 className="sec" id="h-daily">
          日ごとの出来事
        </h2>
        <ul className="daily">
          {dailySummaries.map((day) => (
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
    </main>
  );
}

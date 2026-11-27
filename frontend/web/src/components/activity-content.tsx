import { Fragment, type ReactElement } from "react";
import type { ActivityEvent, ActivityKind, DailySummary, DashboardState, GetActivityResponse } from "../proto/taktdash";
import { activityBodyLines, groupActivityEvents } from "../state/activity";
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
    case ACTIVITY_KIND.CONNECTION_RESTORED:
      return "接続の復帰";
    case ACTIVITY_KIND.TASK_FAILED:
      return "タスクの失敗";
    case ACTIVITY_KIND.TASK_EXCEEDED:
      return "タスクの上限超過";
    case ACTIVITY_KIND.TASK_PR_FAILED:
      return "PR 失敗";
    case ACTIVITY_KIND.RUN_FAILED:
      return "run の失敗";
    case ACTIVITY_KIND.RUN_ABORTED:
      return "run の中断";
    case ACTIVITY_KIND.BACKEND_STARTED:
      return "backend の起動";
    default:
      return "その他";
  }
};

// 種類ごとの状態色。実行中・完了・復帰は live、人の判断や異常（失敗と中断を含む）は warn、
// 登録・一時停止・backend の起動は idle にする。
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
    case ACTIVITY_KIND.TASK_FAILED:
    case ACTIVITY_KIND.TASK_EXCEEDED:
    case ACTIVITY_KIND.TASK_PR_FAILED:
    case ACTIVITY_KIND.RUN_FAILED:
    case ACTIVITY_KIND.RUN_ABORTED:
      return "warn";
    default:
      return "idle";
  }
};

// backend の起動は、どのサーバーにも属さないためサーバーが空で届く。空欄にせず taktDash と出す。
const serverLabel = (serverNames: ReadonlyMap<string, string>, serverId: string): string =>
  serverId === "" ? "taktDash" : (serverNames.get(serverId) ?? serverId);

const eventTime = (event: ActivityEvent): number => (event.occurredAt ? event.occurredAt.getTime() : 0);

// 出来事は backend が新しい順で返す。ここでは同じ意味（新しい順）を保つだけで、順序を反転しない。
const toNewestFirst = (events: readonly ActivityEvent[]): ActivityEvent[] =>
  [...events].sort((a, b) => eventTime(b) - eventTime(a));

// backend は日次集計を古い日から返すが、画面では新しい日を先頭に表示する。
const toNewestDayFirst = (days: readonly DailySummary[]): DailySummary[] =>
  [...days].sort((a, b) => b.date.localeCompare(a.date));

const JST_MS = 9 * 3600_000;
const jstParts = (date: Date) => {
  const jst = new Date(date.getTime() + JST_MS);
  return { year: jst.getUTCFullYear(), month: jst.getUTCMonth() + 1, day: jst.getUTCDate() };
};
const sameJstDay = (a: Date, b: Date): boolean => {
  const x = jstParts(a); const y = jstParts(b);
  return x.year === y.year && x.month === y.month && x.day === y.day;
};
const pad2 = (v: number): string => String(v).padStart(2, "0");
const dayHeading = (date: Date | undefined, now: Date): string => {
  if (date === undefined) return "日付不明";
  const d = jstParts(date); const n = jstParts(now);
  const year = d.year === n.year ? "" : `${d.year}年`;
  const today = sameJstDay(date, now) ? "（今日）" : "";
  return `${year}${d.month}月${d.day}日${today}`;
};
const dayKey = (date: Date | undefined): string => {
  if (date === undefined) return "unknown";
  const d = jstParts(date);
  return `${d.year}-${d.month}-${d.day}`;
};

export function ActivityContent({ state, response, now }: ActivityContentProps): ReactElement {
  const serverNames = new Map(state.servers.map((server) => [server.serverId, server.name]));
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
          (() => {
            type G = (typeof groups)[number];
            const dayGroups: Array<{ key: string; heading: string; items: G[] }> = [];
            for (const group of groups) {
              const key = dayKey(group.newest.occurredAt);
              const last = dayGroups.at(-1);
              if (last !== undefined && last.key === key) {
                last.items.push(group);
              } else {
                dayGroups.push({ key, heading: dayHeading(group.newest.occurredAt, now), items: [group] });
              }
            }
            return dayGroups.map((dayGroup) => (
              <div className="events-day" key={dayGroup.key}>
                <h2 className="day-head">{dayGroup.heading}</h2>
                <ul className="events">
                  {dayGroup.items.map((group, index) => {
                    const newest = group.newest;
                    const single = group.count === 1;
                    const rowState = kindState(newest.kind);
                    const attention = rowState === "warn";
                    const clockOf = (event: ActivityEvent): string =>
                      event.occurredAt ? formatClock(event.occurredAt) : "—";
                    let timeText: string;
                    if (single) {
                      timeText = clockOf(newest);
                    } else if (newest.occurredAt === undefined) {
                      timeText = "— 〜 —";
                    } else if (group.oldest.occurredAt === undefined) {
                      timeText = `— 〜 ${clockOf(newest)}`;
                    } else if (sameJstDay(group.oldest.occurredAt, newest.occurredAt)) {
                      timeText = `${clockOf(group.oldest)} 〜 ${clockOf(newest)}`;
                    } else {
                      const from = jstParts(group.oldest.occurredAt);
                      const to = jstParts(newest.occurredAt);
                      const datePart =
                        from.year === to.year
                          ? `${pad2(from.month)}-${pad2(from.day)}`
                          : `${from.year}-${pad2(from.month)}-${pad2(from.day)}`;
                      timeText = `${datePart} ${clockOf(group.oldest)} 〜 ${clockOf(newest)}`;
                    }
                    return (
                      <li className={`ev ${attention ? "warn" : ""}`} key={`${newest.serverId}-${index}`}>
                        <time dateTime={newest.occurredAt ? newest.occurredAt.toISOString() : undefined}>{timeText}</time>
                        <span className={attention ? "kl warn" : "kl"}>
                          <span className={`dot ${rowState}`} aria-hidden="true" />
                          {single ? kindLabel(newest.kind) : `${kindLabel(newest.kind)}（${group.count}件）`}
                        </span>
                        <span className="msg">
                          {newest.project ? <b>{newest.project}：</b> : null}
                          {activityBodyLines(newest.body).map((line, lineIndex) => (
                            <Fragment key={lineIndex}>
                              {lineIndex > 0 ? <br /> : null}
                              {line}
                            </Fragment>
                          ))}
                        </span>
                        <span className="sv">{serverLabel(serverNames, newest.serverId)}</span>
                      </li>
                    );
                  })}
                </ul>
              </div>
            ));
          })()
        )}
      </section>

      <section aria-labelledby="h-daily">
        <h2 className="sec" id="h-daily">
          日ごとの出来事
        </h2>
        {dailySummaries.length === 0 || dailySummaries.every((day) => day.total === 0) ? (
          <p className="note">この 7 日間の出来事はまだありません。</p>
        ) : (
          <ul className="daily">
            {dailySummaries.map((day) => (
              <li key={day.date} className={day.attention > 0 ? "warn" : ""}>
                <span className="day">{day.date}</span>
                <span className="total">{day.total}件</span>
                {day.attention > 0 ? <span className="attention">うち要対応 {day.attention}件</span> : null}
              </li>
            ))}
          </ul>
        )}
      </section>

      <section aria-labelledby="h-today">
        <h2 className="sec" id="h-today">
          今日の出来事（サーバー別）
        </h2>
        {response.todayCountsByServer.length === 0 ? (
          <p className="note">今日の出来事はまだありません。</p>
        ) : (
          <ul className="counts">
            {response.todayCountsByServer.map((count) => (
              <li key={count.serverId}>
                <span>{serverNames.get(count.serverId) ?? count.serverId}</span>
                <b>{count.count}件</b>
              </li>
            ))}
          </ul>
        )}
      </section>
    </main>
  );
}

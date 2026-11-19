import type { ReactElement } from "react";
import type { DashboardGoal as Goal, DashboardState } from "../state/restore-state";
import {
  isHumanMergeWaiting,
  mergeTaskFailureNote,
  neverReadProjects,
  neverReadServers,
  taskStatusLabel,
} from "../state/dashboard";
import { goalStateSentence } from "../state/goal-badge";
import { goalDisplayName, goalListHeading } from "../state/goal-summary";
import { TASK_STATUS } from "../state/proto-values";
import type { BackendUnavailableSituation } from "../grpc/client";
import { BackendUnavailableNotice } from "./backend-unavailable-notice";

type GoalsContentProps = {
  state: DashboardState;
  project?: string;
  disconnected: boolean;
  /** 切断時の通知の文面を決める状況。ライブ表示だけが渡す。 */
  unavailableSituation?: BackendUnavailableSituation;
};

// 件数は 0 件の項目を出さない（issue #148）。失敗・上限超過・PR 失敗は要対応として
// 目立たせる対象なので attention を付け、呼び出し側が warn の見た目に変換する。
const countEntries = (goal: Goal): Array<{ label: string; count: number; attention: boolean }> => {
  const counts = goal.taskCounts;
  return [
    { label: taskStatusLabel(TASK_STATUS.PENDING), count: counts?.pending ?? 0, attention: false },
    { label: taskStatusLabel(TASK_STATUS.RUNNING), count: counts?.running ?? 0, attention: false },
    { label: taskStatusLabel(TASK_STATUS.COMPLETED), count: counts?.completed ?? 0, attention: false },
    { label: taskStatusLabel(TASK_STATUS.FAILED), count: counts?.failed ?? 0, attention: true },
    { label: taskStatusLabel(TASK_STATUS.EXCEEDED), count: counts?.exceeded ?? 0, attention: true },
    { label: taskStatusLabel(TASK_STATUS.PR_FAILED), count: counts?.prFailed ?? 0, attention: true },
  ].filter((entry) => entry.count > 0);
};

// ゴール一覧は全サーバー・全プロジェクトのゴールを既定で表示し、?project= があるときだけ
// そのプロジェクトに絞り込む。マージ待ちの判定と仕上げタスクの失敗の表示は dashboard の
// 関数を使う。切断時は概要と判断待ちと同じく、内容の先頭に backend 不通の通知を
// 1回だけ出す（issue #252）。
export function GoalsContent({ state, project, disconnected, unavailableSituation }: GoalsContentProps): ReactElement {
  const goals = project === undefined ? state.goals : state.goals.filter((goal) => goal.project === project);
  // 未読の判定は、絞り込みがあるときはそのプロジェクトと所属サーバーだけを対象にする。
  // まだ読めていない対象があると、その配下にゴールがあっても一覧へ出てこない。
  // 0件でも「ゴールはありません」とは断定できないため、未読を注記する（issue #259）。
  const scopedProjects =
    project === undefined ? state.projects : state.projects.filter((item) => item.name === project);
  const scopedServerIds = new Set(scopedProjects.map((item) => item.serverId));
  const hasUnread =
    neverReadProjects(scopedProjects).length > 0 ||
    neverReadServers(
      state.servers.filter((server) => project === undefined || scopedServerIds.has(server.serverId)),
    ).length > 0;
  return (
    <main className="page">
      {disconnected ? <BackendUnavailableNotice situation={unavailableSituation} /> : null}
      <header className="page-head">
        <div>
          <h1>ゴール</h1>
          <p>
            {project === undefined
              ? `全サーバー・全プロジェクトのゴールを${goals.length}件表示しています。`
              : `プロジェクト「${project}」のゴールを${goals.length}件表示しています。`}
          </p>
          {hasUnread ? (
            <p className="stale-note">
              まだ読めていないサーバー・プロジェクトがあります。表示している件数には、まだ読めていない分は含まれていません。
            </p>
          ) : null}
        </div>
      </header>
      {goals.length === 0 ? (
        hasUnread ? (
          <p className="note">サーバーからまだ読めていないため、ゴールがあるかどうかは分かりません。</p>
        ) : (
          <p className="note">ゴールはありません。</p>
        )
      ) : (
        <ul className="cards">
          {goals.map((goal) => {
            const counts = countEntries(goal);
            const heading = goalListHeading(goal.name);
            const fullName = goalDisplayName(goal.name);
            return (
              <li className="card" key={goal.goalId}>
                <div className="card-head goal-card-head">
                  <h3>
                    <a href={`/goals/${encodeURIComponent(goal.goalId)}`}>{heading}</a>
                  </h3>
                  <span className="about">{goal.project}</span>
                </div>
                {fullName !== heading ? (
                  <details className="goal-full">
                    <summary>ゴールの目的の全文を表示します。</summary>
                    <p className="goal-text">{fullName}</p>
                  </details>
                ) : null}
                <p>{goalStateSentence(goal)}</p>
                {isHumanMergeWaiting(goal) ? <p>マージの承認待ちです。</p> : null}
                {mergeTaskFailureNote(goal) !== undefined ? <p>{mergeTaskFailureNote(goal)}</p> : null}
                {counts.length > 0 ? (
                  <>
                    <p className="k">タスクの件数</p>
                    <div className="kv task-counts">
                      {counts.map((entry) => (
                        <span key={entry.label} className={entry.attention ? "warn" : undefined}>
                          {entry.label} {entry.count}件
                        </span>
                      ))}
                    </div>
                  </>
                ) : null}
                {goal.pullRequest !== undefined ? (
                  <p>
                    プルリクエスト{" "}
                    <a href={goal.pullRequest.url}>#{goal.pullRequest.number}</a>
                  </p>
                ) : null}
              </li>
            );
          })}
        </ul>
      )}
    </main>
  );
}

import type { ReactElement } from "react";
import type { DashboardGoal as Goal, DashboardState } from "../state/restore-state";
import { isHumanMergeWaiting, mergeTaskFailureNote, taskStatusLabel } from "../state/dashboard";
import { goalStateSentence } from "../state/goal-badge";
import { goalDisplayName } from "../state/goal-summary";
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

const countEntries = (goal: Goal): Array<{ label: string; count: number }> => {
  const counts = goal.taskCounts;
  return [
    { label: taskStatusLabel(TASK_STATUS.PENDING), count: counts?.pending ?? 0 },
    { label: taskStatusLabel(TASK_STATUS.RUNNING), count: counts?.running ?? 0 },
    { label: taskStatusLabel(TASK_STATUS.COMPLETED), count: counts?.completed ?? 0 },
    { label: taskStatusLabel(TASK_STATUS.FAILED), count: counts?.failed ?? 0 },
    { label: taskStatusLabel(TASK_STATUS.EXCEEDED), count: counts?.exceeded ?? 0 },
    { label: taskStatusLabel(TASK_STATUS.PR_FAILED), count: counts?.prFailed ?? 0 },
  ];
};

// ゴール一覧は全サーバー・全プロジェクトのゴールを既定で表示し、?project= があるときだけ
// そのプロジェクトに絞り込む。マージ待ちの判定と仕上げタスクの失敗の表示は dashboard の
// 関数を使う。切断時は概要と判断待ちと同じく、内容の先頭に backend 不通の通知を
// 1回だけ出す（issue #252）。
export function GoalsContent({ state, project, disconnected, unavailableSituation }: GoalsContentProps): ReactElement {
  const goals = project === undefined ? state.goals : state.goals.filter((goal) => goal.project === project);
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
        </div>
      </header>
      {goals.length === 0 ? (
        <p className="note">ゴールはありません。</p>
      ) : (
        <ul className="cards">
          {goals.map((goal) => (
            <li className="card" key={goal.goalId}>
              <div className="card-head">
                <h3>
                  <a href={`/goals/${encodeURIComponent(goal.goalId)}`}>{goalDisplayName(goal.name)}</a>
                </h3>
                <span className="about">{goal.project}</span>
              </div>
              <p>{goalStateSentence(goal)}</p>
              <p>{isHumanMergeWaiting(goal) ? "マージの承認待ちです。" : "マージの承認待ちではありません。"}</p>
              {mergeTaskFailureNote(goal) !== undefined ? <p>{mergeTaskFailureNote(goal)}</p> : null}
              <p className="k">タスクの件数</p>
              <div className="kv task-counts">
                {countEntries(goal).map((entry) => (
                  <span key={entry.label}>
                    {entry.label} {entry.count}件
                  </span>
                ))}
              </div>
              {goal.pullRequest === undefined ? (
                <p>プルリクエストはありません。</p>
              ) : (
                <p>
                  プルリクエスト{" "}
                  <a href={goal.pullRequest.url}>#{goal.pullRequest.number}</a>
                </p>
              )}
            </li>
          ))}
        </ul>
      )}
    </main>
  );
}

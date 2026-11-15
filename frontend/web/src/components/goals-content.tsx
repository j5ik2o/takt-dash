import type { ReactElement } from "react";
import type { DashboardState, Goal } from "../proto/taktdash";
import { executionStatusLabel, goalStatusLabel, isHumanMergeWaiting, taskStatusLabel } from "../state/dashboard";
import { TASK_STATUS } from "../state/proto-values";

type GoalsContentProps = {
  state: DashboardState;
  project?: string;
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
// そのプロジェクトに絞り込む。マージ待ちの判定は backend と同じ規則を使う。
export function GoalsContent({ state, project }: GoalsContentProps): ReactElement {
  const goals = project === undefined ? state.goals : state.goals.filter((goal) => goal.project === project);
  return (
    <main className="page">
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
                  <a href={`/goals/${encodeURIComponent(goal.goalId)}`}>{goal.name}</a>
                </h3>
                <span className="about">{goal.project}</span>
              </div>
              <p>
                ゴールは{goalStatusLabel(goal.status)}で、実行状態は{executionStatusLabel(goal.executionStatus)}です。
              </p>
              <p>{isHumanMergeWaiting(goal) ? "マージの承認待ちです。" : "マージの承認待ちではありません。"}</p>
              <p className="k">タスクの件数</p>
              <div className="kv">
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

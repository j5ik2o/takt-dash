import type { ReactElement } from "react";
import type { GetProjectsResponse, Goal } from "../proto/taktdash";
import { projectAnchorId } from "../state/anchors";
import { goalBadge } from "../state/goal-badge";
import { goalHeading, goalTaskProgress } from "../state/goal-summary";
import { projectKey, resolveThresholds, sortPendingDecisions, toRunView } from "../state/dashboard";

type ProjectsContentProps = {
  response: GetProjectsResponse;
  now: Date;
};

// 既定では進行中のゴールだけを出し、完了と中止は件数付きの折りたたみへ回す。
const isDoneGoal = (goal: Goal): boolean => {
  const kind = goalBadge(goal).kind;
  return kind === "completed" || kind === "aborted";
};

function GoalRow({ goal }: { goal: Goal }): ReactElement {
  const badge = goalBadge(goal);
  const progress = goalTaskProgress(goal);
  const pullRequest = goal.pullRequest;
  return (
    <li>
      <h4 className="goal-head">{goalHeading(goal.name)}</h4>
      <span className={`goal-badge ${badge.tone}`}>{badge.label}</span>
      {badge.kind === "paused" && goal.pausedReason !== undefined ? (
        <p className="goal-why">一時停止の理由: {goal.pausedReason}</p>
      ) : null}
      <details className="goal-full">
        <summary>このゴールの目的の全文を表示します。</summary>
        <p className="goal-text">{goal.name}</p>
      </details>
      <p className="goal-progress">タスクは完了 {progress.completed} / 全体 {progress.total} です。</p>
      {progress.failed > 0 ? (
        <p className="goal-failed warn">失敗したタスクが {progress.failed}件あります。</p>
      ) : null}
      {pullRequest !== undefined && pullRequest.url !== "" ? (
        <p className="goal-pr">
          <a href={pullRequest.url}>プルリクエスト #{pullRequest.number} を開く</a>
        </p>
      ) : null}
    </li>
  );
}

export function ProjectsContent({ response, now }: ProjectsContentProps): ReactElement {
  const state = response.state;
  if (state === undefined) {
    return <main className="page"><p className="note">プロジェクトの状態を取得できませんでした。</p></main>;
  }
  const serverById = new Map(state.servers.map((server) => [server.serverId, server]));
  const projectByKey = new Map(
    state.projects.map((project) => [projectKey(project.serverId, project.name), project]),
  );
  const thresholds = resolveThresholds(state.thresholds);
  const pending = sortPendingDecisions(state.pendingDecisions, now, {
    staleThresholdMinutes: thresholds.pendingDecisionStaleMinutes,
  });

  return (
    <main className="page">
      <header className="page-head">
        <div>
          <h1>プロジェクト</h1>
          <p>{state.projects.length}件のプロジェクトを集約しています。</p>
        </div>
      </header>
      <div className="cards">
        {state.projects.map((project) => {
          const goals = state.goals.filter((goal) => goal.project === project.name);
          const activeGoals = goals.filter((goal) => !isDoneGoal(goal));
          const doneGoals = goals.filter(isDoneGoal);
          const runs = state.runs.filter((run) => run.project === project.name);
          const projectPending = pending.filter((decision) => decision.project === project.name);
          return (
            <article className="card" id={projectAnchorId(project.name)} key={project.name}>
              <div className="card-head">
                <h3>{project.name}</h3>
                <span className="about">{project.about}</span>
              </div>
              <p className="k">判断待ち {projectPending.length}件</p>
              <div>
                <p className="k">走っている run（{runs.length}件）</p>
                <ul className="mini-runs">
                  {runs.map((run) => {
                    const view = toRunView(run, serverById, thresholds, now, undefined, projectByKey);
                    return (
                      <li key={run.runSlug}>
                        <span className="tn">{run.taskName}</span>
                        <span>{view.label}</span>
                        <a className="view mono" href={`/runs/${encodeURIComponent(run.runSlug)}`}>
                          {run.runSlug}
                        </a>
                      </li>
                    );
                  })}
                  {runs.length === 0 ? <li className="empty">走っている run はありません。</li> : null}
                </ul>
              </div>
              <div>
                <p className="k">進行中のゴール（{activeGoals.length}件）</p>
                <ul className="goals">
                  {activeGoals.map((goal) => (
                    <GoalRow goal={goal} key={goal.goalId} />
                  ))}
                  {activeGoals.length === 0 ? (
                    <li className="empty">進行中のゴールはありません。</li>
                  ) : null}
                </ul>
              </div>
              {doneGoals.length > 0 ? (
                <details className="goal-done">
                  <summary>完了したゴール {doneGoals.length}件を表示します。</summary>
                  <ul className="goals">
                    {doneGoals.map((goal) => (
                      <GoalRow goal={goal} key={goal.goalId} />
                    ))}
                  </ul>
                </details>
              ) : null}
            </article>
          );
        })}
      </div>
    </main>
  );
}

import type { ReactElement } from "react";
import type { DashboardState } from "../proto/taktdash";
import { executionStatusLabel, goalStatusLabel, resolveThresholds, sortPendingDecisions, toRunView } from "../state/dashboard";

type ProjectsContentProps = {
  state: DashboardState;
  now: Date;
};

export function ProjectsContent({ state, now }: ProjectsContentProps): ReactElement {
  const serverById = new Map(state.servers.map((server) => [server.serverId, server]));
  const projectById = new Map(state.projects.map((project) => [project.name, project]));
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
          const runs = state.runs.filter((run) => run.project === project.name);
          const projectPending = pending.filter((decision) => decision.project === project.name);
          return (
            <article className="card" key={project.name}>
              <div className="card-head">
                <h3>{project.name}</h3>
                <span className="about">{project.about}</span>
              </div>
              <p className="k">判断待ち {projectPending.length}件</p>
              <div>
                <p className="k">ゴール</p>
                <ul className="goals">
                  {goals.map((goal) => (
                    <li key={goal.goalId}>
                      <p className="gn">
                        <a href={`/goals?project=${encodeURIComponent(project.name)}`}>{goal.name}</a>
                      </p>
                      <p className="gs">
                        ゴールは{goalStatusLabel(goal.status)}で、実行状態は{executionStatusLabel(goal.executionStatus)}です。
                      </p>
                    </li>
                  ))}
                  {goals.length === 0 ? <li className="empty">ゴールはありません。</li> : null}
                </ul>
              </div>
              <div>
                <p className="k">走っている run（{runs.length}件）</p>
                <ul className="mini-runs">
                  {runs.map((run) => {
                    const view = toRunView(run, serverById, projectById, now);
                    return (
                      <li key={run.runSlug}>
                        <span className="tn">{run.taskName}</span>
                        <span>{view.label}</span>
                        <a className="view" href={`/runs/${encodeURIComponent(run.runSlug)}`}>
                          {run.runSlug}
                        </a>
                      </li>
                    );
                  })}
                  {runs.length === 0 ? <li className="empty">走っている run はありません。</li> : null}
                </ul>
              </div>
            </article>
          );
        })}
      </div>
    </main>
  );
}

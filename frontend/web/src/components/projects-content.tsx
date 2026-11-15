import type { ReactElement } from "react";
import type { ExecutionStatus, GetProjectsResponse } from "../proto/taktdash";
import { projectAnchorId } from "../state/anchors";
import { EXECUTION_STATUS } from "../state/proto-values";
import { executionStatusLabel, goalStatusLabel, projectKey, resolveThresholds, sortPendingDecisions, toRunView } from "../state/dashboard";

type ProjectsContentProps = {
  response: GetProjectsResponse;
  now: Date;
};

const executionStateClass = (status: ExecutionStatus): "live" | "idle" | "warn" => {
  switch (status) {
    case EXECUTION_STATUS.ACTIVE:
      return "live";
    case EXECUTION_STATUS.ABORTED:
      return "warn";
    default:
      return "idle";
  }
};

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
                <p className="k">ゴール</p>
                <ul className="goals">
                  {goals.map((goal) => (
                    <li key={goal.goalId}>
                      <p className="gn">{goal.name}</p>
                      <p className="gs">
                        ゴールは{goalStatusLabel(goal.status)}で、実行状態は
                        <span className={executionStateClass(goal.executionStatus)}>
                          {executionStatusLabel(goal.executionStatus)}
                        </span>
                        です。
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
            </article>
          );
        })}
      </div>
    </main>
  );
}

import type { ReactElement } from "react";
import type { GetProjectsResponse, Goal } from "../proto/taktdash";
import { projectAnchorId } from "../state/anchors";
import { goalBadge, goalSection, type GoalSection } from "../state/goal-badge";
import { goalDisplayName, goalListHeading, goalTaskProgress } from "../state/goal-summary";
import { FAILED_TASK_KINDS_LABEL, projectKey, resolveThresholds, sortPendingDecisions, toRunView } from "../state/dashboard";

type ProjectsContentProps = {
  response: GetProjectsResponse;
  now: Date;
};

// 既定では進行中のゴールだけを出し、未着手・完了・中止は区画ごとに件数付きの
// 折りたたみへ回す。区画分けは goal-badge の対応表（goalSection）に従い、タスクが
// 登録されていないゴールを進行中へ入れず（issue #242）、中止したゴールを完了へ
// 含めない（issue #243）。
const goalsInSection = (goals: Goal[], section: GoalSection): Goal[] =>
  goals.filter((goal) => goalSection(goal) === section);

function GoalRow({ goal }: { goal: Goal }): ReactElement {
  const badge = goalBadge(goal);
  const progress = goalTaskProgress(goal);
  const pullRequest = goal.pullRequest;
  return (
    <li>
      <h4 className="goal-head">{goalListHeading(goal.name)}</h4>
      <span className={`goal-badge ${badge.tone}`}>{badge.label}</span>
      {badge.kind === "paused" && goal.pausedReason !== undefined ? (
        <p className="goal-why">一時停止の理由: {goal.pausedReason}</p>
      ) : null}
      <details className="goal-full">
        <summary>このゴールの目的の全文を表示します。</summary>
        <p className="goal-text">{goalDisplayName(goal.name)}</p>
      </details>
      {progress.total === 0 ? (
        <p className="goal-progress">タスクはまだ登録されていません。</p>
      ) : (
        <p className="goal-progress">タスクは完了 {progress.completed} / 全体 {progress.total} です。</p>
      )}
      {progress.failed > 0 ? (
        <p className="goal-failed warn">{FAILED_TASK_KINDS_LABEL}のタスクが {progress.failed}件あります。</p>
      ) : null}
      {pullRequest !== undefined && pullRequest.url !== "" ? (
        <p className="goal-pr">
          <a href={pullRequest.url}>プルリクエスト #{pullRequest.number} を開く</a>
        </p>
      ) : null}
    </li>
  );
}

// 区画ごとの折りたたみ。スタイルは globals.css の .goal-done（折りたたんだゴールの
// 区画）へそろえ、区画を区別するクラスを併記する。
function GoalFold({
  className,
  title,
  goals,
}: {
  className: string;
  title: string;
  goals: Goal[];
}): ReactElement | null {
  if (goals.length === 0) {
    return null;
  }
  return (
    <details className={className}>
      <summary>
        {title} {goals.length}件を表示します。
      </summary>
      <ul className="goals">
        {goals.map((goal) => (
          <GoalRow goal={goal} key={goal.goalId} />
        ))}
      </ul>
    </details>
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
          const activeGoals = goalsInSection(goals, "inProgress");
          const completedGoals = goalsInSection(goals, "completed");
          const abortedGoals = goalsInSection(goals, "aborted");
          const untouchedGoals = goalsInSection(goals, "untouched");
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
              <GoalFold className="goal-done" title="完了したゴール" goals={completedGoals} />
              <GoalFold className="goal-done goal-aborted" title="中止したゴール" goals={abortedGoals} />
              <GoalFold className="goal-done goal-untouched" title="未着手のゴール" goals={untouchedGoals} />
            </article>
          );
        })}
      </div>
    </main>
  );
}

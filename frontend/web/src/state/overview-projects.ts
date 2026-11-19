import type { Project } from "../proto/taktdash";
import { toRunCount, toRunLimit, type RunCount, type RunLimit } from "./domain-primitives";
import { RUN_STATUS } from "./proto-values";
import { projectKey, type RunView } from "./run-view";

// 概要のプロジェクト行。プロジェクト自身の同時実行上限と、そのプロジェクトで
// 実行中の run の件数を対比できる形にする。
export type OverviewProjectView = Omit<Project, "concurrency"> & {
  concurrency: RunLimit;
  runningRuns: RunCount;
};

export function summarizeOverviewProjects(projects: readonly Project[], runs: readonly RunView[]) {
  const registeredRunningRuns = runs.filter(
    ({ server, run }) => server !== undefined && run.status === RUN_STATUS.RUNNING,
  );
  return {
    runningRuns: registeredRunningRuns.length,
    projects: projects.map((project): OverviewProjectView => ({
      ...project,
      concurrency: toRunLimit(project.concurrency),
      runningRuns: toRunCount(
        registeredRunningRuns.filter(
          ({ run }) => projectKey(run.serverId, run.project) === projectKey(project.serverId, project.name),
        ).length,
      ),
    })),
    runLimit: toRunLimit(projects.reduce((total, project) => total + project.concurrency, 0)),
    completedTasks24h: projects.reduce((total, project) => total + project.completedTasks24h, 0),
  };
}

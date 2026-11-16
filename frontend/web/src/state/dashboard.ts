import type {
  AnomalyThresholds,
  DashboardState,
  Goal,
  PendingDecision,
  Project,
  Run,
  Server,
} from "../proto/taktdash";
import { formatClock } from "./format";
import {
  CONNECTION_STATE,
  EXECUTION_STATUS,
  GOAL_STATUS,
  PENDING_DECISION_TYPE,
  RUN_STATUS,
  TASK_STATUS,
} from "./proto-values";

// 判断待ちの滞留とログの無音の既定値。要件定義 2.7 節の値で、backend が proto の
// AnomalyThresholds を返さないときだけ使う。値を決める場所はここ1か所に閉じる。
export const DEFAULT_ANOMALY_THRESHOLDS: AnomalyThresholds = {
  pendingDecisionStaleMinutes: 240,
  runLogSilentMinutes: 10,
};

// proto の閾値があればそれを使い、無い応答だけ既定値へ解決する。全画面がこの関数を
// 通すことで、既定値の食い違いを防ぐ。
export function resolveThresholds(thresholds: AnomalyThresholds | undefined): AnomalyThresholds {
  return thresholds ?? DEFAULT_ANOMALY_THRESHOLDS;
}

export type PendingDecisionView = PendingDecision & {
  waitMinutes: number;
  stale: boolean;
};

type RunView = {
  run: Run;
  server: Server | undefined;
  connected: boolean;
  state: "live" | "warn" | "delayed" | "idle";
  label: string;
  note: string;
  goalName?: string;
  currentStepName?: string;
  stepNumber?: number;
  totalSteps?: number;
  iteration?: number;
  model?: string;
  elapsedSeconds?: number;
};

type PendingCounts = {
  total: number;
  inquiries: number;
  awaitingMerge: number;
  goalRegistration: number;
};

type OverviewServerView = {
  server: Server;
  state: "live" | "idle" | "down";
  stateLabel: string;
  runs: RunView[];
  runLimit: number;
  runningRuns: number;
  inProgressGoals: number;
  pendingCount: number;
  waitingTasks: number;
};

export type OverviewView = {
  readAt: Date;
  servers: OverviewServerView[];
  projects: Project[];
  runs: RunView[];
  thresholds: AnomalyThresholds;
  runningRuns: number;
  runLimit: number;
  completedTasks24h: number;
  failedTaskCount: number;
  abortedGoalCount: number;
  silentRunCount: number;
  unresponsiveServers: Server[];
  unresponsiveProjects: Project[];
  partialReadAt?: Date;
  hasAnomalies: boolean;
  pendingCounts: PendingCounts;
  pendingDecisions: PendingDecisionView[];
};

export type SortOptions = {
  staleThresholdMinutes?: number;
};

const minutesSince = (from: Date, now: Date): number => Math.max(0, (now.getTime() - from.getTime()) / 60_000);

const staleThreshold = (options: SortOptions): number =>
  options.staleThresholdMinutes ?? DEFAULT_ANOMALY_THRESHOLDS.pendingDecisionStaleMinutes;
export function sortPendingDecisions(
  decisions: readonly PendingDecision[],
  now: Date,
  options: SortOptions = {},
): PendingDecisionView[] {
  const threshold = staleThreshold(options);
  return decisions
    .map((decision) => {
      const waitMinutes = decision.createdAt ? minutesSince(decision.createdAt, now) : 0;
      return { ...decision, waitMinutes, stale: waitMinutes >= threshold };
    })
    .sort((a, b) => b.waitMinutes - a.waitMinutes);
}

export function summarizePendingDecisions(decisions: readonly PendingDecisionView[]): PendingCounts {
  const inquiries = decisions.filter(
    (decision) => decision.type === PENDING_DECISION_TYPE.INQUIRY,
  ).length;
  const awaitingMerge = decisions.filter(
    (decision) => decision.type === PENDING_DECISION_TYPE.AWAITING_MERGE,
  ).length;
  const goalRegistration = decisions.filter(
    (decision) => decision.type === PENDING_DECISION_TYPE.GOAL_REGISTRATION,
  ).length;
  return { total: decisions.length, inquiries, awaitingMerge, goalRegistration };
}

export const projectKey = (serverId: string, name: string): string => `${serverId}\u0000${name}`;

export function toRunView(
  run: Run,
  serverById: ReadonlyMap<string, Server>,
  thresholds: AnomalyThresholds,
  now: Date,
  goalsById?: ReadonlyMap<string, Goal>,
  projectByKey?: ReadonlyMap<string, Project>,
): RunView {
  const server = serverById.get(run.serverId);
  const lastStep = run.steps.at(-1);
  const goalName = goalsById?.get(run.goalId)?.name;
  const elapsedSeconds = run.lastLogAt
    ? Math.max(0, Math.floor((now.getTime() - run.lastLogAt.getTime()) / 1_000))
    : undefined;
  const derived = {
    run,
    server,
    goalName: goalName !== undefined && goalName !== "" ? goalName : undefined,
    currentStepName:
      run.currentStepName !== undefined && run.currentStepName !== "" ? run.currentStepName : undefined,
    model: lastStep?.model !== undefined && lastStep.model !== "" ? lastStep.model : undefined,
    stepNumber: run.currentStepNumber,
    totalSteps: run.totalSteps,
    iteration: run.currentIteration,
    elapsedSeconds,
  };
  const project = projectByKey?.get(projectKey(run.serverId, run.project));
  if (server !== undefined && (!server.connected || project?.connectionState === CONNECTION_STATE.UNRESPONSIVE)) {
    const lastReadAt = project?.lastReadAt ?? server.lastReadAt;
    const at = lastReadAt ? formatClock(lastReadAt) : "不明";
    return {
      ...derived,
      connected: false,
      state: "idle",
      label: `実行中（${at} 時点）`,
      note: "サーバーから応答がないため、最後に読めた時点の状態です。",
    };
  }
  if (project?.connectionState === CONNECTION_STATE.DELAYED) {
    const lastReadAt = project.lastReadAt ?? server?.lastReadAt;
    const seconds = lastReadAt ? Math.max(0, Math.floor((now.getTime() - lastReadAt.getTime()) / 1_000)) : undefined;
    return {
      ...derived,
      connected: true,
      state: "delayed",
      label: "実行中（読み取り遅延）",
      note:
        seconds !== undefined
          ? `読み取りが遅れています（最後に読めたのは ${seconds} 秒前）`
          : "読み取りが遅れています（最後に読めた時刻は不明です）",
    };
  }
  const silentMinutes = run.lastLogAt
    ? Math.max(0, Math.floor((now.getTime() - run.lastLogAt.getTime()) / 60_000))
    : undefined;
  const silent =
    run.lastLogAt !== undefined &&
    (run.logSilent === true ||
      (silentMinutes !== undefined && silentMinutes > thresholds.runLogSilentMinutes));
  if (silent && silentMinutes !== undefined) {
    return {
      ...derived,
      connected: true,
      state: "warn",
      label: "ログが止まっています",
      note: `ログが ${silentMinutes}分 出ていません。`,
    };
  }
  return { ...derived, connected: true, state: "live", label: "実行中", note: "" };
}

const serverState = (server: Server, runs: readonly Run[]): OverviewServerView["state"] => {
  if (!server.connected) {
    return "down";
  }
  return runs.length > 0 ? "live" : "idle";
};

const serverStateLabel = (state: OverviewServerView["state"]): string => {
  switch (state) {
    case "down":
      return "応答なし";
    case "live":
      return "動作中";
    case "idle":
      return "待機中";
  }
};

// 仕上げタスクが pending/running/failed などでまだ終わっていなければ、人のマージ待ちではなく
// 進行中のゴールとして数える。merge_task_last が未設定または完了なら人の判断待ちになる。
const MERGE_TASK_IN_PROGRESS: ReadonlySet<number> = new Set([
  TASK_STATUS.PENDING,
  TASK_STATUS.RUNNING,
  TASK_STATUS.FAILED,
  TASK_STATUS.EXCEEDED,
  TASK_STATUS.PR_FAILED,
]);

const isHumanMerge = (goal: Goal): boolean =>
  goal.status === GOAL_STATUS.AWAITING_MERGE &&
  !(goal.mergeTaskLast !== undefined && MERGE_TASK_IN_PROGRESS.has(goal.mergeTaskLast));

const isInProgressGoal = (goal: Goal): boolean =>
  goal.executionStatus !== EXECUTION_STATUS.ABORTED &&
  goal.status !== GOAL_STATUS.COMPLETED &&
  !isHumanMerge(goal);

const waitingTaskCount = (goals: readonly Goal[]): number =>
  goals.reduce((total, goal) => total + (goal.taskCounts?.pending ?? 0), 0);

const failedTaskCount = (goals: readonly Goal[]): number =>
  goals.reduce(
    (total, goal) =>
      total +
      (goal.taskCounts?.failed ?? 0) +
      (goal.taskCounts?.exceeded ?? 0) +
      (goal.taskCounts?.prFailed ?? 0),
    0,
  );

const isUnresponsiveProject = (project: Project): boolean =>
  project.connectionState === CONNECTION_STATE.UNRESPONSIVE;

// 応答のないサーバー・プロジェクトから最後に読めた時刻のうち、最も古いものを集計の基準にする。
// 1件も読めた時刻が無ければ undefined のままにし、実在しない時刻を補わない。
const earliestReadAt = (sources: ReadonlyArray<Date | undefined>): Date | undefined => {
  const times = sources
    .filter((value): value is Date => value !== undefined)
    .map((value) => value.getTime());
  return times.length === 0 ? undefined : new Date(Math.min(...times));
};

export function buildOverviewView(state: DashboardState, now: Date): OverviewView {
  const thresholds = resolveThresholds(state.thresholds);
  const serverById = new Map(state.servers.map((server) => [server.serverId, server]));
  const goalsById = new Map(state.goals.map((goal) => [goal.goalId, goal]));
  const projectByKey = new Map(
    state.projects.map((project) => [projectKey(project.serverId, project.name), project]),
  );
  const pendingDecisions = sortPendingDecisions(state.pendingDecisions, now, {
    staleThresholdMinutes: thresholds.pendingDecisionStaleMinutes,
  });
  const runs = state.runs.map((run) =>
    toRunView(run, serverById, thresholds, now, goalsById, projectByKey),
  );
  const unresponsiveServers = state.servers.filter((server) => !server.connected);
  const unresponsiveProjects = state.projects.filter(isUnresponsiveProject);
  const servers = state.servers.map((server) => {
    const serverRuns = state.runs.filter((run) => run.serverId === server.serverId);
    const serverGoals = state.goals.filter((goal) => goal.serverId === server.serverId);
    const status = serverState(server, serverRuns);
    return {
      server,
      state: status,
      stateLabel: serverStateLabel(status),
      runs: serverRuns.map((run) =>
        toRunView(run, serverById, thresholds, now, goalsById, projectByKey),
      ),
      runLimit: server.concurrency,
      runningRuns: serverRuns.filter((run) => run.status === RUN_STATUS.RUNNING).length,
      inProgressGoals: serverGoals.filter(isInProgressGoal).length,
      pendingCount: pendingDecisions.filter((decision) => decision.serverId === server.serverId).length,
      waitingTasks: waitingTaskCount(serverGoals),
    };
  });
  const runningRuns = state.runs.filter((run) => run.status === RUN_STATUS.RUNNING).length;
  const silentRunCount = runs.filter((run) => run.state === "warn").length;
  const failedTasks = failedTaskCount(state.goals);
  const abortedGoals = state.goals.filter(
    (goal) => goal.executionStatus === EXECUTION_STATUS.ABORTED,
  ).length;
  const pendingCounts = summarizePendingDecisions(pendingDecisions);
  const readAt = state.readAt ?? now;
  const partialReadAt = earliestReadAt(
    [...unresponsiveServers, ...unresponsiveProjects].map((item) => item.lastReadAt),
  );
  return {
    readAt,
    servers,
    projects: state.projects,
    runs,
    thresholds,
    runningRuns,
    runLimit: state.servers.reduce((total, server) => total + server.concurrency, 0),
    completedTasks24h: state.projects.reduce(
      (total, project) => total + project.completedTasks24h,
      0,
    ),
    failedTaskCount: failedTasks,
    abortedGoalCount: abortedGoals,
    silentRunCount,
    unresponsiveServers,
    unresponsiveProjects,
    partialReadAt,
    hasAnomalies:
      pendingCounts.total > 0 ||
      unresponsiveServers.length > 0 ||
      unresponsiveProjects.length > 0 ||
      silentRunCount > 0 ||
      failedTasks > 0 ||
      abortedGoals > 0,
    pendingCounts,
    pendingDecisions,
  };
}

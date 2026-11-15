import type {
  AnomalyThresholds,
  DashboardState,
  ExecutionStatus,
  Goal,
  GoalStatus,
  PendingDecision,
  Project,
  Run,
  Server,
} from "../proto/taktdash";
import { formatClock } from "./format";
import { CONNECTION_STATE, EXECUTION_STATUS, GOAL_STATUS, PENDING_DECISION_TYPE, RUN_STATUS } from "./proto-values";

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
};

type OverviewServerView = {
  server: Server;
  state: "live" | "idle" | "down";
  stateLabel: string;
  runs: RunView[];
};

export type OverviewView = {
  readAt: Date;
  servers: OverviewServerView[];
  projects: Project[];
  runs: RunView[];
  thresholds: AnomalyThresholds;
  runningRuns: number;
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
  return { total: decisions.length, inquiries, awaitingMerge };
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
  const servers = state.servers.map((server) => {
    const serverRuns = state.runs.filter((run) => run.serverId === server.serverId);
    const status = serverState(server, serverRuns);
    return {
      server,
      state: status,
      stateLabel: serverStateLabel(status),
      runs: serverRuns.map((run) =>
        toRunView(run, serverById, thresholds, now, goalsById, projectByKey),
      ),
    };
  });
  return {
    readAt: state.readAt ?? now,
    servers,
    projects: state.projects,
    runs,
    thresholds,
    runningRuns: state.runs.filter((run) => run.status === RUN_STATUS.RUNNING).length,
    pendingCounts: summarizePendingDecisions(pendingDecisions),
    pendingDecisions,
  };
}

export const goalStatusLabel = (status: GoalStatus): string => {
  switch (status) {
    case GOAL_STATUS.CREATED:
      return "作成済み";
    case GOAL_STATUS.AWAITING_MERGE:
      return "人のマージ待ち";
    case GOAL_STATUS.COMPLETED:
      return "完了";
    default:
      return "不明";
  }
};

export const executionStatusLabel = (status: ExecutionStatus): string => {
  switch (status) {
    case EXECUTION_STATUS.ACTIVE:
      return "稼働中";
    case EXECUTION_STATUS.PAUSED:
      return "一時停止中";
    case EXECUTION_STATUS.ABORTED:
      return "中止";
    default:
      return "不明";
  }
};

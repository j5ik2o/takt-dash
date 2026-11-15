import type {
  AnomalyThresholds,
  DashboardState,
  ExecutionStatus,
  GoalStatus,
  PendingDecision,
  Project,
  Run,
  Server,
} from "../proto/taktdash";
import { formatClock } from "./format";
import { EXECUTION_STATUS, GOAL_STATUS, PENDING_DECISION_TYPE, RUN_STATUS } from "./proto-values";

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
  label: string;
  note: string;
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

export function toRunView(
  run: Run,
  serverById: ReadonlyMap<string, Server>,
  projectById: ReadonlyMap<string, Project>,
  thresholds: AnomalyThresholds,
  now: Date,
): RunView {
  const server = serverById.get(run.serverId);
  const project = projectById.get(run.project);
  const serverConnected = server === undefined || server.connected;
  const projectConnected = project === undefined || project.connected;
  if (!serverConnected || !projectConnected) {
    const lastReadAt = projectConnected ? server?.lastReadAt : project?.lastReadAt;
    const at = lastReadAt ? formatClock(lastReadAt) : "不明";
    const note = projectConnected
      ? "サーバーから応答がないため、最後に読めた時点の状態です。"
      : "プロジェクトから応答がないため、最後に読めた時点の状態です。";
    return { run, server, connected: false, label: `実行中（${at} 時点）`, note };
  }
  if (run.logSilent && run.lastLogAt) {
    const silentMinutes = Math.round(minutesSince(run.lastLogAt, now));
    if (silentMinutes > thresholds.runLogSilentMinutes) {
      return { run, server, connected: true, label: "ログが止まっています", note: `ログが ${silentMinutes}分 出ていません。` };
    }
  }
  return { run, server, connected: true, label: "実行中", note: "" };
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
  const projectById = new Map(state.projects.map((project) => [project.name, project]));
  const pendingDecisions = sortPendingDecisions(state.pendingDecisions, now, {
    staleThresholdMinutes: thresholds.pendingDecisionStaleMinutes,
  });
  const runs = state.runs.map((run) => toRunView(run, serverById, projectById, thresholds, now));
  const servers = state.servers.map((server) => {
    const serverRuns = state.runs.filter((run) => run.serverId === server.serverId);
    const status = serverState(server, serverRuns);
    return {
      server,
      state: status,
      stateLabel: serverStateLabel(status),
      runs: serverRuns.map((run) => toRunView(run, serverById, projectById, thresholds, now)),
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

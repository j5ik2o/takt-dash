// SSE で受信した proto の JSON 表現を、画面が使う内部の型へ復元する。
//
// 送信側（app/api/state/route.ts）は DashboardState.toJSON の JSON を送る。これは
// proto の JSON 形式で、時刻は ISO 文字列になる。生成コード（src/proto/taktdash.ts）の
// fromJSON は @grpc/grpc-js を値として読み込むため、ブラウザ側のバンドルへ持ち込めない。
// そこで、時刻の復元には gRPC に依存しない Timestamp.fromJSON を使い、必要な
// フィールドだけを内部の型へ正規化する。生成コードとの意味の一致は
// test/state/dashboard.test.ts と test/components/state-stream.test.ts で確認する。
import type {
  AnomalyThresholds,
  DashboardState,
  Goal,
  PendingDecision,
  Project,
  ReportSummary,
  Run,
  RunStep,
  Server,
  TaskStatusCounts,
} from "../proto/taktdash";
import { Timestamp } from "../proto/google/protobuf/timestamp";
import { enumFromWire } from "./proto-values";

const toDate = (value: unknown): Date | undefined => {
  if (value === undefined || value === null) {
    return undefined;
  }
  const timestamp = Timestamp.fromJSON(value);
  return new Date(timestamp.seconds * 1_000 + Math.round(timestamp.nanos / 1_000_000));
};

const asNumber = (value: unknown): number => (typeof value === "number" ? value : Number(value ?? 0));

const asOptionalNumber = (value: unknown): number | undefined => (typeof value === "number" ? value : undefined);

const asOptionalString = (value: unknown): string | undefined =>
  typeof value === "string" && value !== "" ? value : undefined;

const toServer = (raw: Record<string, unknown>): Server => ({
  serverId: String(raw.serverId ?? ""),
  name: String(raw.name ?? ""),
  kind: String(raw.kind ?? ""),
  connected: raw.connected === true,
  lastReadAt: toDate(raw.lastReadAt),
  concurrency: asNumber(raw.concurrency),
});

const toProject = (raw: Record<string, unknown>): Project => ({
  name: String(raw.name ?? ""),
  about: String(raw.about ?? ""),
  serverId: String(raw.serverId ?? ""),
  completedTasks24h: asNumber(raw.completedTasks24h),
});

const toTaskCounts = (raw: unknown): TaskStatusCounts | undefined => {
  if (raw === undefined || raw === null) {
    return undefined;
  }
  const value = raw as Record<string, unknown>;
  return {
    pending: asNumber(value.pending),
    running: asNumber(value.running),
    completed: asNumber(value.completed),
    failed: asNumber(value.failed),
    exceeded: asNumber(value.exceeded),
    prFailed: asNumber(value.prFailed),
  };
};

const toGoal = (raw: Record<string, unknown>): Goal => ({
  goalId: String(raw.goalId ?? ""),
  serverId: String(raw.serverId ?? ""),
  project: String(raw.project ?? ""),
  name: String(raw.name ?? ""),
  status: enumFromWire("GoalStatus", raw.status),
  executionStatus: enumFromWire("ExecutionStatus", raw.executionStatus),
  taskCounts: toTaskCounts(raw.taskCounts),
  pausedReason: typeof raw.pausedReason === "string" ? raw.pausedReason : undefined,
  pullRequest:
    raw.pullRequest !== null && typeof raw.pullRequest === "object"
      ? {
          number: asNumber((raw.pullRequest as Record<string, unknown>).number),
          url: String((raw.pullRequest as Record<string, unknown>).url ?? ""),
        }
      : undefined,
  awaitingMergeAt: toDate(raw.awaitingMergeAt),
  mergeTaskLast:
    raw.mergeTaskLast === undefined || raw.mergeTaskLast === null
      ? undefined
      : enumFromWire("TaskStatus", raw.mergeTaskLast),
});

const toRunStep = (raw: Record<string, unknown>): RunStep => ({
  stepName: String(raw.stepName ?? ""),
  model: String(raw.model ?? ""),
  startedAt: toDate(raw.startedAt),
  endedAt: toDate(raw.endedAt),
});

const toReportSummary = (raw: Record<string, unknown>): ReportSummary => ({
  filename: String(raw.filename ?? ""),
  sizeBytes: asNumber(raw.sizeBytes),
});

const toRun = (raw: Record<string, unknown>): Run => ({
  runSlug: String(raw.runSlug ?? ""),
  sessionId: String(raw.sessionId ?? ""),
  serverId: String(raw.serverId ?? ""),
  project: String(raw.project ?? ""),
  goalId: String(raw.goalId ?? ""),
  taskName: String(raw.taskName ?? ""),
  summary: String(raw.summary ?? ""),
  startedAt: toDate(raw.startedAt),
  steps: Array.isArray(raw.steps) ? raw.steps.map((step) => toRunStep(step as Record<string, unknown>)) : [],
  currentStepStartedAt: toDate(raw.currentStepStartedAt),
  lastLogAt: toDate(raw.lastLogAt),
  reports: Array.isArray(raw.reports) ? raw.reports.map((report) => toReportSummary(report as Record<string, unknown>)) : [],
  status: enumFromWire("RunStatus", raw.status),
  logSilent: raw.logSilent === true,
  currentStepNumber: asOptionalNumber(raw.currentStepNumber),
  totalSteps: asOptionalNumber(raw.totalSteps),
  currentIteration: asOptionalNumber(raw.currentIteration),
  currentStepName: asOptionalString(raw.currentStepName),
});

const toPendingDecision = (raw: Record<string, unknown>): PendingDecision => ({
  type: enumFromWire("PendingDecisionType", raw.type),
  serverId: String(raw.serverId ?? ""),
  project: String(raw.project ?? ""),
  goalId: typeof raw.goalId === "string" ? raw.goalId : undefined,
  inquiryKind: enumFromWire("InquiryKind", raw.inquiryKind),
  createdAt: toDate(raw.createdAt),
  body: String(raw.body ?? ""),
  options: Array.isArray(raw.options) ? raw.options.map((option) => String(option)) : [],
  pullRequest:
    raw.pullRequest !== null && typeof raw.pullRequest === "object"
      ? {
          number: asNumber((raw.pullRequest as Record<string, unknown>).number),
          url: String((raw.pullRequest as Record<string, unknown>).url ?? ""),
        }
      : undefined,
  stale: raw.stale === true,
});

const toThresholds = (raw: unknown): AnomalyThresholds | undefined => {
  if (raw === undefined || raw === null) {
    return undefined;
  }
  const value = raw as Record<string, unknown>;
  return {
    pendingDecisionStaleMinutes: asNumber(value.pendingDecisionStaleMinutes),
    runLogSilentMinutes: asNumber(value.runLogSilentMinutes),
  };
};

// proto の JSON 表現（文字列の時刻を含む）を、内部の DashboardState へ復元する。
export function restoreDashboardState(payload: unknown): DashboardState {
  const raw = (payload ?? {}) as Record<string, unknown>;
  const arrayOf = (value: unknown): Record<string, unknown>[] =>
    Array.isArray(value) ? value.map((item) => item as Record<string, unknown>) : [];
  return {
    readAt: toDate(raw.readAt),
    servers: arrayOf(raw.servers).map(toServer),
    projects: arrayOf(raw.projects).map(toProject),
    goals: arrayOf(raw.goals).map(toGoal),
    runs: arrayOf(raw.runs).map(toRun),
    pendingDecisions: arrayOf(raw.pendingDecisions).map(toPendingDecision),
    thresholds: toThresholds(raw.thresholds),
  };
}

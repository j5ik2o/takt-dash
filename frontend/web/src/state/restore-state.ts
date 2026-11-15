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
  GetGoalDetailResponse,
  GetTaskDetailResponse,
  Goal,
  GoalQuestion,
  GoalTask,
  GoalWorkLog,
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

const arrayOf = (value: unknown): Record<string, unknown>[] =>
  Array.isArray(value) ? value.map((item) => item as Record<string, unknown>) : [];

const toStringArray = (value: unknown): string[] =>
  Array.isArray(value) ? value.map((item) => String(item)) : [];

const toOptionalString = (value: unknown): string | undefined =>
  typeof value === "string" ? value : undefined;

const toOptionalNumber = (value: unknown): number | undefined =>
  value === undefined || value === null ? undefined : asNumber(value);

const toSpecifications = (value: unknown): { [key: string]: string } => {
  if (value === null || typeof value !== "object") {
    return {};
  }
  return Object.fromEntries(
    Object.entries(value as Record<string, unknown>).map(([key, item]) => [key, String(item)]),
  );
};

const toGoalWorkLog = (raw: Record<string, unknown>): GoalWorkLog => ({
  workLogId: String(raw.workLogId ?? ""),
  recordedAt: toDate(raw.recordedAt),
  summary: String(raw.summary ?? ""),
});

const toGoalQuestion = (raw: Record<string, unknown>): GoalQuestion => ({
  questionId: String(raw.questionId ?? ""),
  kind: String(raw.kind ?? ""),
  status: String(raw.status ?? ""),
  body: String(raw.body ?? ""),
  options: toStringArray(raw.options),
  answerText: toOptionalString(raw.answerText),
  answerDecision: toOptionalString(raw.answerDecision),
  answerReport: toOptionalString(raw.answerReport),
  createdAt: toDate(raw.createdAt),
  answeredAt: toDate(raw.answeredAt),
});

const toGoalTask = (raw: Record<string, unknown>): GoalTask => ({
  taskName: String(raw.taskName ?? ""),
  status: enumFromWire("TaskStatus", raw.status),
  runSlug: toOptionalString(raw.runSlug),
});

// proto の JSON 表現（文字列の時刻を含む）を、内部の DashboardState へ復元する。
export function restoreDashboardState(payload: unknown): DashboardState {
  const raw = (payload ?? {}) as Record<string, unknown>;
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

// ゴール詳細の Route Handler が返す JSON を、画面の内部型へ復元する。
export function restoreGoalDetail(payload: unknown): GetGoalDetailResponse {
  const raw = (payload ?? {}) as Record<string, unknown>;
  return {
    goalId: String(raw.goalId ?? ""),
    serverId: String(raw.serverId ?? ""),
    project: String(raw.project ?? ""),
    status: enumFromWire("GoalStatus", raw.status),
    executionStatus: enumFromWire("ExecutionStatus", raw.executionStatus),
    objective: String(raw.objective ?? ""),
    outOfScope: toStringArray(raw.outOfScope),
    acceptanceCriteria: toStringArray(raw.acceptanceCriteria),
    constraints: toStringArray(raw.constraints),
    specifications: toSpecifications(raw.specifications),
    branch: String(raw.branch ?? ""),
    integrationBranch: String(raw.integrationBranch ?? ""),
    workLog: arrayOf(raw.workLog).map(toGoalWorkLog),
    questions: arrayOf(raw.questions).map(toGoalQuestion),
    tasks: arrayOf(raw.tasks).map(toGoalTask),
  };
}

// タスク詳細の Route Handler が返す JSON を、画面の内部型へ復元する。
export function restoreTaskDetail(payload: unknown): GetTaskDetailResponse {
  const raw = (payload ?? {}) as Record<string, unknown>;
  return {
    project: String(raw.project ?? ""),
    taskName: String(raw.taskName ?? ""),
    goalId: toOptionalString(raw.goalId),
    status: enumFromWire("TaskStatus", raw.status),
    summary: String(raw.summary ?? ""),
    startedAt: toDate(raw.startedAt),
    runSlug: toOptionalString(raw.runSlug),
    pullRequestNumber: toOptionalNumber(raw.pullRequestNumber),
    linkedRunSlugs: toStringArray(raw.linkedRunSlugs),
    branch: toOptionalString(raw.branch),
    pullRequestUrl: toOptionalString(raw.pullRequestUrl),
  };
}

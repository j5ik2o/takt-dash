// SSE で受信した proto の JSON 表現と、RSC が渡す proto の初期状態を、画面が使う
// 内部の型へ復元する。
//
// 送信側（app/api/state/route.ts）は DashboardState.toJSON の JSON を送る。これは
// proto の JSON 形式で、時刻は ISO 文字列になる。生成コード（src/proto/taktdash.ts）の
// fromJSON は @grpc/grpc-js を値として読み込むため、ブラウザ側のバンドルへ持ち込めない。
// そこで、時刻の復元には gRPC に依存しない Timestamp.fromJSON を使い、ドメイン上の値は
// domain-primitives.ts の検証関数を通して内部の型へ復元する。生成コードとの意味の一致は
// test/state/dashboard.test.ts と test/components/state-stream.test.ts で確認する。
import type {
  BuildInfo,
  DashboardState as ProtoDashboardState,
  GetGoalDetailResponse,
  GetTaskDetailResponse,
  Goal as ProtoGoal,
  GoalQuestion,
  GoalTask,
  GoalWorkLog,
  PendingDecision as ProtoPendingDecision,
  Project as ProtoProject,
  PullRequest as ProtoPullRequest,
  Run as ProtoRun,
  Server as ProtoServer,
} from "../proto/taktdash";
import {
  toGoalId,
  toInquiryId,
  toProjectName,
  toPullRequestNumber,
  toRegistrationRevision,
  toRunSlug,
  toServerId,
  toSilentMinutes,
  toStaleMinutes,
  toTaskCount,
  toTaskName,
  type GoalId,
  type InquiryId,
  type ProjectName,
  type PullRequestNumber,
  type RegistrationRevision,
  type RunSlug,
  type ServerId,
  type TaskCount,
  type TaskName,
  type ValidatedAnomalyThresholds,
} from "./domain-primitives";
import { enumFromWire, enumFromWireOrUnrecognizedName } from "./proto-values";
import { toDate, recordOf, asOptionalNumber, asOptionalString, arrayOf, toStringArray } from "./wire-values";
import { restoreWorkflowProgress } from "./workflow-progress-wire";

export type ValidatedTaskStatusCounts = {
  pending: TaskCount;
  running: TaskCount;
  completed: TaskCount;
  failed: TaskCount;
  exceeded: TaskCount;
  prFailed: TaskCount;
};

export type DashboardPullRequest = Omit<ProtoPullRequest, "number"> & {
  number: PullRequestNumber;
};

export type DashboardServer = Omit<ProtoServer, "serverId"> & {
  serverId: ServerId;
};

export type DashboardProject = Omit<ProtoProject, "name" | "serverId"> & {
  name: ProjectName;
  serverId: ServerId;
};

export type DashboardGoal = Omit<
  ProtoGoal,
  "goalId" | "serverId" | "project" | "taskCounts" | "pullRequest"
> & {
  goalId: GoalId;
  serverId: ServerId;
  project: ProjectName;
  taskCounts: ValidatedTaskStatusCounts | undefined;
  pullRequest: DashboardPullRequest | undefined;
};

// run はどのゴールにも属さない場合がある（backend の RunEntry.goal_id は Option）。wire は
// 空文字または欠落になるため、未指定を undefined で表す。
export type DashboardRun = Omit<
  ProtoRun,
  "runSlug" | "serverId" | "project" | "goalId" | "taskName"
> & {
  runSlug: RunSlug;
  serverId: ServerId;
  project: ProjectName;
  goalId: GoalId | undefined;
  taskName: TaskName;
};

export type DashboardPendingDecision = Omit<
  ProtoPendingDecision,
  "serverId" | "project" | "goalId" | "pullRequest" | "inquiryId" | "revision"
> & {
  serverId: ServerId;
  project: ProjectName;
  goalId: GoalId | undefined;
  pullRequest: DashboardPullRequest | undefined;
  // 問い合わせ以外の判断待ちでは未指定。proto の文字列境界では空文字へ射影する。
  inquiryId: InquiryId | undefined;
  revision: RegistrationRevision | undefined;
};

export type DashboardState = Omit<
  ProtoDashboardState,
  "servers" | "projects" | "goals" | "runs" | "pendingDecisions" | "thresholds"
> & {
  servers: DashboardServer[];
  projects: DashboardProject[];
  goals: DashboardGoal[];
  runs: DashboardRun[];
  pendingDecisions: DashboardPendingDecision[];
  thresholds: ValidatedAnomalyThresholds | undefined;
};


const asNumber = (value: unknown): number => (typeof value === "number" ? value : Number(value ?? 0));




// 任意のゴール ID。欠落・空文字は未指定の run やゴール非所属の判断待ちとして undefined にする。
const optionalGoalId = (value: unknown): GoalId | undefined => {
  if (value === undefined || value === null || value === "") {
    return undefined;
  }
  return toGoalId(value);
};

// 任意の inquiry ID。欠落・空文字は「問い合わせ以外」として undefined にし、空白のみは
// テキスト検証で lexical 拒否する。"" as InquiryId のような検証の迂回はしない。
const optionalInquiryId = (value: unknown): InquiryId | undefined => {
  if (value === undefined || value === null || value === "") {
    return undefined;
  }
  return toInquiryId(value);
};

const toPullRequest = (raw: unknown): DashboardPullRequest | undefined => {
  if (raw === undefined || raw === null || typeof raw !== "object") {
    return undefined;
  }
  const value = recordOf(raw);
  return {
    number: toPullRequestNumber(value.number),
    url: String(value.url ?? ""),
  };
};

const toTaskCounts = (raw: unknown): ValidatedTaskStatusCounts | undefined => {
  if (raw === undefined || raw === null) {
    return undefined;
  }
  const value = recordOf(raw);
  // wire が 0 を省略するため、欠落は 0 として復元する。負数・上限超・非整数は拒否する。
  return {
    pending: toTaskCount(value.pending ?? 0),
    running: toTaskCount(value.running ?? 0),
    completed: toTaskCount(value.completed ?? 0),
    failed: toTaskCount(value.failed ?? 0),
    exceeded: toTaskCount(value.exceeded ?? 0),
    prFailed: toTaskCount(value.prFailed ?? 0),
  };
};

const toServer = (raw: unknown): DashboardServer => {
  const value = recordOf(raw);
  return {
    serverId: toServerId(value.serverId),
    name: String(value.name ?? ""),
    kind: String(value.kind ?? ""),
    connected: value.connected === true,
    lastReadAt: toDate(value.lastReadAt),
    writeOnly: value.writeOnly === true,
  };
};

const toProject = (raw: unknown): DashboardProject => {
  const value = recordOf(raw);
  return {
    name: toProjectName(value.name),
    about: String(value.about ?? ""),
    serverId: toServerId(value.serverId),
    completedTasks24h: asNumber(value.completedTasks24h),
    concurrency: asNumber(value.concurrency),
    connected: value.connected === true,
    lastReadAt: toDate(value.lastReadAt),
    connectionState:
      value.connectionState === undefined || value.connectionState === null
        ? 0
        : enumFromWire("ConnectionState", value.connectionState),
  };
};

const toGoal = (raw: unknown): DashboardGoal => {
  const value = recordOf(raw);
  return {
    goalId: toGoalId(value.goalId),
    nameTruncated: value.nameTruncated === true,
    serverId: toServerId(value.serverId),
    project: toProjectName(value.project),
    name: String(value.name ?? ""),
    status: enumFromWire("GoalStatus", value.status),
    executionStatus: enumFromWire("ExecutionStatus", value.executionStatus),
    taskCounts: toTaskCounts(value.taskCounts),
    pausedReason: typeof value.pausedReason === "string" ? value.pausedReason : undefined,
    pullRequest: toPullRequest(value.pullRequest),
    awaitingMergeAt: toDate(value.awaitingMergeAt),
    mergeTaskLast:
      value.mergeTaskLast === undefined || value.mergeTaskLast === null
        ? undefined
        : enumFromWire("TaskStatus", value.mergeTaskLast),
  };
};

const toRunStep = (raw: unknown) => {
  const value = recordOf(raw);
  return {
    stepName: String(value.stepName ?? ""),
    model: String(value.model ?? ""),
    startedAt: toDate(value.startedAt),
    endedAt: toDate(value.endedAt),
  };
};

const toReportSummary = (raw: unknown) => {
  const value = recordOf(raw);
  return {
    filename: String(value.filename ?? ""),
    sizeBytes: asNumber(value.sizeBytes),
  };
};

const toRun = (raw: unknown): DashboardRun => {
  const value = recordOf(raw);
  return {
    runSlug: toRunSlug(value.runSlug),
    sessionId: String(value.sessionId ?? ""),
    serverId: toServerId(value.serverId),
    project: toProjectName(value.project),
    goalId: optionalGoalId(value.goalId),
    taskName: toTaskName(value.taskName),
    summary: String(value.summary ?? ""),
    startedAt: toDate(value.startedAt),
    steps: arrayOf(value.steps).map(toRunStep),
    currentStepStartedAt: toDate(value.currentStepStartedAt),
    lastLogAt: toDate(value.lastLogAt),
    reports: arrayOf(value.reports).map(toReportSummary),
    status: enumFromWire("RunStatus", value.status),
    logSilent: value.logSilent === true,
    currentStepNumber: asOptionalNumber(value.currentStepNumber),
    totalSteps: asOptionalNumber(value.totalSteps),
    currentIteration: asOptionalNumber(value.currentIteration),
    currentStepName: asOptionalString(value.currentStepName),
    progress: restoreWorkflowProgress(value.progress),
  };
};

const toPendingDecision = (raw: unknown): DashboardPendingDecision => {
  const value = recordOf(raw);
  return {
    type: enumFromWire("PendingDecisionType", value.type),
    serverId: toServerId(value.serverId),
    project: toProjectName(value.project),
    goalId: optionalGoalId(value.goalId),
    inquiryKind: enumFromWireOrUnrecognizedName("InquiryKind", value.inquiryKind),
    createdAt: toDate(value.createdAt),
    body: String(value.body ?? ""),
    options: Array.isArray(value.options) ? value.options.map((option) => String(option)) : [],
    pullRequest: toPullRequest(value.pullRequest),
    stale: value.stale === true,
    inquiryId: optionalInquiryId(value.inquiryId),
    revision:
      value.revision === undefined || value.revision === null
        ? undefined
        : toRegistrationRevision(value.revision),
  };
};

const toThresholds = (raw: unknown): ValidatedAnomalyThresholds | undefined => {
  if (raw === undefined || raw === null) {
    return undefined;
  }
  const value = recordOf(raw);
  // wire が 0 を省略するため、欠落は 0 として復元する。範囲外・非整数は拒否する。
  return {
    pendingDecisionStaleMinutes: toStaleMinutes(value.pendingDecisionStaleMinutes ?? 0),
    runLogSilentMinutes: toSilentMinutes(value.runLogSilentMinutes ?? 0),
  };
};

const toBuildInfo = (raw: unknown): BuildInfo | undefined => {
  if (raw === undefined || raw === null || typeof raw !== "object") {
    return undefined;
  }
  const value = raw as Record<string, unknown>;
  return {
    commit: String(value.commit ?? ""),
    branch: String(value.branch ?? ""),
  };
};



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
  const raw = recordOf(payload);
  return {
    readAt: toDate(raw.readAt),
    servers: arrayOf(raw.servers).map(toServer),
    projects: arrayOf(raw.projects).map(toProject),
    goals: arrayOf(raw.goals).map(toGoal),
    runs: arrayOf(raw.runs).map(toRun),
    pendingDecisions: arrayOf(raw.pendingDecisions).map(toPendingDecision),
    thresholds: toThresholds(raw.thresholds),
    buildInfo: toBuildInfo(raw.buildInfo),
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
    createdAt: toDate(raw.createdAt),
    startedAt: toDate(raw.startedAt),
    completedAt: toDate(raw.completedAt),
    runSlug: toOptionalString(raw.runSlug),
    pullRequestNumber: toOptionalNumber(raw.pullRequestNumber),
    linkedRunSlugs: toStringArray(raw.linkedRunSlugs),
    branch: toOptionalString(raw.branch),
    pullRequestUrl: toOptionalString(raw.pullRequestUrl),
  };
}

// RSC が渡す proto の初期状態も、SSE と同じエンティティ単位の正規化関数を通してから
// 画面の型にする。フィールドの列挙と検証規則は各エンティティ関数に1か所だけ置く。
export function dashboardStateFromProto(state: ProtoDashboardState): DashboardState {
  return {
    readAt: toDate(state.readAt),
    servers: state.servers.map(toServer),
    projects: state.projects.map(toProject),
    goals: state.goals.map(toGoal),
    runs: state.runs.map(toRun),
    pendingDecisions: state.pendingDecisions.map(toPendingDecision),
    thresholds: toThresholds(state.thresholds),
    buildInfo: toBuildInfo(state.buildInfo),
  };
}

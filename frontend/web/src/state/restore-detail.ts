// ゴール詳細とタスク詳細の復元。restore-state.ts がファイル行数の関門（400 行）を
// 超えないよう、詳細画面専用の復元関数をこのファイルへ分けた。共有の補助は
// ここで定義して export し、restore-state.ts からも import して使うことで、
// 振る舞いを変えずに両方から使えるようにする。
import type {
  GetGoalDetailResponse,
  GetTaskDetailResponse,
  GoalQuestion,
  GoalTask,
  GoalWorkLog,
} from "../proto/taktdash";
import { Timestamp } from "../proto/google/protobuf/timestamp";
import { enumFromWire } from "./proto-values";

export const toDate = (value: unknown): Date | undefined => {
  if (value === undefined || value === null) {
    return undefined;
  }
  if (value instanceof Date) {
    return value;
  }
  const timestamp = Timestamp.fromJSON(value);
  return new Date(timestamp.seconds * 1_000 + Math.round(timestamp.nanos / 1_000_000));
};

const asNumber = (value: unknown): number => (typeof value === "number" ? value : Number(value ?? 0));

export const arrayOf = (value: unknown): Record<string, unknown>[] =>
  Array.isArray(value) ? value.map((item) => item as Record<string, unknown>) : [];

export const toStringArray = (value: unknown): string[] =>
  Array.isArray(value) ? value.map((item) => String(item)) : [];

export const toOptionalString = (value: unknown): string | undefined =>
  typeof value === "string" ? value : undefined;

export const toOptionalNumber = (value: unknown): number | undefined =>
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

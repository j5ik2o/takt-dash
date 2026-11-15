// proto のメッセージ型に合わせたテスト用のフィクスチャ。
//
// 生成物の create 関数を使うため、フィールド名・enum 値が proto の契約から
// ずれると型検査で失敗する。テストごとに必要な値だけを上書きして使い回さない。
import {
  ActivityKind,
  AnomalyThresholds,
  DashboardState,
  ExecutionStatus,
  GetGoalDetailResponse,
  GetTaskDetailResponse,
  GoalStatus,
  InquiryKind,
  PendingDecisionType,
  RunStatus,
  TaskStatus,
  type ActivityEvent,
  type DailySummary,
  type GetActivityResponse,
  type GetProjectsResponse,
  type GetRunDetailResponse,
  type Goal,
  type ServerEventCount,
} from "../../src/proto/taktdash.ts";

export const STALE_MINUTES = 240;
export const SILENT_MINUTES = 10;

export const NOW = new Date("2026-11-13T07:30:00Z");

const minutesAgo = (minutes: number): Date => new Date(NOW.getTime() - minutes * 60_000);

export function createThresholds(): AnomalyThresholds {
  return AnomalyThresholds.create({
    pendingDecisionStaleMinutes: STALE_MINUTES,
    runLogSilentMinutes: SILENT_MINUTES,
  });
}

export function createDashboardState(): DashboardState {
  return DashboardState.create({
    readAt: NOW,
    servers: [
      {
        serverId: "mini",
        name: "Mac mini",
        kind: "Mac mini",
        connected: true,
        lastReadAt: minutesAgo(1),
        concurrency: 4,
      },
      {
        serverId: "down",
        name: "Mac Studio",
        kind: "Mac Studio",
        connected: false,
        lastReadAt: minutesAgo(21),
        concurrency: 4,
      },
    ],
    projects: [
      { name: "corporate-site", about: "採用ページを管理する", serverId: "mini", completedTasks24h: 3 },
      { name: "esa", about: "イベントストアの実装", serverId: "mini", completedTasks24h: 1 },
    ],
    goals: [
      {
        goalId: "goal-1",
        serverId: "mini",
        project: "corporate-site",
        name: "採用ページのレイアウトを直す",
        status: GoalStatus.GOAL_STATUS_AWAITING_MERGE,
        executionStatus: ExecutionStatus.EXECUTION_STATUS_ACTIVE,
        taskCounts: { pending: 0, running: 0, completed: 6, failed: 0, exceeded: 0, prFailed: 0 },
        pullRequest: { number: 418, url: "https://example.test/pr/418" },
        awaitingMergeAt: minutesAgo(300),
      },
      {
        goalId: "goal-2",
        serverId: "mini",
        project: "esa",
        name: "v2.4.0 を公開する",
        status: GoalStatus.GOAL_STATUS_CREATED,
        executionStatus: ExecutionStatus.EXECUTION_STATUS_PAUSED,
        taskCounts: { pending: 1, running: 0, completed: 5, failed: 1, exceeded: 0, prFailed: 0 },
        pausedReason: "公開の日取りが決まるまで止めています。",
      },
    ],
    runs: [
      {
        runSlug: "run-aaa",
        sessionId: "session-aaa",
        serverId: "mini",
        project: "corporate-site",
        goalId: "goal-1",
        taskName: "採用ページのレイアウトを直す",
        summary: "レイアウトを直しています。",
        startedAt: minutesAgo(35),
        steps: [{ stepName: "implement", model: "glm-5.3", startedAt: minutesAgo(20) }],
        currentStepStartedAt: minutesAgo(20),
        lastLogAt: minutesAgo(1),
        reports: [{ filename: "review.md", sizeBytes: 1200 }],
        status: RunStatus.RUN_STATUS_RUNNING,
        logSilent: false,
      },
    ],
    pendingDecisions: [
      {
        type: PendingDecisionType.PENDING_DECISION_TYPE_INQUIRY,
        serverId: "mini",
        project: "esa",
        goalId: "goal-2",
        inquiryKind: InquiryKind.INQUIRY_KIND_QUESTION,
        createdAt: minutesAgo(30),
        body: "リリースの日取りはどうしますか。",
        options: ["今週", "来週"],
        stale: false,
      },
      {
        type: PendingDecisionType.PENDING_DECISION_TYPE_AWAITING_MERGE,
        serverId: "mini",
        project: "corporate-site",
        goalId: "goal-1",
        inquiryKind: InquiryKind.INQUIRY_KIND_UNSPECIFIED,
        createdAt: minutesAgo(300),
        body: "プルリクエスト #418 のマージを待っています。",
        options: [],
        pullRequest: { number: 418, url: "https://example.test/pr/418" },
        stale: true,
      },
    ],
    thresholds: createThresholds(),
  });
}

export function createActivityEvents(): ActivityEvent[] {
  return [
    {
      occurredAt: minutesAgo(5),
      serverId: "mini",
      project: "corporate-site",
      kind: ActivityKind.ACTIVITY_KIND_AWAITING_MERGE,
      body: "ゴール「採用ページのレイアウトを直す」が人のマージ待ちになりました。",
      runSlug: "run-aaa",
    },
    {
      occurredAt: minutesAgo(40),
      serverId: "mini",
      project: "esa",
      kind: ActivityKind.ACTIVITY_KIND_INQUIRY,
      body: "問い合わせが届きました。",
    },
    {
      occurredAt: minutesAgo(90),
      serverId: "down",
      kind: ActivityKind.ACTIVITY_KIND_CONNECTION_LOST,
      body: "接続が切れました。",
    },
  ];
}

export function createDailySummaries(): DailySummary[] {
  return Array.from({ length: 7 }, (_, index) => ({
    date: `2026-11-${String(7 + index).padStart(2, "0")}`,
    total: 10 + index,
    attention: index % 3,
  }));
}

export function createServerEventCounts(): ServerEventCount[] {
  return [
    { serverId: "mini", count: 12 },
    { serverId: "down", count: 3 },
  ];
}

export function createProjectsResponse(): GetProjectsResponse {
  return { state: createDashboardState(), recentEvents: createActivityEvents() };
}

export function createActivityResponse(): GetActivityResponse {
  return {
    state: createDashboardState(),
    events: createActivityEvents(),
    dailySummaries: createDailySummaries(),
    todayCountsByServer: createServerEventCounts(),
  };
}

export function createRunDetailResponse(runSlug = "run-aaa"): GetRunDetailResponse {
  return {
    runSlug,
    sessionId: "session-aaa",
    serverId: "mini",
    project: "corporate-site",
    goalId: "goal-1",
    taskName: "採用ページのレイアウトを直す",
    taskText: "採用ページのレイアウトを直すという依頼です。",
    summary: "レイアウトを確認しています。",
    workflow: "simple-mini",
    status: RunStatus.RUN_STATUS_RUNNING,
    startedAt: minutesAgo(35),
    currentStep: "implement",
    currentIteration: 2,
    currentStepStartedAt: minutesAgo(20),
    currentStepExecutionCount: 1,
    phase: 1,
    stepLogs: [
      {
        stepName: "plan",
        persona: "planner",
        status: "done",
        content: "計画をまとめました。",
        timestamp: minutesAgo(30),
        iteration: 1,
      },
      {
        stepName: "implement",
        persona: "coder",
        status: "running",
        content: "変更を加えています。",
        timestamp: minutesAgo(20),
        iteration: 2,
      },
    ],
    logTail: "2026-11-13T07:29:59Z 最後のログの行です。",
    reports: [{ filename: "review.md", content: "指摘はありません。", sizeBytes: 1200 }],
    instructions: [
      { instructionId: 7, issuedAt: minutesAgo(10), state: "accepted", content: "サマリーを短くしてください。" },
    ],
    lastLogAt: minutesAgo(1),
    logSilent: false,
    serverConnected: true,
    serverLastReadAt: minutesAgo(1),
  };
}

export function createGoal(overrides: Partial<Goal> = {}): Goal {
  return {
    goalId: "goal-1",
    serverId: "mini",
    project: "corporate-site",
    name: "採用ページのレイアウトを直す",
    status: GoalStatus.GOAL_STATUS_CREATED,
    executionStatus: ExecutionStatus.EXECUTION_STATUS_ACTIVE,
    taskCounts: { pending: 0, running: 0, completed: 0, failed: 0, exceeded: 0, prFailed: 0 },
    pullRequest: undefined,
    ...overrides,
  };
}

export function createGoalDetailResponse(
  overrides: Partial<GetGoalDetailResponse> = {},
): GetGoalDetailResponse {
  return GetGoalDetailResponse.create({
    goalId: "goal-1",
    serverId: "mini",
    project: "corporate-site",
    status: GoalStatus.GOAL_STATUS_AWAITING_MERGE,
    executionStatus: ExecutionStatus.EXECUTION_STATUS_ACTIVE,
    objective: "採用ページのレイアウトを直す目的です。",
    outOfScope: ["管理画面の変更は対象外です。"],
    acceptanceCriteria: ["採用ページのレイアウトが崩れないこと。", "スマートフォンで読めること。"],
    constraints: ["既存の配色を変えないこと。"],
    specifications: {},
    branch: "goal/45-frontend",
    integrationBranch: "main",
    workLog: [
      { workLogId: "w1", recordedAt: minutesAgo(50), summary: "最初の作業記録です。" },
      { workLogId: "w2", recordedAt: minutesAgo(10), summary: "次の作業記録です。" },
    ],
    questions: [
      {
        questionId: "q1",
        kind: "question",
        status: "answered",
        body: "配色は変えてよいですか。",
        options: [],
        answerText: "変えないでください。",
        answerDecision: "deny",
        createdAt: minutesAgo(40),
        answeredAt: minutesAgo(35),
      },
      {
        questionId: "q2",
        kind: "operation_request",
        status: "pending",
        body: "スクリーンショットを撮ってよいですか。",
        options: ["実施した", "対応できない"],
        createdAt: minutesAgo(5),
      },
    ],
    tasks: [
      {
        taskName: "採用ページのレイアウトを直す",
        status: TaskStatus.TASK_STATUS_COMPLETED,
        runSlug: "run-aaa",
      },
      { taskName: "タスク/2", status: TaskStatus.TASK_STATUS_RUNNING, runSlug: "run-bbb" },
    ],
    ...overrides,
  });
}

export function createTaskDetailResponse(
  overrides: Partial<GetTaskDetailResponse> = {},
): GetTaskDetailResponse {
  return GetTaskDetailResponse.create({
    project: "corporate-site",
    taskName: "採用ページのレイアウトを直す",
    goalId: "goal-1",
    status: TaskStatus.TASK_STATUS_COMPLETED,
    summary: "レイアウトを直して、確認を待っています。",
    startedAt: minutesAgo(35),
    runSlug: "run-aaa",
    pullRequestNumber: 418,
    linkedRunSlugs: ["run-aaa", "run-bbb"],
    branch: "goal/45-frontend",
    pullRequestUrl: "https://example.test/pr/418",
    ...overrides,
  });
}

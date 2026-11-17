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
  TaskStatus,
} from "../proto/taktdash";
import { formatClock } from "./format";
import { goalSection } from "./goal-badge";
import {
  toRunCount,
  toRunLimit,
  toSilentMinutes,
  toStaleMinutes,
  type RunCount,
  type RunLimit,
  type ValidatedAnomalyThresholds,
} from "./domain-primitives";
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
export const DEFAULT_ANOMALY_THRESHOLDS: ValidatedAnomalyThresholds = {
  pendingDecisionStaleMinutes: toStaleMinutes(240),
  runLogSilentMinutes: toSilentMinutes(10),
};

// proto の閾値があればそれを検証して使い、無い応答だけ既定値へ解決する。全画面がこの関数を
// 通すことで、既定値の食い違いと範囲外の値の混入を防ぐ。
export function resolveThresholds(
  thresholds: AnomalyThresholds | undefined,
): ValidatedAnomalyThresholds {
  if (thresholds === undefined) {
    return DEFAULT_ANOMALY_THRESHOLDS;
  }
  return {
    pendingDecisionStaleMinutes: toStaleMinutes(thresholds.pendingDecisionStaleMinutes),
    runLogSilentMinutes: toSilentMinutes(thresholds.runLogSilentMinutes),
  };
}

// 復元境界の内部状態は、proto の必須フィールドが未指定になる場合がある。
// Run.goalId はゴール非所属の run で、PendingDecision.inquiryId は問い合わせ以外で未指定に
// なるため、表示用の変換は proto の必須形ではなく未指定を許す入力型で受ける。
export type RunInput = Omit<Run, "goalId"> & { goalId: string | undefined };
export type PendingDecisionInput = Omit<PendingDecision, "inquiryId"> & {
  inquiryId: string | undefined;
};

export type PendingDecisionView = PendingDecision & {
  waitMinutes: number;
  stale: boolean;
};

type RunView = {
  run: RunInput;
  server: Server | undefined;
  connected: boolean;
  state: "live" | "warn" | "idle";
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
  runLimit: RunLimit;
  runningRuns: number;
  inProgressGoals: number;
  pendingCount: number;
  waitingTasks: number;
};

// 概要のプロジェクト行。プロジェクト自身の同時実行上限と、そのプロジェクトで
// 実行中の run の件数を対比できる形にする。
export type OverviewProjectView = Omit<Project, "concurrency"> & {
  concurrency: RunLimit;
  runningRuns: RunCount;
};

export type OverviewView = {
  // 最後に実際に読めた時刻。1件も読めていない場合は実在しない時刻を作らず undefined にする。
  readAt?: Date;
  servers: OverviewServerView[];
  projects: OverviewProjectView[];
  runs: RunView[];
  thresholds: ValidatedAnomalyThresholds;
  runningRuns: number;
  runLimit: RunLimit;
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

// 概要と判断待ちの表示は、SSE の JSON 復元結果・RSC の proto 初期状態・proto の応答の
// いずれも同じ関数へ渡せるようにする。未指定を許す runs / pendingDecisions だけを入力型で表す。
export type OverviewStateInput = Omit<DashboardState, "runs" | "pendingDecisions"> & {
  runs: readonly RunInput[];
  pendingDecisions: readonly PendingDecisionInput[];
};

export type DecisionsStateInput = {
  thresholds: AnomalyThresholds | undefined;
  pendingDecisions: readonly PendingDecisionInput[];
  servers: readonly Server[];
};

const minutesSince = (from: Date, now: Date): number => Math.max(0, (now.getTime() - from.getTime()) / 60_000);

const staleThreshold = (options: SortOptions): number =>
  options.staleThresholdMinutes ?? DEFAULT_ANOMALY_THRESHOLDS.pendingDecisionStaleMinutes;
export function sortPendingDecisions(
  decisions: readonly PendingDecisionInput[],
  now: Date,
  options: SortOptions = {},
): PendingDecisionView[] {
  const threshold = staleThreshold(options);
  return decisions
    .map((decision) => {
      const waitMinutes = decision.createdAt ? minutesSince(decision.createdAt, now) : 0;
      // 未指定の inquiryId は、proto の文字列境界で空文字へ射影する。識別子生成・
      // 真偽判定・表示は従来どおり空文字を受け取る。
      return { ...decision, inquiryId: decision.inquiryId ?? "", waitMinutes, stale: waitMinutes >= threshold };
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
  run: RunInput,
  serverById: ReadonlyMap<string, Server>,
  thresholds: AnomalyThresholds,
  now: Date,
  goalsById?: ReadonlyMap<string, Goal>,
  projectByKey?: ReadonlyMap<string, Project>,
): RunView {
  const server = serverById.get(run.serverId);
  const lastStep = run.steps.at(-1);
  const goalName = run.goalId !== undefined ? goalsById?.get(run.goalId)?.name : undefined;
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
  // run の表示は、プロジェクトの読み取り状態ではなく run 自身のログの鮮度で決める。
  // 読み取りが遅れている（DELAYED）だけでは run の表示を変えず、読み取り時刻を注記に
  // 使わない。「遅れているのに最後に読めたのは 0 秒前」のような自己矛盾が起きるため、
  // 遅延の注記は最後のログの時刻で書く（issue #194）。
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

// サーバーの状態は接続の有無と run の件数だけで決め、概要のサーバーカードと
// サイドバーで同じ関数と同じ文言を使う。別々の基準で決めると、同じサーバーが
// 「動作中」と「待機中」のように食い違って見える（issue #188）。run の件数は
// 検証済みの専用型 RunCount で受け取り、生の数値を流通させない。
export const serverStateFrom = (
  connected: boolean,
  runCount: RunCount,
): OverviewServerView["state"] => {
  if (!connected) {
    return "down";
  }
  return runCount > 0 ? "live" : "idle";
};

const serverState = (server: Server, runs: readonly RunInput[]): OverviewServerView["state"] =>
  serverStateFrom(server.connected, toRunCount(runs.length));

// idle は「接続できているが run が無い」状態であり、接続そのものができていない
// down（応答なし）と区別できる文言にする。文言は「接続中」と短くし、run が無いことは
// サイドバーの「run 0件」とカードの件数表示で別途分かるため文言には含めない。
// 長い文言は幅の狭いサイドバーの行にはみ出すおそれがある（issue #188）。
export const serverStateLabel = (state: OverviewServerView["state"]): string => {
  switch (state) {
    case "down":
      return "応答なし";
    case "live":
      return "動作中";
    case "idle":
      return "接続中";
  }
};

// 仕上げタスクが未着手・実行中の間は仕上げの途中なので、人のマージ待ちではなく進行中の
// ゴールとして数える。仕上げタスクが失敗・上限超過・PR 作成失敗で終わったゴールは、自動
// では進まず人の判断が要るため、人のマージ待ちとして数える（issue #231）。
// merge_task_last が未設定または完了の場合も人のマージ待ちである。
const MERGE_TASK_RUNNING: ReadonlySet<number> = new Set([TASK_STATUS.PENDING, TASK_STATUS.RUNNING]);

const isHumanMerge = (goal: Goal): boolean =>
  goal.status === GOAL_STATUS.AWAITING_MERGE &&
  !(goal.mergeTaskLast !== undefined && MERGE_TASK_RUNNING.has(goal.mergeTaskLast));

// 進行中のゴールは、goal-badge の区画分けで進行中の区画に入るゴールから、人の
// マージ待ちを除いたものとする。区画分けにより、タスクが登録されていない未着手の
// ゴールと、完了・中止のゴールは進行中に数えない（issue #242、issue #243）。
const isInProgressGoal = (goal: Goal): boolean =>
  goalSection(goal) === "inProgress" && !isHumanMerge(goal);

const waitingTaskCount = (goals: readonly Goal[]): number =>
  goals.reduce((total, goal) => total + (goal.taskCounts?.pending ?? 0), 0);

// 失敗系の合計の表示名。failedTaskCount が数える「失敗」は、失敗・上限超過・PR 失敗の
// 3状態の合計である。ゴールページの状態別の欄と同じ文言（taskStatusLabel の「失敗」、
// 「上限超過」、「PR 失敗」）で内訳を示し、概要の合計をゴールページの欄の合計と
// 照合できるようにする（issue #260）。
export const FAILED_TASK_KINDS_LABEL = "失敗・上限超過・PR 失敗";

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

export function buildOverviewView(state: OverviewStateInput, now: Date): OverviewView {
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
      runLimit: toRunLimit(
        state.projects
          .filter((project) => project.serverId === server.serverId)
          .reduce((total, project) => total + project.concurrency, 0),
      ),
      runningRuns: serverRuns.filter((run) => run.status === RUN_STATUS.RUNNING).length,
      inProgressGoals: serverGoals.filter(isInProgressGoal).length,
      pendingCount: pendingDecisions.filter((decision) => decision.serverId === server.serverId).length,
      waitingTasks: waitingTaskCount(serverGoals),
    };
  });
  const runningRuns = state.runs.filter((run) => run.status === RUN_STATUS.RUNNING).length;
  const silentRunCount = runs.filter((run) => run.state === "warn").length;
  const failedTasks = failedTaskCount(state.goals);
  const abortedGoals = state.goals.filter((goal) => goalSection(goal) === "aborted").length;
  const pendingCounts = summarizePendingDecisions(pendingDecisions);
  const readAt = state.readAt;
  const partialReadAt = earliestReadAt(
    [...unresponsiveServers, ...unresponsiveProjects].map((item) => item.lastReadAt),
  );
  return {
    readAt,
    servers,
    projects: state.projects.map((project) => ({
      ...project,
      concurrency: toRunLimit(project.concurrency),
      runningRuns: toRunCount(
        state.runs.filter(
          (run) =>
            projectKey(run.serverId, run.project) === projectKey(project.serverId, project.name) &&
            run.status === RUN_STATUS.RUNNING,
        ).length,
      ),
    })),
    runs,
    thresholds,
    runningRuns,
    runLimit: toRunLimit(state.projects.reduce((total, project) => total + project.concurrency, 0)),
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

export const taskStatusLabel = (status: TaskStatus): string => {
  switch (status) {
    case TASK_STATUS.PENDING:
      return "未着手";
    case TASK_STATUS.RUNNING:
      return "実行中";
    case TASK_STATUS.COMPLETED:
      return "完了";
    case TASK_STATUS.FAILED:
      return "失敗";
    case TASK_STATUS.EXCEEDED:
      return "上限超過";
    case TASK_STATUS.PR_FAILED:
      return "PR 失敗";
    default:
      return "不明";
  }
};

// 仕上げタスクの末尾状態だけで人のマージ待ちかどうかを判定する。通常タスクの pending/running は
// 判定に影響しない。awaiting_merge で、末尾が未着手・実行中でなければ人のマージ待ちになる。
// 失敗・上限超過・PR 作成失敗は、仕上げが止まって人の判断が要る状態として含める（issue #231）。
export function isHumanMergeWaiting(goal: Goal): boolean {
  if (goal.status !== GOAL_STATUS.AWAITING_MERGE) {
    return false;
  }
  return goal.mergeTaskLast !== TASK_STATUS.PENDING && goal.mergeTaskLast !== TASK_STATUS.RUNNING;
}

// 仕上げタスクの末尾状態が失敗系のときに、仕上げが止まった理由を示す文面を返す。
// ゴール一覧のカードと判断待ちのカードで同じ文面を使うため、末尾状態だけから決める。
const mergeTaskFailureNoteForLast = (mergeTaskLast: TaskStatus | undefined): string | undefined => {
  switch (mergeTaskLast) {
    case TASK_STATUS.FAILED:
      return "仕上げタスクが失敗しました。";
    case TASK_STATUS.EXCEEDED:
      return "仕上げタスクが上限超過で止まりました。";
    case TASK_STATUS.PR_FAILED:
      return "仕上げタスクのプルリクエスト作成が失敗しました。";
    default:
      return undefined;
  }
};

// 仕上げタスクが失敗系の状態で終わったことを示す文面（issue #231）。人のマージ待ちとして
// 出すときに、仕上げが止まった理由が分かるようにする。awaiting_merge でないゴールと、
// 失敗系でない末尾状態では undefined を返す。
export function mergeTaskFailureNote(goal: Goal): string | undefined {
  if (goal.status !== GOAL_STATUS.AWAITING_MERGE) {
    return undefined;
  }
  return mergeTaskFailureNoteForLast(goal.mergeTaskLast);
}

// 判断待ちのカードに出す、仕上げタスクの失敗を示す文面（issue #231）。マージ待ちの項目で、
// 仕上げタスクが失敗系の状態で終わったときだけ返す。それ以外の種別と末尾状態では
// undefined を返す。
export function pendingDecisionMergeFailureNote(decision: PendingDecision): string | undefined {
  if (decision.type !== PENDING_DECISION_TYPE.AWAITING_MERGE) {
    return undefined;
  }
  return mergeTaskFailureNoteForLast(decision.mergeTaskLast);
}

// 表示用の変換（C4/C7/C9）の契約。
//
// 待ち時間順・滞留・run の状態は、画面が使う観測可能な値としてここで固定する。
// 閾値は proto の AnomalyThresholds から取り、240/10 を実装側へ直書きさせない。
import test from "node:test";
import assert from "node:assert/strict";
import {
  ConnectionState,
  ExecutionStatus,
  GoalStatus,
  InquiryKind,
  PendingDecisionType,
  TaskStatus,
  type PendingDecision,
} from "../../src/proto/taktdash.ts";
import {
  createDashboardState,
  createDashboardStateWithProjectConnection,
  createHealthyDashboardState,
  createNestedStepDashboardState,
  createGoal,
  NOW,
  SILENT_MINUTES,
} from "../support/fixtures.ts";
import {
  buildOverviewView,
  DEFAULT_ANOMALY_THRESHOLDS,
  projectKey,
  isHumanMergeWaiting,
  resolveThresholds,
  sortPendingDecisions,
  summarizePendingDecisions,
  toRunView,
} from "../../src/state/dashboard.ts";
import { DomainPrimitiveError } from "../../src/state/domain-primitives.ts";
import { decisionKey } from "../../src/state/decision-actions.ts";

test("判断待ちは待ち時間の長い順に並ぶ", () => {
  const sorted = sortPendingDecisions(createDashboardState().pendingDecisions, NOW);
  assert.deepEqual(
    sorted.map((item: PendingDecision) => item.type),
    [PendingDecisionType.PENDING_DECISION_TYPE_AWAITING_MERGE, PendingDecisionType.PENDING_DECISION_TYPE_INQUIRY],
  );
  assert.equal(sorted[0]?.waitMinutes, 300);
});

test("待ち時間が閾値以上なら滞留として扱い、閾値未満は滞留にしない", () => {
  const thresholds = { pendingDecisionStaleMinutes: 240, runLogSilentMinutes: 10 };
  const state = createDashboardState();
  const atThreshold = sortPendingDecisions(
    [{ ...state.pendingDecisions[0]!, createdAt: new Date(NOW.getTime() - 240 * 60_000) }],
    NOW,
  );
  const justUnder = sortPendingDecisions(
    [{ ...state.pendingDecisions[0]!, createdAt: new Date(NOW.getTime() - 239 * 60_000) }],
    NOW,
  );
  assert.equal(atThreshold[0]?.stale, true);
  assert.equal(justUnder[0]?.stale, false);
  assert.equal(thresholds.pendingDecisionStaleMinutes, 240);
});

test("判断待ちの件数を問い合わせとマージ待ちに分けて数える", () => {
  const counts = summarizePendingDecisions(sortPendingDecisions(createDashboardState().pendingDecisions, NOW));
  assert.equal(counts.total, 2);
  assert.equal(counts.inquiries, 1);
  assert.equal(counts.awaitingMerge, 1);
});

test("承認待ちを独立の区分として数え、内訳の和を total に一致させる", () => {
  const state = createDashboardState();
  const goalRegistration: PendingDecision = {
    type: PendingDecisionType.PENDING_DECISION_TYPE_GOAL_REGISTRATION,
    serverId: "mini",
    project: "esa",
    inquiryKind: InquiryKind.INQUIRY_KIND_UNSPECIFIED,
    createdAt: new Date(NOW.getTime() - 60 * 60_000),
    body: "新しいゴール",
    options: [],
    pullRequest: undefined,
    stale: false,
    inquiryId: "",
    revision: 3,
  };
  const counts = summarizePendingDecisions(
    sortPendingDecisions([...state.pendingDecisions, goalRegistration], NOW),
  );
  assert.equal(counts.total, 3);
  assert.equal(counts.inquiries, 1);
  assert.equal(counts.awaitingMerge, 1);
  assert.equal(counts.goalRegistration, 1);
  assert.equal(counts.inquiries + counts.awaitingMerge + counts.goalRegistration, counts.total);
});

test("承認待ちが無いときは既存2種の数え方が変わらない", () => {
  const counts = summarizePendingDecisions(sortPendingDecisions(createDashboardState().pendingDecisions, NOW));
  assert.equal(counts.goalRegistration, 0);
  assert.equal(counts.inquiries + counts.awaitingMerge + counts.goalRegistration, counts.total);
});

test("run の状態はサーバー応答とログ無音と実行状態から決まる", () => {
  const state = createDashboardState();
  const serverById = new Map(state.servers.map((server) => [server.serverId, server]));
  const projectByKey = new Map(
    state.projects.map((project) => [projectKey(project.serverId, project.name), project]),
  );
  const thresholds = state.thresholds ?? DEFAULT_ANOMALY_THRESHOLDS;

  const live = toRunView(state.runs[0]!, serverById, thresholds, NOW, undefined, projectByKey);
  assert.equal(live.connected, true);
  assert.equal(live.label, "実行中");

  const disconnected = toRunView(
    { ...state.runs[0]!, serverId: "down" },
    serverById,
    thresholds,
    NOW,
    undefined,
    projectByKey,
  );
  assert.equal(disconnected.connected, false);
  assert.match(disconnected.label, /時点/);
});

test("run の状態はその run のプロジェクトの接続状態を使う", () => {
  const state = createDashboardState();
  const serverById = new Map(state.servers.map((server) => [server.serverId, server]));
  const projectByKey = new Map(
    state.projects.map((project) => [projectKey(project.serverId, project.name), project]),
  );
  const thresholds = state.thresholds ?? DEFAULT_ANOMALY_THRESHOLDS;
  const esaKey = projectKey("mini", "esa");
  projectByKey.set(esaKey, {
    ...projectByKey.get(esaKey)!,
    connected: false,
    connectionState: ConnectionState.CONNECTION_STATE_UNRESPONSIVE,
    lastReadAt: new Date(NOW.getTime() - 21 * 60_000),
  });

  const failed = toRunView(
    { ...state.runs[0]!, runSlug: "run-esa", project: "esa" },
    serverById,
    thresholds,
    NOW,
    undefined,
    projectByKey,
  );
  assert.equal(failed.connected, false, "読み取りに失敗したプロジェクトの run は接続なしとして扱う");
  assert.match(failed.label, /実行中（.+時点）/);
  assert.ok(failed.note.length > 0, "接続がない理由を表示します");

  const healthy = toRunView(state.runs[0]!, serverById, thresholds, NOW, undefined, projectByKey);
  assert.equal(healthy.connected, true, "同じサーバーの健常なプロジェクトの run は実行中のままです");
  assert.equal(healthy.label, "実行中");
});

test("バックエンドが無音と判定した run は閾値直後でも警告になり、サーバー別の run 行にも出る", () => {
  const state = createDashboardState();
  const base = state.runs[0]!;
  // 閾値の10分を1秒だけ超えた時点。画面側の丸め直しが残っていると 10 > 10 で警告を落とす。
  const justOverThreshold = new Date(NOW.getTime() - (SILENT_MINUTES * 60_000 + 1_000));
  const silentState = {
    ...state,
    runs: [{ ...base, logSilent: true, lastLogAt: justOverThreshold }],
  };

  const overview = buildOverviewView(silentState, NOW);

  assert.equal(overview.runs[0]?.label, "ログが止まっています", "バックエンドの無音判定をそのまま表示します");
  assert.match(overview.runs[0]?.note ?? "", /ログが .*出ていません。/, "無音の説明を添えます");
  const mini = overview.servers.find((item) => item.server.serverId === "mini");
  assert.ok(mini, "run の所属サーバーが概要にあります");
  assert.equal(mini.runs[0]?.label, "ログが止まっています", "サーバー別の run 行にも同じ警告を出します");
});

test("logSilent が偽でも経過時間が閾値を超えた run は警告にする", () => {
  const state = createDashboardState();
  const base = state.runs[0]!;
  const staleLog = new Date(NOW.getTime() - (SILENT_MINUTES * 60_000 + 60_000));
  const overview = buildOverviewView(
    { ...state, runs: [{ ...base, logSilent: false, lastLogAt: staleLog }] },
    NOW,
  );

  assert.equal(
    overview.runs[0]?.label,
    "ログが止まっています",
    "バックエンドの logSilent が偽でも、経過時間が閾値を超えたら警告にします",
  );
  assert.equal(overview.runs[0]?.note, "ログが 11分 出ていません。", "無音の分数を注記に出す");
});

test("概要の集計は proto の閾値をそのまま表示用に運ぶ", () => {
  const overview = buildOverviewView(createDashboardState(), NOW);
  assert.equal(overview.thresholds.pendingDecisionStaleMinutes, 240);
  assert.equal(overview.thresholds.runLogSilentMinutes, 10);
  assert.equal(overview.runningRuns, 1);
  assert.equal(overview.pendingCounts.total, 2);
  assert.equal(overview.servers.length, 2);
});

test("概要の集計は閾値の変更に追随し、240/10 を決め打ちしない", () => {
  const state = createDashboardState();
  const changed = {
    ...state,
    thresholds: { pendingDecisionStaleMinutes: 60, runLogSilentMinutes: 5 },
  };
  const overview = buildOverviewView(changed, NOW);
  assert.equal(overview.thresholds.pendingDecisionStaleMinutes, 60);
  assert.equal(overview.thresholds.runLogSilentMinutes, 5);
});

test("閾値が無い応答では既定値を1か所で解決し、全画面が同じ値を使う", () => {
  assert.equal(resolveThresholds(undefined), DEFAULT_ANOMALY_THRESHOLDS);
  assert.equal(DEFAULT_ANOMALY_THRESHOLDS.pendingDecisionStaleMinutes, 240);
  assert.equal(DEFAULT_ANOMALY_THRESHOLDS.runLogSilentMinutes, 10);

  const withoutThresholds = { ...createDashboardState(), thresholds: undefined };
  const overview = buildOverviewView(withoutThresholds, NOW);
  assert.equal(overview.thresholds.pendingDecisionStaleMinutes, 240);
  assert.equal(overview.thresholds.runLogSilentMinutes, 10);
});

const overviewInputs = (state = createDashboardState()) => ({
  serverById: new Map(state.servers.map((server) => [server.serverId, server])),
  goalsById: new Map(state.goals.map((goal) => [goal.goalId, goal])),
  thresholds: state.thresholds ?? DEFAULT_ANOMALY_THRESHOLDS,
});

test("run 行の表示値はゴール名・ステップ名・モデル・位置・反復・経過秒数を proto から導く", () => {
  const state = createDashboardState();
  const { serverById, goalsById, thresholds } = overviewInputs(state);

  const view = toRunView(state.runs[0]!, serverById, thresholds, NOW, goalsById);

  assert.equal(view.goalName, "採用ページのレイアウトを直す", "ゴール ID からゴールの目的を引く");
  assert.equal(view.currentStepName, "implement", "今のステップ名は proto の currentStepName から出す");
  assert.equal(view.model, "glm-5.3", "最後のステップのモデルを出す");
  assert.equal(view.stepNumber, 2, "現在ステップの位置を出す");
  assert.equal(view.totalSteps, 5, "全ステップ数を出す");
  assert.equal(view.iteration, 2, "反復回数を出す");
  assert.equal(view.elapsedSeconds, 60, "最後のログから60秒経過している");
  assert.equal(view.state, "live", "実行中の run は live である");
});

test("同じ状態のまま時刻が1秒進むと最後のログからの経過秒数も1秒進む", () => {
  const state = createDashboardState();
  const { serverById, goalsById, thresholds } = overviewInputs(state);
  const oneSecondLater = new Date(NOW.getTime() + 1_000);

  const atNow = toRunView(state.runs[0]!, serverById, thresholds, NOW, goalsById);
  const atNext = toRunView(state.runs[0]!, serverById, thresholds, oneSecondLater, goalsById);

  assert.equal(atNow.elapsedSeconds, 60);
  assert.equal(atNext.elapsedSeconds, 61, "backend の再配信が無くても状態が同じなら時刻だけで進む");
  assert.equal(atNext.state, "live");
});

test("同じ状態のままログの無音が閾値を超えると run は警告になり注記を出す", () => {
  const state = createDashboardState();
  const { serverById, goalsById, thresholds } = overviewInputs(state);
  const tenMinutesLater = new Date(NOW.getTime() + 10 * 60_000);

  const view = toRunView(state.runs[0]!, serverById, thresholds, tenMinutesLater, goalsById);

  assert.equal(view.state, "warn", "backend の logSilent フラグに依存せず時間の経過で警告になる");
  assert.equal(view.note, "ログが 11分 出ていません。", "無音の分数を注記に出す");
  assert.equal(view.elapsedSeconds, 660);
});

test("ステップ位置が分からない run は位置と総数を捏造しない", () => {
  const state = createDashboardState();
  const { serverById, goalsById, thresholds } = overviewInputs(state);
  const run = { ...state.runs[0]!, currentStepNumber: undefined, totalSteps: undefined };

  const view = toRunView(run, serverById, thresholds, NOW, goalsById);

  assert.equal(view.stepNumber, undefined, "位置が無ければ undefined のままにする");
  assert.equal(view.totalSteps, undefined, "総数が無ければ undefined のままにする");
  assert.equal(view.iteration, 2, "反復回数は位置とは独立して保持する");
});

test("入れ子 workflow の内側ログがあってもステップ名は位置と同じ currentStep 由来になる", () => {
  const state = createNestedStepDashboardState();
  const { serverById, goalsById, thresholds } = overviewInputs(state);

  const view = toRunView(state.runs[0]!, serverById, thresholds, NOW, goalsById);

  assert.equal(view.currentStepName, "develop", "名前は steps 末尾ではなく currentStepName から出す");
  assert.notEqual(view.currentStepName, "write_tests", "入れ子 workflow の内側の step 名を出さない");
  assert.equal(view.stepNumber, 1, "位置は同じ currentStep 由来の 1 つ目");
  assert.equal(view.totalSteps, 1, "総数も同じ workflow の step 集合から出す");
  assert.equal(view.iteration, 3, "反復回数は独立して保持する");
});

test("遅延中のプロジェクトの run は接続中のまま遅延の状態と最後に読めた時刻の注記を出す", () => {
  const state = createDashboardStateWithProjectConnection(
    "corporate-site",
    ConnectionState.CONNECTION_STATE_DELAYED,
    new Date(NOW.getTime() - 300 * 1_000),
  );

  const view = buildOverviewView(state, NOW);
  const run = view.runs[0]!;

  assert.equal(run.connected, true, "遅延中は接続中として扱う");
  assert.equal(run.state, "delayed", "遅延中は idle にしない");
  assert.equal(run.note, "読み取りが遅れています（最後に読めたのは 300 秒前）");
});

test("遅延の注記の秒数は時刻が1秒進むと1秒増える", () => {
  const state = createDashboardStateWithProjectConnection(
    "corporate-site",
    ConnectionState.CONNECTION_STATE_DELAYED,
    new Date(NOW.getTime() - 300 * 1_000),
  );

  const oneSecondLater = buildOverviewView(state, new Date(NOW.getTime() + 1_000));

  assert.equal(oneSecondLater.runs[0]!.note, "読み取りが遅れています（最後に読めたのは 301 秒前）");
});

test("応答なしのプロジェクトの run は従来どおり最後に読めた時点の idle 表示になる", () => {
  const state = createDashboardStateWithProjectConnection(
    "corporate-site",
    ConnectionState.CONNECTION_STATE_UNRESPONSIVE,
    new Date(NOW.getTime() - 21 * 60_000),
  );

  const view = buildOverviewView(state, NOW);
  const run = view.runs[0]!;

  assert.equal(run.state, "idle", "応答なしは idle のままにする");
  assert.doesNotMatch(run.note ?? "", /読み取りが遅れています/);
});

test("概要は実行上限・24時間完了・失敗・中止・無音を導出する", () => {
  const view = buildOverviewView(createDashboardState(), NOW);

  assert.equal(view.runLimit, 8, "全サーバーの concurrency の合計を実行上限にする");
  assert.equal(view.completedTasks24h, 4, "全プロジェクトの24時間完了タスクを合計する");
  assert.equal(view.failedTaskCount, 1, "失敗・上限超過・PR作成失敗を合わせて数える");
  assert.equal(view.abortedGoalCount, 0, "中止したゴールは無い");
  assert.equal(view.silentRunCount, 0, "無音の run は無い");
  assert.equal(view.hasAnomalies, true, "判断待ちなどがあるので異常ありとする");
});

test("異常が無い状態は hasAnomalies を false にし、失敗・中止・無音を数えない", () => {
  const view = buildOverviewView(createHealthyDashboardState(), NOW);

  assert.equal(view.hasAnomalies, false, "異常が無ければ false にする");
  assert.equal(view.silentRunCount, 0);
  assert.equal(view.failedTaskCount, 0);
  assert.equal(view.abortedGoalCount, 0);
});

test("中止したゴールは異常として数える", () => {
  const base = createHealthyDashboardState();
  const state = {
    ...base,
    goals: base.goals.map((goal, index) =>
      index === 0 ? { ...goal, executionStatus: ExecutionStatus.EXECUTION_STATUS_ABORTED } : goal,
    ),
  };
  const view = buildOverviewView(state, NOW);

  assert.equal(view.abortedGoalCount, 1);
  assert.equal(view.hasAnomalies, true);
});

test("ログが止まった run は無音として数え、異常ありにする", () => {
  const base = createHealthyDashboardState();
  const justOverThreshold = new Date(NOW.getTime() - (SILENT_MINUTES * 60_000 + 1_000));
  const state = {
    ...base,
    runs: base.runs.map((run) => ({ ...run, logSilent: true, lastLogAt: justOverThreshold })),
  };
  const view = buildOverviewView(state, NOW);

  assert.equal(view.silentRunCount, 1, "run 行と同じ警告判定で無音を数える");
  assert.equal(view.hasAnomalies, true);
});

test("プロジェクトの応答なしだけでも異常として扱い、最後に読めた時刻を最古にする", () => {
  const base = createHealthyDashboardState();
  const failedAt = new Date(NOW.getTime() - 21 * 60_000);
  const state = {
    ...base,
    projects: base.projects.map((project) =>
      project.name === "esa"
        ? {
            ...project,
            connected: false,
            connectionState: ConnectionState.CONNECTION_STATE_UNRESPONSIVE,
            lastReadAt: failedAt,
          }
        : project,
    ),
  };
  const view = buildOverviewView(state, NOW);

  assert.equal(view.hasAnomalies, true, "プロジェクトの応答なしも異常として扱う");
  assert.equal(view.partialReadAt?.getTime(), failedAt.getTime(), "応答なしの最後に読めた時刻を使う");
});

test("runLimit は concurrency が0でも破綻しない", () => {
  const base = createHealthyDashboardState();
  const state = {
    ...base,
    servers: base.servers.map((server) => ({ ...server, concurrency: 0 })),
  };
  const view = buildOverviewView(state, NOW);

  assert.equal(view.runLimit, 0);
});

test("サーバーごとの下端値は run・進行中ゴール・判断待ち・待機中タスクを数える", () => {
  const view = buildOverviewView(createDashboardState(), NOW);
  const mini = view.servers.find((item) => item.server.serverId === "mini");
  const down = view.servers.find((item) => item.server.serverId === "down");
  assert.ok(mini, "mini のカードがある");
  assert.ok(down, "down のカードがある");

  assert.equal(mini.runLimit, 4, "サーバーの concurrency を上限にする");
  assert.equal(mini.runningRuns, 1, "実行中の run を数える");
  assert.equal(mini.inProgressGoals, 1, "人のマージ待ちを除いた進行中のゴールを数える");
  assert.equal(mini.pendingCount, 2, "そのサーバーの判断待ちを数える");
  assert.equal(mini.waitingTasks, 1, "そのサーバーの待機中のタスクを合計する");

  assert.equal(down.runningRuns, 0);
  assert.equal(down.pendingCount, 0);
});

test("応答なしの読み取り時刻が1件も無いときは partialReadAt を捏造しない", () => {
  const base = createDashboardState();
  const state = {
    ...base,
    servers: base.servers.map((server) =>
      server.serverId === "down" ? { ...server, lastReadAt: undefined } : server,
    ),
  };
  const view = buildOverviewView(state, NOW);

  assert.equal(view.unresponsiveServers.length, 1, "応答なしのサーバーは異常として残す");
  assert.equal(view.partialReadAt, undefined, "読めた時刻が無ければ now で補わない");
});

test("未読のプロジェクトは応答なしに含めず、異常として扱わない", () => {
  const base = createHealthyDashboardState();
  const state = {
    ...base,
    projects: base.projects.map((project) =>
      project.name === "esa"
        ? {
            ...project,
            connected: false,
            connectionState: ConnectionState.CONNECTION_STATE_UNSPECIFIED,
            lastReadAt: undefined,
          }
        : project,
    ),
  };
  const view = buildOverviewView(state, NOW);

  assert.equal(view.unresponsiveProjects.length, 0, "未読（UNSPECIFIED）は応答なしに含めない");
  assert.equal(view.hasAnomalies, false, "未読だけでは異常にしない");
  assert.equal(view.partialReadAt, undefined, "未読を最後に読めた時刻に使わない");
});

test("進行中ゴールは完了・中止・人のマージ待ちを除き、仕上げタスクが進行中のマージ待ちを含める", () => {
  const base = createHealthyDashboardState();
  const goalBase = base.goals[0]!;
  const goal = (overrides: Partial<typeof goalBase>) => ({ ...goalBase, ...overrides });
  const state = {
    ...base,
    goals: [
      goal({
        goalId: "g-human-merge",
        status: GoalStatus.GOAL_STATUS_AWAITING_MERGE,
        mergeTaskLast: undefined,
      }),
      goal({
        goalId: "g-paused",
        status: GoalStatus.GOAL_STATUS_CREATED,
        executionStatus: ExecutionStatus.EXECUTION_STATUS_PAUSED,
      }),
      goal({ goalId: "g-completed", status: GoalStatus.GOAL_STATUS_COMPLETED }),
      goal({
        goalId: "g-aborted",
        status: GoalStatus.GOAL_STATUS_CREATED,
        executionStatus: ExecutionStatus.EXECUTION_STATUS_ABORTED,
      }),
      goal({
        goalId: "g-merge-in-progress",
        status: GoalStatus.GOAL_STATUS_AWAITING_MERGE,
        mergeTaskLast: TaskStatus.TASK_STATUS_PENDING,
      }),
    ],
  };
  const view = buildOverviewView(state, NOW);
  const mini = view.servers.find((item) => item.server.serverId === "mini");

  assert.ok(mini, "mini のカードがある");
  assert.equal(mini.inProgressGoals, 2, "一時停止中と仕上げタスク進行中のマージ待ちだけを数える");
});

test("失敗タスクは失敗・上限超過・PR作成失敗の3分類を合計する", () => {
  const base = createHealthyDashboardState();
  const state = {
    ...base,
    goals: base.goals.map((goal, index) =>
      index === 0
        ? {
            ...goal,
            taskCounts: { pending: 0, running: 0, completed: 6, failed: 1, exceeded: 2, prFailed: 3 },
          }
        : goal,
    ),
  };
  const view = buildOverviewView(state, NOW);

  assert.equal(view.failedTaskCount, 6, "failed 1 + exceeded 2 + prFailed 3 を合計する");
});
test("人のマージ待ちは仕上げタスクの末尾状態だけで判定する", () => {
  const awaiting = (mergeTaskLast?: TaskStatus): ReturnType<typeof createGoal> =>
    createGoal({ status: GoalStatus.GOAL_STATUS_AWAITING_MERGE, mergeTaskLast });

  assert.equal(
    isHumanMergeWaiting(awaiting(TaskStatus.TASK_STATUS_COMPLETED)),
    true,
    "仕上げタスクが完了していれば人のマージ待ちです",
  );
  assert.equal(
    isHumanMergeWaiting(awaiting(undefined)),
    true,
    "仕上げタスクの末尾状態が未設定でも人のマージ待ちです",
  );

  for (const status of [
    TaskStatus.TASK_STATUS_PENDING,
    TaskStatus.TASK_STATUS_RUNNING,
    TaskStatus.TASK_STATUS_FAILED,
    TaskStatus.TASK_STATUS_EXCEEDED,
    TaskStatus.TASK_STATUS_PR_FAILED,
  ]) {
    assert.equal(
      isHumanMergeWaiting(awaiting(status)),
      false,
      `仕上げタスクの末尾状態が ${TaskStatus[status]} なら人のマージ待ちではありません`,
    );
  }

  assert.equal(
    isHumanMergeWaiting(
      createGoal({ status: GoalStatus.GOAL_STATUS_CREATED, mergeTaskLast: TaskStatus.TASK_STATUS_COMPLETED }),
    ),
    false,
    "awaiting_merge 以外のゴールは人のマージ待ちではありません",
  );
});

test("通常タスクの未着手・実行中は人のマージ待ちの判定に影響しない", () => {
  const taskCounts = { pending: 0, running: 0, completed: 5, failed: 0, exceeded: 0, prFailed: 0 };
  const base = createGoal({
    status: GoalStatus.GOAL_STATUS_AWAITING_MERGE,
    mergeTaskLast: TaskStatus.TASK_STATUS_COMPLETED,
    taskCounts,
  });

  assert.equal(isHumanMergeWaiting(base), true, "通常タスクが動いていなくても人のマージ待ちです");

  const withRunning = { ...base, taskCounts: { ...taskCounts, pending: 0, running: 1 } };
  assert.equal(isHumanMergeWaiting(withRunning), true, "通常タスクが実行中でも人のマージ待ちです");

  const withPending = { ...base, taskCounts: { ...taskCounts, pending: 1, running: 0 } };
  assert.equal(isHumanMergeWaiting(withPending), true, "通常タスクが未着手でも人のマージ待ちです");
});

test("proto の閾値は範囲を検証してから表示用の値へ解決する", () => {
  const resolved = resolveThresholds({ pendingDecisionStaleMinutes: 60, runLogSilentMinutes: 5 });
  assert.equal(resolved.pendingDecisionStaleMinutes, 60);
  assert.equal(resolved.runLogSilentMinutes, 5);

  assert.equal(
    resolveThresholds({ pendingDecisionStaleMinutes: 0, runLogSilentMinutes: 0 }).pendingDecisionStaleMinutes,
    0,
  );

  const invalid = [
    { pendingDecisionStaleMinutes: -1, runLogSilentMinutes: 10 },
    { pendingDecisionStaleMinutes: 240, runLogSilentMinutes: -1 },
    { pendingDecisionStaleMinutes: 2_147_483_648, runLogSilentMinutes: 10 },
  ];
  for (const thresholds of invalid) {
    assert.throws(
      () => resolveThresholds(thresholds),
      (error) => error instanceof DomainPrimitiveError && error.stage === "size",
    );
  }
});

test("範囲外の閾値を持つ状態から概要を組み立てない", () => {
  const state = {
    ...createDashboardState(),
    thresholds: { pendingDecisionStaleMinutes: -1, runLogSilentMinutes: 10 },
  };
  assert.throws(
    () => buildOverviewView(state, NOW),
    (error) => error instanceof DomainPrimitiveError && error.stage === "size",
  );
});

test("未指定の inquiryId は proto 文字列の境界で空文字へ射影し、識別子を変えない", () => {
  const merge = { ...createDashboardState().pendingDecisions[1]!, inquiryId: undefined };
  const [view] = sortPendingDecisions([merge], NOW);

  assert.ok(view, "未指定の inquiryId を持つ判断待ちも並び替えられる");
  assert.equal(view.inquiryId, "");
  assert.equal(decisionKey(view), "merge:mini:corporate-site:goal-1");
});

test("ゴールに属さない run も概要とサーバー別の run 行に表示する", () => {
  const state = createDashboardState();
  const withoutGoal = { ...state, runs: [{ ...state.runs[0]!, goalId: undefined }] };
  const overview = buildOverviewView(withoutGoal, NOW);

  assert.equal(overview.runs[0]?.run.goalId, undefined);
  const mini = overview.servers.find((item) => item.server.serverId === "mini");
  assert.equal(mini?.runs[0]?.run.goalId, undefined);
  assert.equal(mini?.runs[0]?.run.runSlug, "run-aaa");
});

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
  mergeTaskFailureNote,
  notRespondingProjectText,
  notRespondingProjects,
  pendingDecisionMergeFailureNote,
  resolveThresholds,
  serverDisplayState,
  serverStateFrom,
  serverStateLabel,
  sortPendingDecisions,
  summarizePendingDecisions,
  toRunView,
} from "../../src/state/dashboard.ts";
import { DomainPrimitiveError, toRunCount } from "../../src/state/domain-primitives.ts";
import { decisionKey } from "../../src/state/decision-actions.ts";
import { goalSection } from "../../src/state/goal-badge.ts";

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
    // ゴール登録の承認待ちにはプルリクエストと問い合わせ ID が無い。proto の
    // 必須フィールドとして、フィクスチャと同じ形で明示する。
    pullRequest: undefined,
    inquiryId: "",
    stale: false,
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

test("バックエンドが無音と判定した run は閾値直後でも警告になる", () => {
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

test("遅延中のプロジェクトでも、ログが出続けている run は実行中のままにし、読み取り時刻の注記を付けない", () => {
  const state = createDashboardStateWithProjectConnection(
    "corporate-site",
    ConnectionState.CONNECTION_STATE_DELAYED,
    new Date(NOW.getTime() - 300 * 1_000),
  );

  const view = buildOverviewView(state, NOW);
  const run = view.runs[0]!;

  assert.equal(run.connected, true, "遅延中は接続中として扱う");
  assert.equal(run.state, "live", "ログが出続けている run は遅延表示にしない");
  assert.equal(run.label, "実行中");
  assert.equal(run.note, "", "読み取り時刻を根拠にした注記は付けない");
});

test("最後に読めた時刻が最新でも、ログが無音なら遅延の理由はログの無音として表示する", () => {
  const base = createDashboardStateWithProjectConnection(
    "corporate-site",
    ConnectionState.CONNECTION_STATE_DELAYED,
    NOW,
  );
  const state = {
    ...base,
    runs: base.runs.map((run) => ({
      ...run,
      logSilent: false,
      lastLogAt: new Date(NOW.getTime() - 93 * 60_000),
    })),
  };

  const view = buildOverviewView(state, NOW);
  const run = view.runs[0]!;

  assert.equal(run.state, "warn", "ログの無音で警告にする");
  assert.equal(run.label, "ログが止まっています");
  assert.equal(run.note, "ログが 93分 出ていません。", "注記は最後のログの時刻で書く");
  assert.doesNotMatch(run.note, /最後に読めたのは/, "読み取り時刻の注記と混ぜない");
});

test("読み取りもログも古いときは、注記は読み取り時刻ではなくログの時刻で書く", () => {
  const base = createDashboardStateWithProjectConnection(
    "corporate-site",
    ConnectionState.CONNECTION_STATE_DELAYED,
    new Date(NOW.getTime() - 300 * 1_000),
  );
  const state = {
    ...base,
    runs: base.runs.map((run) => ({
      ...run,
      logSilent: false,
      lastLogAt: new Date(NOW.getTime() - (SILENT_MINUTES * 60_000 + 60_000)),
    })),
  };

  const view = buildOverviewView(state, NOW);
  const run = view.runs[0]!;

  assert.equal(run.state, "warn", "読み取りが遅れていてもログの無音で警告にする");
  assert.equal(run.note, "ログが 11分 出ていません。", "注記はログ基準にする");
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

  assert.equal(view.runLimit, 8, "全プロジェクトの concurrency の合計を実行上限にする");
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
    projects: base.projects.map((project) => ({ ...project, concurrency: 0 })),
  };
  const view = buildOverviewView(state, NOW);

  assert.equal(view.runLimit, 0);
});

// main のこのテストは、サーバーカードの下端に出す view.servers[].runLimit / runningRuns / inProgressGoals /
// pendingCount / waitingTasks を確かめていた。概要はサーバーを1行で出す形（issue #120）に変わり、
// OverviewServerView はこれらのフィールドを持たない。上限を配下プロジェクトの concurrency の合計にする
// main の振る舞いと、run・判断待ちの件数は、概要が表示する全体の値で確かめる。
test("全体の上限はプロジェクトの concurrency の合計で、run と判断待ちの件数を数える", () => {
  const view = buildOverviewView(createDashboardState(), NOW);

  assert.equal(view.runLimit, 8, "全プロジェクトの concurrency の合計を上限にする");
  assert.equal(view.runningRuns, 1, "実行中の run を数える");
  assert.equal(view.pendingCounts.total, 2, "判断待ちを数える");
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

// main のこのテストは、サーバーカードの下端の view.servers[].inProgressGoals で進行中ゴールの件数を
// 確かめていた。概要はサーバーごとの件数を表示しなくなった（issue #120）ため、同じ7件のゴールについて、
// 進行中の分類（goal-badge の進行中の区画から、人のマージ待ちを除く）を公開関数で確かめる。
test("進行中ゴールは完了・中止・未着手・人のマージ待ちを除き、仕上げタスクが進行中のマージ待ちを含める", () => {
  const goalBase = createHealthyDashboardState().goals[0]!;
  const goal = (overrides: Partial<typeof goalBase>) => ({ ...goalBase, ...overrides });
  const goals = [
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
    goal({
      goalId: "g-merge-failed",
      status: GoalStatus.GOAL_STATUS_AWAITING_MERGE,
      mergeTaskLast: TaskStatus.TASK_STATUS_FAILED,
    }),
    goal({
      goalId: "g-untouched",
      status: GoalStatus.GOAL_STATUS_CREATED,
      executionStatus: ExecutionStatus.EXECUTION_STATUS_ACTIVE,
      taskCounts: { pending: 0, running: 0, completed: 0, failed: 0, exceeded: 0, prFailed: 0 },
    }),
  ];

  const inProgress = goals
    .filter((item) => goalSection(item) === "inProgress" && !isHumanMergeWaiting(item))
    .map((item) => item.goalId);
  assert.deepEqual(inProgress, ["g-paused", "g-merge-in-progress"], "一時停止中と仕上げタスク進行中のマージ待ちだけを数える");
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

  // 仕上げタスクが失敗系の状態で終わったゴールは、自動では進まず人の判断が要るため
  // 人のマージ待ちに含める（issue #231）。
  for (const status of [
    TaskStatus.TASK_STATUS_FAILED,
    TaskStatus.TASK_STATUS_EXCEEDED,
    TaskStatus.TASK_STATUS_PR_FAILED,
  ]) {
    assert.equal(
      isHumanMergeWaiting(awaiting(status)),
      true,
      `仕上げタスクの末尾状態が ${TaskStatus[status]} なら人のマージ待ちです`,
    );
  }

  for (const status of [TaskStatus.TASK_STATUS_PENDING, TaskStatus.TASK_STATUS_RUNNING]) {
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

test("仕上げタスクの失敗の文面は失敗系の末尾状態だけに出る", () => {
  const awaiting = (mergeTaskLast?: TaskStatus): ReturnType<typeof createGoal> =>
    createGoal({ status: GoalStatus.GOAL_STATUS_AWAITING_MERGE, mergeTaskLast });

  assert.equal(
    mergeTaskFailureNote(awaiting(TaskStatus.TASK_STATUS_FAILED)),
    "仕上げタスクが失敗しました。",
  );
  assert.equal(
    mergeTaskFailureNote(awaiting(TaskStatus.TASK_STATUS_EXCEEDED)),
    "仕上げタスクが上限超過で止まりました。",
  );
  assert.equal(
    mergeTaskFailureNote(awaiting(TaskStatus.TASK_STATUS_PR_FAILED)),
    "仕上げタスクのプルリクエスト作成が失敗しました。",
  );

  for (const status of [
    undefined,
    TaskStatus.TASK_STATUS_PENDING,
    TaskStatus.TASK_STATUS_RUNNING,
    TaskStatus.TASK_STATUS_COMPLETED,
  ]) {
    assert.equal(
      mergeTaskFailureNote(awaiting(status)),
      undefined,
      `仕上げタスクの末尾状態が ${status === undefined ? "未設定" : TaskStatus[status]} なら失敗の文面は出ない`,
    );
  }

  assert.equal(
    mergeTaskFailureNote(
      createGoal({ status: GoalStatus.GOAL_STATUS_CREATED, mergeTaskLast: TaskStatus.TASK_STATUS_FAILED }),
    ),
    undefined,
    "awaiting_merge 以外のゴールでは失敗の文面を出さない",
  );
});

test("判断待ちの失敗の文面はマージ待ちで失敗系の末尾状態のときだけ出る", () => {
  const merge = createDashboardState().pendingDecisions[1]!;
  const withLast = (mergeTaskLast?: TaskStatus): PendingDecision => ({ ...merge, mergeTaskLast });

  assert.equal(
    pendingDecisionMergeFailureNote(withLast(TaskStatus.TASK_STATUS_FAILED)),
    "仕上げタスクが失敗しました。",
  );
  assert.equal(
    pendingDecisionMergeFailureNote(withLast(TaskStatus.TASK_STATUS_EXCEEDED)),
    "仕上げタスクが上限超過で止まりました。",
  );
  assert.equal(
    pendingDecisionMergeFailureNote(withLast(TaskStatus.TASK_STATUS_PR_FAILED)),
    "仕上げタスクのプルリクエスト作成が失敗しました。",
  );

  for (const status of [
    undefined,
    TaskStatus.TASK_STATUS_PENDING,
    TaskStatus.TASK_STATUS_RUNNING,
    TaskStatus.TASK_STATUS_COMPLETED,
  ]) {
    assert.equal(
      pendingDecisionMergeFailureNote(withLast(status)),
      undefined,
      `仕上げタスクの末尾状態が ${status === undefined ? "未設定" : TaskStatus[status]} なら失敗の文面は出ない`,
    );
  }

  const inquiry = createDashboardState().pendingDecisions[0]!;
  assert.equal(
    pendingDecisionMergeFailureNote({ ...inquiry, mergeTaskLast: TaskStatus.TASK_STATUS_FAILED }),
    undefined,
    "マージ待ち以外の判断待ちでは失敗の文面を出さない",
  );
});

test("通常タスクの未着手・実行中は人のマージ待ちの判定に影響しない", () => {
  const base = createGoal({
    status: GoalStatus.GOAL_STATUS_AWAITING_MERGE,
    mergeTaskLast: TaskStatus.TASK_STATUS_COMPLETED,
    taskCounts: { pending: 0, running: 0, completed: 5, failed: 0, exceeded: 0, prFailed: 0 },
  });

  assert.equal(isHumanMergeWaiting(base), true, "通常タスクが動いていなくても人のマージ待ちです");

  // 件数の一部だけを変えたゴールも createGoal で作り、TaskStatusCounts の
  // 全フィールドがそろった形を保つ。
  const withRunning = createGoal({
    status: GoalStatus.GOAL_STATUS_AWAITING_MERGE,
    mergeTaskLast: TaskStatus.TASK_STATUS_COMPLETED,
    taskCounts: { pending: 0, running: 1, completed: 5, failed: 0, exceeded: 0, prFailed: 0 },
  });
  assert.equal(isHumanMergeWaiting(withRunning), true, "通常タスクが実行中でも人のマージ待ちです");

  const withPending = createGoal({
    status: GoalStatus.GOAL_STATUS_AWAITING_MERGE,
    mergeTaskLast: TaskStatus.TASK_STATUS_COMPLETED,
    taskCounts: { pending: 1, running: 0, completed: 5, failed: 0, exceeded: 0, prFailed: 0 },
  });
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

test("ゴールに属さない run も概要の run 一覧に表示する", () => {
  const state = createDashboardState();
  const withoutGoal = { ...state, runs: [{ ...state.runs[0]!, goalId: undefined }] };
  const overview = buildOverviewView(withoutGoal, NOW);

  assert.equal(overview.runs[0]?.run.goalId, undefined);
  assert.equal(overview.runs[0]?.run.runSlug, "run-aaa");
});

test("サーバーの状態は接続の有無と run の件数だけで決まる", () => {
  assert.equal(serverStateFrom(false, toRunCount(3)), "down", "未接続なら run があっても応答なしの側にする");
  assert.equal(serverStateFrom(false, toRunCount(0)), "down", "未接続なら down にする");
  assert.equal(serverStateFrom(true, toRunCount(1)), "live", "接続済みで run があれば live にする");
  assert.equal(serverStateFrom(true, toRunCount(0)), "idle", "接続済みで run が無ければ idle にする");
});

test("サーバーの状態の文言は接続と活動を区別する", () => {
  assert.equal(serverStateLabel("down"), "応答なし", "未接続の表示は従来どおり応答なしにする");
  assert.equal(serverStateLabel("live"), "動作中", "run がある場合は動作中にする");
  assert.equal(
    serverStateLabel("idle"),
    "接続中",
    "接続済みで run が無いことを、応答なしとも動作中とも違う文言で示す",
  );
});

test("接続済みで run が0件のサーバーは概要カードで「接続中」になる", () => {
  const state = createDashboardState();
  const overview = buildOverviewView({ ...state, runs: [] }, NOW);

  const mini = overview.servers.find((item) => item.server.serverId === "mini");
  assert.equal(mini?.state, "idle", "接続済みで run が無いサーバーは idle である");
  assert.equal(mini?.stateLabel, "接続中", "カードの文言が接続できていることを示す");

  const down = overview.servers.find((item) => item.server.serverId === "down");
  assert.equal(down?.state, "down", "未接続のサーバーは down のままである");
  assert.equal(down?.stateLabel, "応答なし", "応答なしの表示は従来どおりである");
});

test("接続済みで run があるサーバーは概要カードで「動作中」になる", () => {
  const overview = buildOverviewView(createDashboardState(), NOW);

  const mini = overview.servers.find((item) => item.server.serverId === "mini");
  assert.equal(mini?.state, "live", "run があるサーバーは live である");
  assert.equal(mini?.stateLabel, "動作中", "カードの文言は動作中である");
});

// main のこのテストは、全体の上限に加えて、サーバーカードの下端に出す view.servers[].runLimit（配下の
// プロジェクトの concurrency の合計。プロジェクトが無いサーバーは0）を確かめていた。概要はサーバーを1行で
// 出す形（issue #120）に変わり、OverviewServerView はサーバーごとの上限を持たない。サーバーごとの上限の
// 代わりに、別のサーバーのプロジェクトも全体の上限へ合計されることを確かめる。
test("runLimit はサーバーの concurrency ではなく、プロジェクトの concurrency の合計にする", () => {
  const base = createDashboardState();
  const state = {
    ...base,
    projects: base.projects.map((project) =>
      project.name === "corporate-site" ? { ...project, concurrency: 1 } : { ...project, concurrency: 2 },
    ),
  };
  const view = buildOverviewView(state, NOW);

  assert.equal(view.runLimit, 3, "全プロジェクトの concurrency の合計を実行上限にする");

  const template = state.projects[0]!;
  const withDownProject = buildOverviewView(
    { ...state, projects: [...state.projects, { ...template, name: "down-project", serverId: "down", concurrency: 4 }] },
    NOW,
  );
  assert.equal(withDownProject.runLimit, 7, "別のサーバーのプロジェクトの concurrency も合計する");
});

test("プロジェクトごとに実行中の run 件数とそのプロジェクトの上限を対比できる", () => {
  const view = buildOverviewView(createDashboardState(), NOW);
  const corporate = view.projects.find((project) => project.name === "corporate-site");
  assert.ok(corporate, "corporate-site のカードがある");
  assert.equal(corporate.concurrency, 4, "プロジェクト自身の concurrency を上限として持つ");
  assert.equal(corporate.runningRuns, 1, "そのプロジェクトの実行中の run を数える");
  const esa = view.projects.find((project) => project.name === "esa");
  assert.ok(esa, "esa のカードがある");
  assert.equal(esa.runningRuns, 0, "実行中の run が無いプロジェクトは0件にする");
});


test("概要の最終読み取り時刻は、1件も読めていない場合に現在時刻で補わない", () => {
  const state = { ...createDashboardState(), readAt: undefined };
  const later = new Date(NOW.getTime() + 30 * 60_000);
  const overview = buildOverviewView(state, later);
  assert.equal(overview.readAt, undefined, "読めた時刻が無い場合は時刻を作りません");
});

test("概要の最終読み取り時刻は、表示用の現在時刻ではなく最後に実際に読めた時刻を保持する", () => {
  const later = new Date(NOW.getTime() + 30 * 60_000);
  const overview = buildOverviewView(createDashboardState(), later);
  assert.equal(overview.readAt?.getTime(), NOW.getTime(), "state の readAt をそのまま使います");
});

test("応答していないプロジェクトは応答なしと未読の両方を含み、遅延と接続済みは含めない", () => {
  const state = createDashboardState();
  const readAt = new Date(NOW.getTime() - 5 * 60_000);
  const projects = [
    ...state.projects.map((project) =>
      project.name === "esa"
        ? {
            ...project,
            connected: false,
            connectionState: ConnectionState.CONNECTION_STATE_UNRESPONSIVE,
            lastReadAt: readAt,
          }
        : project,
    ),
    {
      ...state.projects[0]!,
      name: "unread",
      connectionState: ConnectionState.CONNECTION_STATE_UNSPECIFIED,
      lastReadAt: undefined,
    },
    {
      ...state.projects[0]!,
      name: "delayed",
      connectionState: ConnectionState.CONNECTION_STATE_DELAYED,
      lastReadAt: readAt,
    },
  ];

  assert.deepEqual(
    notRespondingProjects(projects).map((project) => project.name),
    ["esa", "unread"],
    "応答なしと未読だけを応答していないものとして数える",
  );
});

test("応答していないプロジェクトの文言は応答なしと未読で言い方を分ける", () => {
  const base = createDashboardState().projects[0]!;

  assert.equal(
    notRespondingProjectText({
      ...base,
      connectionState: ConnectionState.CONNECTION_STATE_UNRESPONSIVE,
    }),
    "corporate-site から応答がありません。",
    "応答なしの文言にする",
  );
  assert.equal(
    notRespondingProjectText({
      ...base,
      connectionState: ConnectionState.CONNECTION_STATE_UNSPECIFIED,
      lastReadAt: undefined,
    }),
    "corporate-site はまだ一度も読めていません。",
    "未読の文言にする",
  );
});

test("配下のプロジェクトが応答していないサーバーは表示状態が warn になる", () => {
  const state = createDashboardState();
  const mini = state.servers[0]!;
  const down = state.servers[1]!;
  const failed = state.projects.map((project) =>
    project.name === "esa"
      ? {
          ...project,
          connected: false,
          connectionState: ConnectionState.CONNECTION_STATE_UNRESPONSIVE,
        }
      : project,
  );

  assert.equal(
    serverDisplayState(mini, toRunCount(1), failed),
    "warn",
    "応答なしのプロジェクトがあれば run があっても warn にする",
  );
  assert.equal(
    serverDisplayState(mini, toRunCount(0), failed),
    "warn",
    "応答なしのプロジェクトがあれば run が無くても warn にする",
  );
  assert.equal(
    serverDisplayState(mini, toRunCount(1), state.projects),
    "live",
    "全プロジェクトが正常なら基本状態のままにする",
  );
  assert.equal(
    serverDisplayState(down, toRunCount(0), failed),
    "down",
    "サーバー自体の応答なしを優先する",
  );
  assert.equal(serverStateLabel("warn"), "一部応答なし", "warn の文言は一部応答なしにする");
});

test("まだ読めていないプロジェクトがあるサーバーも表示状態が warn になる", () => {
  const state = createDashboardState();
  const mini = state.servers[0]!;
  const unread = state.projects.map((project) =>
    project.name === "esa" ? { ...project, lastReadAt: undefined } : project,
  );

  assert.equal(serverDisplayState(mini, toRunCount(1), unread), "warn", "未読も正常とは言わない");
});

test("概要のサーバー表示は応答していないプロジェクトを抱えると warn になり、対象を保持する", () => {
  const base = createDashboardState();
  const state = {
    ...base,
    projects: base.projects.map((project) =>
      project.name === "esa"
        ? {
            ...project,
            connected: false,
            connectionState: ConnectionState.CONNECTION_STATE_UNRESPONSIVE,
          }
        : project,
    ),
  };
  const view = buildOverviewView(state, NOW);
  const mini = view.servers.find((item) => item.server.serverId === "mini");
  assert.ok(mini, "mini のカードがある");
  assert.equal(mini.state, "warn", "カードの状態が正常に見えないようにする");
  assert.equal(mini.stateLabel, "一部応答なし", "カードの文言も一部応答なしにする");
  assert.deepEqual(
    mini.notRespondingProjects.map((project) => project.name),
    ["esa"],
    "応答していないプロジェクトをカードが保持する",
  );

  const down = view.servers.find((item) => item.server.serverId === "down");
  assert.equal(down?.state, "down", "未接続のサーバーは応答なしのままにする");
  assert.deepEqual(down?.notRespondingProjects, [], "配下にプロジェクトが無いサーバーは空にする");
});

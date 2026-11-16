// 表示用の変換（C4/C7/C9）の契約。
//
// 待ち時間順・滞留・run の状態は、画面が使う観測可能な値としてここで固定する。
// 閾値は proto の AnomalyThresholds から取り、240/10 を実装側へ直書きさせない。
import test from "node:test";
import assert from "node:assert/strict";
import { PendingDecisionType, GoalStatus, InquiryKind, TaskStatus, type PendingDecision } from "../../src/proto/taktdash.ts";
import { createDashboardState, createGoal, NOW, SILENT_MINUTES } from "../support/fixtures.ts";
import {
  buildOverviewView,
  DEFAULT_ANOMALY_THRESHOLDS,
  isHumanMergeWaiting,
  resolveThresholds,
  sortPendingDecisions,
  summarizePendingDecisions,
  toRunView,
} from "../../src/state/dashboard.ts";

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
  const projectById = new Map(state.projects.map((project) => [project.name, project]));

  const live = toRunView(state.runs[0]!, serverById, projectById, NOW);
  assert.equal(live.connected, true);
  assert.equal(live.label, "実行中");

  const disconnected = toRunView(
    { ...state.runs[0]!, serverId: "down" },
    serverById,
    projectById,
    NOW,
  );
  assert.equal(disconnected.connected, false);
  assert.match(disconnected.label, /時点/);
});

test("run の状態はその run のプロジェクトの接続状態を使う", () => {
  const state = createDashboardState();
  const serverById = new Map(state.servers.map((server) => [server.serverId, server]));
  const projectById = new Map(state.projects.map((project) => [project.name, project]));
  projectById.set("esa", {
    ...projectById.get("esa")!,
    connected: false,
    lastReadAt: new Date(NOW.getTime() - 21 * 60_000),
  });

  const failed = toRunView(
    { ...state.runs[0]!, runSlug: "run-esa", project: "esa" },
    serverById,
    projectById,
    NOW,
  );
  assert.equal(failed.connected, false, "読み取りに失敗したプロジェクトの run は接続なしとして扱う");
  assert.match(failed.label, /実行中（.+時点）/);
  assert.ok(failed.note.length > 0, "接続がない理由を表示します");

  const healthy = toRunView(state.runs[0]!, serverById, projectById, NOW);
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

test("logSilent が偽の run は経過時間だけでは警告にしない", () => {
  const state = createDashboardState();
  const base = state.runs[0]!;
  const staleLog = new Date(NOW.getTime() - (SILENT_MINUTES * 60_000 + 60_000));
  const overview = buildOverviewView({ ...state, runs: [{ ...base, logSilent: false, lastLogAt: staleLog }] }, NOW);

  assert.equal(overview.runs[0]?.label, "実行中", "バックエンドが無音と判定していなければ警告にしません");
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
  const base = createGoal({
    status: GoalStatus.GOAL_STATUS_AWAITING_MERGE,
    mergeTaskLast: TaskStatus.TASK_STATUS_COMPLETED,
    taskCounts: { pending: 0, running: 0, completed: 5, failed: 0, exceeded: 0, prFailed: 0 },
  });

  assert.equal(isHumanMergeWaiting(base), true, "通常タスクが動いていなくても人のマージ待ちです");

  const withRunning = { ...base, taskCounts: { ...base.taskCounts, pending: 0, running: 1 } };
  assert.equal(isHumanMergeWaiting(withRunning), true, "通常タスクが実行中でも人のマージ待ちです");

  const withPending = { ...base, taskCounts: { ...base.taskCounts, pending: 1, running: 0 } };
  assert.equal(isHumanMergeWaiting(withPending), true, "通常タスクが未着手でも人のマージ待ちです");
});

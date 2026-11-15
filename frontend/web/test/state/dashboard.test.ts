// 表示用の変換（C4/C7/C9）の契約。
//
// 待ち時間順・滞留・run の状態は、画面が使う観測可能な値としてここで固定する。
// 閾値は proto の AnomalyThresholds から取り、240/10 を実装側へ直書きさせない。
import test from "node:test";
import assert from "node:assert/strict";
import { ConnectionState, PendingDecisionType, type PendingDecision } from "../../src/proto/taktdash.ts";
import {
  createDashboardState,
  createDashboardStateWithProjectConnection,
  createNestedStepDashboardState,
  NOW,
} from "../support/fixtures.ts";
import {
  buildOverviewView,
  DEFAULT_ANOMALY_THRESHOLDS,
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

test("run の状態はサーバー応答とログ無音と実行状態から決まる", () => {
  const state = createDashboardState();
  const serverById = new Map(state.servers.map((server) => [server.serverId, server]));

  const live = toRunView(state.runs[0]!, serverById, state.thresholds!, NOW);
  assert.equal(live.connected, true);
  assert.equal(live.label, "実行中");

  const disconnected = toRunView(
    { ...state.runs[0]!, serverId: "down" },
    serverById,
    state.thresholds!,
    NOW,
  );
  assert.equal(disconnected.connected, false);
  assert.match(disconnected.label, /時点/);
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

// ゴール一覧の契約（LIST-01 / LIST-02 / LIST-03）。
//
// 一覧は DashboardState.goals を表示用の形へ変換し、全サーバー・全プロジェクトの
// ゴールを既定で表示する。?project= 指定時はそのプロジェクトだけに絞り込む。
import test from "node:test";
import assert from "node:assert/strict";
import { createElement, type ComponentProps } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { ConnectionState, ExecutionStatus, GoalStatus, TaskStatus } from "../../src/proto/taktdash.ts";
import { createDashboardState, createGoal } from "../support/fixtures.ts";
import { dashboardStateFromProto } from "../../src/state/restore-state.ts";
import { GoalsContent } from "../../src/components/goals-content.tsx";
import { StateStreamProvider } from "../../src/components/state-provider.tsx";
import GoalsPage from "../../app/goals/page.tsx";

// GoalsContent は内部型の状態を受けるため、proto のフィクスチャを変換して渡す。
// このファイルは内容の契約を見るため、接続中（disconnected: false）で描画する。
// 切断時の通知の契約は live-disconnected.test.ts と live-provider-wiring.test.ts が担う。
const renderGoals = (state = createDashboardState(), project?: string): string =>
  renderToStaticMarkup(
    createElement(GoalsContent, { snapshotLoaded: true, state: dashboardStateFromProto(state), project, disconnected: false }),
  );

// ページは RSC で searchParams を受け取り、SSE の state を配る Provider の内側で一覧を描く。
// 描画時の配線（searchParams → LiveGoals → GoalsContent）を観測するため、ページ関数を
// 直接呼び出して得た要素を Provider の子として静的に描画する。
const renderGoalsPage = async (project?: string): Promise<string> => {
  const page = await GoalsPage({
    searchParams: Promise.resolve(project === undefined ? {} : { project }),
  });
  // children は Provider の props の必須項目のため、型付けした props ごと渡す。
  const props: ComponentProps<typeof StateStreamProvider> = { initialSnapshotLoaded: true,
    initialState: createDashboardState(),
    children: page,
  };
  return renderToStaticMarkup(createElement(StateStreamProvider, props));
};

// 一覧の行は 1件につき1つの li 要素になる。名前から行を切り出し、行ごとの表示を観測する。
const rowFor = (html: string, name: string): string => {
  const row = html
    .split("<li")
    .slice(1)
    .find((candidate) => candidate.includes(name));
  assert.ok(row !== undefined, `「${name}」の行がある`);
  return row;
};

test("ゴール一覧は全サーバー・全プロジェクトのゴールを既定で表示する", () => {
  const html = renderGoals();
  assert.match(html, /採用ページのレイアウトを直す/);
  assert.match(html, /v2\.4\.0 を公開する/);
});

test("ゴール一覧は ?project= の指定でそのプロジェクトのゴールだけに絞り込む", () => {
  const html = renderGoals(createDashboardState(), "corporate-site");
  assert.match(html, /採用ページのレイアウトを直す/);
  assert.doesNotMatch(html, /v2\.4\.0 を公開する/);
  assert.match(html, /corporate-site/);
});

test("ゴール一覧は0件のとき空であることが分かる表示になる", () => {
  const state = createDashboardState();
  const html = renderGoals({ ...state, goals: [] });
  assert.match(html, /ゴールはありません/);
});

test("ゴール一覧はまだ読めていないとき0件でも「ゴールはありません」と断定しない", () => {
  const base = createDashboardState();
  const html = renderGoals({
    ...base,
    readAt: undefined,
    servers: base.servers.map((server) => ({ ...server, connected: false, lastReadAt: undefined })),
    projects: base.projects.map((item) => ({
      ...item,
      connected: false,
      connectionState: ConnectionState.CONNECTION_STATE_UNSPECIFIED,
      lastReadAt: undefined,
    })),
    goals: [],
  });
  assert.doesNotMatch(html, /ゴールはありません/);
  assert.match(html, /まだ読めていないサーバー・プロジェクトがあります/);
  assert.match(html, /ゴールがあるかどうかは分かりません/);
});

test("ゴール一覧ページは ?project= を一覧へ渡してそのプロジェクトだけを表示する", async () => {
  const html = await renderGoalsPage("esa");
  assert.match(html, /v2\.4\.0 を公開する/);
  assert.doesNotMatch(html, /採用ページのレイアウトを直す/);
  assert.match(html, /プロジェクト「esa」のゴールを1件表示しています。/);
});

test("ゴール一覧ページは ?project= が無いとき全プロジェクトのゴールを表示する", async () => {
  const html = await renderGoalsPage();
  assert.match(html, /採用ページのレイアウトを直す/);
  assert.match(html, /v2\.4\.0 を公開する/);
  assert.match(html, /全サーバー・全プロジェクトのゴールを2件表示しています。/);
});

test("ゴール一覧の各行は状態の文面・状態別のタスク件数・マージ待ち・プルリクエストを表示する", () => {
  const humanMerge = createGoal({
    goalId: "goal-human",
    name: "人のマージ待ちのゴール",
    status: GoalStatus.GOAL_STATUS_AWAITING_MERGE,
    executionStatus: ExecutionStatus.EXECUTION_STATUS_ACTIVE,
    taskCounts: { pending: 0, running: 0, completed: 13, failed: 0, exceeded: 0, prFailed: 0 },
    pullRequest: { number: 418, url: "https://example.test/pr/418" },
  });
  const failedLast = createGoal({
    goalId: "goal-failed-last",
    name: "仕上げが失敗したゴール",
    status: GoalStatus.GOAL_STATUS_AWAITING_MERGE,
    taskCounts: { pending: 0, running: 0, completed: 7, failed: 1, exceeded: 0, prFailed: 0 },
    mergeTaskLast: TaskStatus.TASK_STATUS_FAILED,
    pullRequest: { number: 419, url: "https://example.test/pr/419" },
  });
  const running = createGoal({
    goalId: "goal-running",
    name: "実行中のゴール",
    status: GoalStatus.GOAL_STATUS_AWAITING_MERGE,
    taskCounts: { pending: 0, running: 1, completed: 2, failed: 0, exceeded: 0, prFailed: 0 },
    mergeTaskLast: TaskStatus.TASK_STATUS_RUNNING,
  });
  const exceededLast = createGoal({
    goalId: "goal-exceeded-last",
    name: "仕上げが上限超過したゴール",
    status: GoalStatus.GOAL_STATUS_AWAITING_MERGE,
    taskCounts: { pending: 0, running: 0, completed: 7, failed: 0, exceeded: 1, prFailed: 0 },
    mergeTaskLast: TaskStatus.TASK_STATUS_EXCEEDED,
    pullRequest: { number: 420, url: "https://example.test/pr/420" },
  });
  const prFailedLast = createGoal({
    goalId: "goal-pr-failed-last",
    name: "仕上げのPR作成が失敗したゴール",
    status: GoalStatus.GOAL_STATUS_AWAITING_MERGE,
    taskCounts: { pending: 0, running: 0, completed: 7, failed: 0, exceeded: 0, prFailed: 1 },
    mergeTaskLast: TaskStatus.TASK_STATUS_PR_FAILED,
  });
  const counts = createGoal({
    goalId: "goal-counts",
    name: "状態別の件数を持つゴール",
    status: GoalStatus.GOAL_STATUS_CREATED,
    taskCounts: { pending: 11, running: 12, completed: 13, failed: 14, exceeded: 15, prFailed: 16 },
  });
  const state = createDashboardState();
  const html = renderGoals({ ...state, goals: [humanMerge, failedLast, running, exceededLast, prFailedLast, counts] });

  const humanRow = rowFor(html, "人のマージ待ちのゴール");
  assert.match(humanRow, /人のマージ待ち/);
  assert.match(humanRow, /ゴールは人のマージ待ちです/);
  assert.doesNotMatch(humanRow, /稼働中/);
  assert.match(humanRow, /マージの承認待ちです/);
  assert.match(humanRow, /13/);
  assert.match(humanRow, /418/);
  assert.match(humanRow, /https:\/\/example\.test\/pr\/418/);

  // 仕上げタスクが失敗したゴールは、人のマージ待ちとして出し、失敗した旨も示す（issue #231）。
  const failedRow = rowFor(html, "仕上げが失敗したゴール");
  assert.match(failedRow, /マージの承認待ちです/);
  assert.doesNotMatch(failedRow, /マージの承認待ちではありません/);
  assert.match(failedRow, /仕上げタスクが失敗しました/);

  const exceededRow = rowFor(html, "仕上げが上限超過したゴール");
  assert.match(exceededRow, /マージの承認待ちです/);
  assert.match(exceededRow, /仕上げタスクが上限超過で止まりました/);

  const prFailedRow = rowFor(html, "仕上げのPR作成が失敗したゴール");
  assert.match(prFailedRow, /マージの承認待ちです/);
  assert.match(prFailedRow, /仕上げタスクのプルリクエスト作成が失敗しました/);

  const runningRow = rowFor(html, "実行中のゴール");
  assert.match(runningRow, /マージの承認待ちではありません/);
  assert.doesNotMatch(runningRow, /仕上げタスクが/);

  const countsRow = rowFor(html, "状態別の件数を持つゴール");
  for (const entry of [
    "未着手 11件",
    "実行中 12件",
    "完了 13件",
    "失敗 14件",
    "上限超過 15件",
    "PR 失敗 16件",
  ]) {
    assert.ok(countsRow.includes(entry), `状態別の件数が「${entry}」と表示される`);
  }
  // 件数の項目どうしは、区切りを持つコンテナの中に並ぶ（issue #232）。
  assert.match(countsRow, /class="kv task-counts"/);
});

test("ゴール一覧の各行はゴール ID をエスケープした詳細へのリンクを持つ", () => {
  const goal = createGoal({ goalId: "goal/1", name: "リンクを持つゴール" });
  const state = createDashboardState();
  const html = renderGoals({ ...state, goals: [goal] });
  assert.match(html, /href="\/goals\/goal%2F1"/);
  assert.doesNotMatch(html, /href="\/goals\/goal\/1"/);
});

test("ゴール一覧の各行は状態と実行状態が矛盾しない文面を表示する", () => {
  const completed = createGoal({
    goalId: "goal-completed",
    name: "完了したゴール",
    status: GoalStatus.GOAL_STATUS_COMPLETED,
    executionStatus: ExecutionStatus.EXECUTION_STATUS_ACTIVE,
    taskCounts: { pending: 0, running: 0, completed: 5, failed: 0, exceeded: 0, prFailed: 0 },
  });
  const completedRunning = createGoal({
    goalId: "goal-completed-running",
    name: "完了して実行中のタスクが残るゴール",
    status: GoalStatus.GOAL_STATUS_COMPLETED,
    executionStatus: ExecutionStatus.EXECUTION_STATUS_ACTIVE,
    taskCounts: { pending: 0, running: 2, completed: 5, failed: 0, exceeded: 0, prFailed: 0 },
  });
  const aborted = createGoal({
    goalId: "goal-aborted",
    name: "中止されたゴール",
    status: GoalStatus.GOAL_STATUS_CREATED,
    executionStatus: ExecutionStatus.EXECUTION_STATUS_ABORTED,
    taskCounts: { pending: 1, running: 0, completed: 2, failed: 0, exceeded: 0, prFailed: 0 },
  });
  const untouched = createGoal({
    goalId: "goal-untouched",
    name: "タスクが無いゴール",
    status: GoalStatus.GOAL_STATUS_CREATED,
    executionStatus: ExecutionStatus.EXECUTION_STATUS_ACTIVE,
    taskCounts: { pending: 0, running: 0, completed: 0, failed: 0, exceeded: 0, prFailed: 0 },
  });
  const running = createGoal({
    goalId: "goal-running-tasks",
    name: "実行中のタスクがあるゴール",
    status: GoalStatus.GOAL_STATUS_CREATED,
    executionStatus: ExecutionStatus.EXECUTION_STATUS_ACTIVE,
    taskCounts: { pending: 1, running: 3, completed: 0, failed: 0, exceeded: 0, prFailed: 0 },
  });
  const paused = createGoal({
    goalId: "goal-paused",
    name: "一時停止のゴール",
    status: GoalStatus.GOAL_STATUS_CREATED,
    executionStatus: ExecutionStatus.EXECUTION_STATUS_PAUSED,
    taskCounts: { pending: 2, running: 0, completed: 1, failed: 0, exceeded: 0, prFailed: 0 },
  });
  const state = createDashboardState();
  const html = renderGoals({ ...state, goals: [completed, completedRunning, aborted, untouched, running, paused] });

  const completedRow = rowFor(html, "完了したゴール");
  assert.match(completedRow, /ゴールは完了しています/);
  assert.doesNotMatch(completedRow, /稼働中/);
  assert.doesNotMatch(completedRow, /残っています/);

  const completedRunningRow = rowFor(html, "完了して実行中のタスクが残るゴール");
  assert.match(completedRunningRow, /ゴールは完了しています。実行中のタスクが 2 件残っています/);
  assert.doesNotMatch(completedRunningRow, /稼働中/);

  const abortedRow = rowFor(html, "中止されたゴール");
  assert.match(abortedRow, /ゴールは中止されました/);
  assert.doesNotMatch(abortedRow, /作成済み/);

  const untouchedRow = rowFor(html, "タスクが無いゴール");
  assert.match(untouchedRow, /ゴールは作成済みで、タスクはまだ登録されていません/);
  assert.doesNotMatch(untouchedRow, /稼働中/);

  const runningRow = rowFor(html, "実行中のタスクがあるゴール");
  assert.match(runningRow, /ゴールは作成済みで、実行中のタスクが 3 件あります/);
  assert.doesNotMatch(runningRow, /稼働中/);

  const pausedRow = rowFor(html, "一時停止のゴール");
  assert.match(pausedRow, /ゴールは作成済みで、実行は一時停止しています/);
});

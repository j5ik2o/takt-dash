// ゴール一覧の契約（LIST-01 / LIST-02 / LIST-03）。
//
// 一覧は DashboardState.goals を表示用の形へ変換し、全サーバー・全プロジェクトの
// ゴールを既定で表示する。?project= 指定時はそのプロジェクトだけに絞り込む。
import test from "node:test";
import assert from "node:assert/strict";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { ExecutionStatus, GoalStatus, TaskStatus } from "../../src/proto/taktdash.ts";
import { createDashboardState, createGoal } from "../support/fixtures.ts";
import { GoalsContent } from "../../src/components/goals-content.tsx";
import { StateStreamProvider } from "../../src/components/state-provider.tsx";
import GoalsPage from "../../app/goals/page.tsx";

const renderGoals = (state = createDashboardState(), project?: string): string =>
  renderToStaticMarkup(createElement(GoalsContent, { state, project }));

// ページは RSC で searchParams を受け取り、SSE の state を配る Provider の内側で一覧を描く。
// 描画時の配線（searchParams → LiveGoals → GoalsContent）を観測するため、ページ関数を
// 直接呼び出して得た要素を Provider の子として静的に描画する。
const renderGoalsPage = async (project?: string): Promise<string> => {
  const page = await GoalsPage({
    searchParams: Promise.resolve(project === undefined ? {} : { project }),
  });
  return renderToStaticMarkup(
    createElement(StateStreamProvider, { initialState: createDashboardState() }, page),
  );
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

test("ゴール一覧の各行は状態・実行状態・状態別のタスク件数・マージ待ち・プルリクエストを表示する", () => {
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
  const counts = createGoal({
    goalId: "goal-counts",
    name: "状態別の件数を持つゴール",
    status: GoalStatus.GOAL_STATUS_CREATED,
    taskCounts: { pending: 11, running: 12, completed: 13, failed: 14, exceeded: 15, prFailed: 16 },
  });
  const state = createDashboardState();
  const html = renderGoals({ ...state, goals: [humanMerge, failedLast, running, counts] });

  const humanRow = rowFor(html, "人のマージ待ちのゴール");
  assert.match(humanRow, /人のマージ待ち/);
  assert.match(humanRow, /稼働中/);
  assert.match(humanRow, /マージの承認待ちです/);
  assert.match(humanRow, /13/);
  assert.match(humanRow, /418/);
  assert.match(humanRow, /https:\/\/example\.test\/pr\/418/);

  const failedRow = rowFor(html, "仕上げが失敗したゴール");
  assert.match(failedRow, /マージの承認待ちではありません/);
  assert.doesNotMatch(failedRow, /マージの承認待ちです/);

  const runningRow = rowFor(html, "実行中のゴール");
  assert.match(runningRow, /マージの承認待ちではありません/);

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
});

test("ゴール一覧の各行はゴール ID をエスケープした詳細へのリンクを持つ", () => {
  const goal = createGoal({ goalId: "goal/1", name: "リンクを持つゴール" });
  const state = createDashboardState();
  const html = renderGoals({ ...state, goals: [goal] });
  assert.match(html, /href="\/goals\/goal%2F1"/);
  assert.doesNotMatch(html, /href="\/goals\/goal\/1"/);
});

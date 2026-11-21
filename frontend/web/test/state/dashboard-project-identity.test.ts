// 同じ名前のプロジェクトが複数サーバーに存在するとき、run は自サーバーの
// プロジェクトの接続状態で判定される契約。
//
// プロジェクト名だけをキーに辞書を引くと、後から入った別サーバーの状態で
// 上書きされ、他サーバーの run の表示まで巻き込まれる。サーバー ID と名前の
// 組で引くことで、同名プロジェクト同士が独立して判定されることを観測する。
import test from "node:test";
import assert from "node:assert/strict";
import {
  ConnectionState,
  DashboardState,
  RunStatus,
  type Run,
  type Server,
} from "../../src/proto/taktdash.ts";
import { NOW, createThresholds } from "../support/fixtures.ts";
import { buildOverviewView } from "../../src/state/dashboard.ts";

const minutesAgo = (minutes: number): Date => new Date(NOW.getTime() - minutes * 60_000);

const server = (serverId: string, kind: string): Server => ({
  serverId,
  name: `サーバー ${serverId}`,
  kind,
  connected: true,
  lastReadAt: minutesAgo(1),
  writeOnly: false,
});

const run = (serverId: string): Run => ({
  progress: undefined,
  runSlug: `run-${serverId}`,
  sessionId: `session-${serverId}`,
  serverId,
  project: "takt",
  goalId: `goal-${serverId}`,
  taskName: "takt の作業",
  summary: "作業中です。",
  steps: [{ stepName: "implement", model: "glm-5.3", startedAt: minutesAgo(20) }],
  currentStepName: "implement",
  currentStepNumber: 1,
  totalSteps: 1,
  currentIteration: 1,
  lastLogAt: minutesAgo(1),
  reports: [],
  status: RunStatus.RUN_STATUS_RUNNING,
  logSilent: false,
});

const createSharedNameState = (
  stateA: ConnectionState,
  stateB: ConnectionState,
): DashboardState =>
  DashboardState.create({
    readAt: NOW,
    servers: [server("a", "Mac mini"), server("b", "Mac Studio")],
    projects: [
      {
        name: "takt",
        about: "",
        serverId: "a",
        completedTasks24h: 0,
        connectionState: stateA,
        lastReadAt: minutesAgo(5),
      },
      {
        name: "takt",
        about: "",
        serverId: "b",
        completedTasks24h: 0,
        connectionState: stateB,
        lastReadAt: minutesAgo(1),
      },
    ],
    runs: [run("a"), run("b")],
    thresholds: createThresholds(),
  });

test("同名プロジェクトの集計は登録済みの実行中runだけを自サーバーへ数え、設定と履歴を保つ", () => {
  const base = createSharedNameState(ConnectionState.CONNECTION_STATE_CONNECTED, ConnectionState.CONNECTION_STATE_CONNECTED);
  const state = DashboardState.create({ ...base,
    servers: base.servers.filter((server) => server.serverId === "a"),
    projects: base.projects.map((project, index) => ({ ...project, concurrency: index + 2, completedTasks24h: index + 3 })),
    runs: [...base.runs, { ...run("a"), runSlug: "completed", status: RunStatus.RUN_STATUS_COMPLETED }],
  });
  const view = buildOverviewView(state, NOW, true);
  assert.equal(view.runningRuns, 1);
  assert.deepEqual(view.projects.map(({ serverId, runningRuns }) => [serverId, runningRuns]), [["a", 1], ["b", 0]]);
  assert.equal(view.runLimit, 5);
  assert.equal(view.completedTasks24h, 7);
  assert.equal(view.runs.length, 3, "未登録と完了済みのrunも詳細表示用に保持する");
});

test("同名プロジェクトでも遅延中のサーバーの run は遅延表示にならず、他サーバーへ波及しない", () => {
  const view = buildOverviewView(
    createSharedNameState(ConnectionState.CONNECTION_STATE_DELAYED, ConnectionState.CONNECTION_STATE_CONNECTED),
    NOW, true,
  );
  const delayed = view.runs.find((item) => item.run.serverId === "a");
  const healthy = view.runs.find((item) => item.run.serverId === "b");
  assert.ok(delayed !== undefined, "サーバー a の run がある");
  assert.ok(healthy !== undefined, "サーバー b の run がある");

  assert.equal(delayed.state, "live", "遅延中でもログが出続けている run は実行中のままになる");
  assert.equal(delayed.connected, true, "遅延中は接続中として扱う");
  assert.equal(delayed.note, "", "読み取り時刻の注記は付けない");
  assert.equal(healthy.state, "live", "同じ名前でもサーバー b の run は実行中のままになる");
  assert.equal(healthy.connected, true);
});

test("同名プロジェクトでも応答なしのサーバーの run だけが停止表示になり、他サーバーへ波及しない", () => {
  const view = buildOverviewView(
    createSharedNameState(
      ConnectionState.CONNECTION_STATE_UNRESPONSIVE,
      ConnectionState.CONNECTION_STATE_CONNECTED,
    ),
    NOW, true,
  );
  const unresponsive = view.runs.find((item) => item.run.serverId === "a");
  const healthy = view.runs.find((item) => item.run.serverId === "b");
  assert.ok(unresponsive !== undefined, "サーバー a の run がある");
  assert.ok(healthy !== undefined, "サーバー b の run がある");

  assert.equal(unresponsive.connected, false, "応答なしのサーバー a の run は接続なしになる");
  assert.equal(unresponsive.state, "idle", "応答なしは最後に読めた時点の idle 表示になる");
  assert.match(unresponsive.label, /時点/, "最後に読めた時点を明示する");
  assert.equal(healthy.connected, true, "同名でもサーバー b の run は接続中のままになる");
  assert.equal(healthy.state, "live", "サーバー b の run は実行中のままになる");
});

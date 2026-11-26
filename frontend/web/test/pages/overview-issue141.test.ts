// 概要の警告バナーとタイルから、失敗したタスクと中止したゴールの件数を外す契約（issue #141）。
//
// 失敗と中止は要件 2.7 の異常（判断待ちの滞留とログの無音）でも接続の異常でもなく、画面から
// 対処もできない。バナーは判断待ち・応答なし・ログの無音だけで出し、残る文は main の文面と
// 並び順のまま出す。設計は docs/design/issue-141.md。描画結果の HTML から観測する。
import test from "node:test";
import assert from "node:assert/strict";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { createDashboardState, createHealthyDashboardState, NOW, SILENT_MINUTES } from "../support/fixtures.ts";
import { elementsWithClass, elementWithAttribute, textOf } from "../support/markup.ts";
import {
  type DashboardState,
  ConnectionState,
  ExecutionStatus,
  InquiryKind,
  PendingDecision,
  PendingDecisionType,
} from "../../src/proto/taktdash.ts";
import { buildOverviewView } from "../../src/state/dashboard.ts";
import { OverviewContent } from "../../src/components/overview-content.tsx";

const minutesAgo = (minutes: number): Date => new Date(NOW.getTime() - minutes * 60_000);

const renderOverview = (state: DashboardState, disconnected = false): string =>
  renderToStaticMarkup(
    createElement(OverviewContent, { view: buildOverviewView(state, NOW, true), disconnected }),
  );

// 警告バナーの文を、描画された順に返す。バナーが無ければ undefined。
const bannerLines = (html: string): string[] | undefined => {
  const banner = elementsWithClass(html, "health")[0];
  if (banner === undefined) {
    return undefined;
  }
  return Array.from(banner.outer.matchAll(/<p\b[^>]*>([\s\S]*?)<\/p>/g), (match) => textOf(match[1]!));
};

const completedTile = (html: string): string => {
  const tiles = elementWithAttribute(html, "aria-label", "概要の集計").outer;
  const tile = elementsWithClass(tiles, "tile").find((candidate) =>
    candidate.outer.includes("24時間に完了したタスク"),
  );
  if (tile === undefined) {
    assert.fail("「24時間に完了したタスク」のタイルが描画されていない");
  }
  return tile.outer;
};

const tileNotes = (tile: string): string[] => elementsWithClass(tile, "t-note").map((note) => textOf(note.outer));

// 消す3種類の文。どこにも出てはならない。
const FAILED_COUNT_TEXT = /失敗・上限超過・PR 失敗のタスクが\s*\d+件/;
const ABORTED_COUNT_TEXT = /中止したゴールが\s*\d+件/;

const assertNoCountText = (html: string, context: string): void => {
  assert.doesNotMatch(textOf(html), FAILED_COUNT_TEXT, `失敗したタスクの件数の文を出さない（${context}）`);
  assert.doesNotMatch(textOf(html), ABORTED_COUNT_TEXT, `中止したゴールの件数の文を出さない（${context}）`);
};

// main のバナーの文面（残す文）。
const PENDING_LINE = (count: number): string => `判断待ちが ${count}件あります。`;
const SERVER_DOWN_LINE = "Mac Studio から応答がありません。";
const PROJECT_DOWN_LINE = "esa から応答がありません。";
const SILENT_LINE = "ログが止まっている run が 1件あります。";

// main のタイルの注記の文面（残す文）。
const HISTORY_NOTE = "保存済みの履歴を全プロジェクト分集計しています。";
const UNREAD_NOTE = "未読のプロジェクトの最新状態はまだ確認できていません。";

type TaskFailures = { failed: number; exceeded: number; prFailed: number };

type Scenario = {
  // 1件ごとの判断待ちの待ち時間（分）。
  pendingWaits: number[];
  serverDown: boolean;
  projectDown: boolean;
  silent: boolean;
  // ゴールごとの失敗系の件数と、中止したかどうか。ゴールは2件。
  failures: [TaskFailures, TaskFailures];
  aborted: [boolean, boolean];
};

const NO_FAILURES: TaskFailures = { failed: 0, exceeded: 0, prFailed: 0 };

// 健全な状態に、シナリオの異常と失敗・中止を載せる。判断待ちと無音の run は、応答なしにしうる
// esa ではなく corporate-site に置き、応答なしが判断待ちや無音の判定に混ざらないようにする。
const buildState = (scenario: Scenario): DashboardState => {
  const base = createHealthyDashboardState();
  return {
    ...base,
    servers: base.servers.map((server) =>
      server.serverId === "down" ? { ...server, connected: !scenario.serverDown } : server,
    ),
    projects: base.projects.map((project) =>
      project.name === "esa" && scenario.projectDown
        ? { ...project, connected: false, connectionState: ConnectionState.CONNECTION_STATE_UNRESPONSIVE }
        : project,
    ),
    goals: base.goals.map((goal, index) => ({
      ...goal,
      executionStatus: scenario.aborted[index]
        ? ExecutionStatus.EXECUTION_STATUS_ABORTED
        : ExecutionStatus.EXECUTION_STATUS_ACTIVE,
      taskCounts: { pending: 0, running: 0, completed: 6, ...scenario.failures[index]! },
    })),
    runs: base.runs.map((run) =>
      scenario.silent
        ? { ...run, logSilent: true, lastLogAt: new Date(NOW.getTime() - (SILENT_MINUTES * 60_000 + 1_000)) }
        : run,
    ),
    pendingDecisions: scenario.pendingWaits.map((waitMinutes, index) => PendingDecision.create({
      type: PendingDecisionType.PENDING_DECISION_TYPE_INQUIRY,
      serverId: "mini",
      project: "corporate-site",
      goalId: "goal-1",
      inquiryKind: InquiryKind.INQUIRY_KIND_QUESTION,
      inquiryId: `inquiry-${index}`,
      createdAt: minutesAgo(waitMinutes),
      body: `問い合わせ ${index}`,
      options: [],
      stale: waitMinutes >= 240,
    })),
  };
};

// 異常の判定だけから、main の文面と並び順でバナーの文を作る。失敗と中止は見ない。
const expectedBannerLines = (scenario: Scenario): string[] | undefined => {
  const lines = [
    ...(scenario.pendingWaits.length > 0 ? [PENDING_LINE(scenario.pendingWaits.length)] : []),
    ...(scenario.serverDown ? [SERVER_DOWN_LINE] : []),
    ...(scenario.projectDown ? [PROJECT_DOWN_LINE] : []),
    ...(scenario.silent ? [SILENT_LINE] : []),
  ];
  return lines.length > 0 ? lines : undefined;
};

// 判断待ち（滞留1件と滞留していない1件）・応答のないサーバー・応答のないプロジェクト・ログの無音を全部載せる。
const allAnomalies = (failures: Scenario["failures"], aborted: Scenario["aborted"]): Scenario => ({
  pendingWaits: [300, 30],
  serverDown: true,
  projectDown: true,
  silent: true,
  failures,
  aborted,
});

const MANY_FAILURES: [TaskFailures, TaskFailures] = [
  { failed: 21, exceeded: 2, prFailed: 3 },
  { failed: 1, exceeded: 0, prFailed: 5 },
];

test("失敗したタスクと中止したゴールしか無いときは、警告バナーを出さない", () => {
  const html = renderOverview(
    buildState({
      pendingWaits: [],
      serverDown: false,
      projectDown: false,
      silent: false,
      failures: MANY_FAILURES,
      aborted: [true, true],
    }),
  );

  assert.equal(bannerLines(html), undefined, "異常が無いので警告バナーを描画しない");
  assertNoCountText(html, "失敗と中止だけ");
});

test("異常と失敗・中止が同時にあるとき、バナーには異常の文だけが main の文面と順序で並ぶ", () => {
  const html = renderOverview(buildState(allAnomalies(MANY_FAILURES, [true, false])));

  assert.deepEqual(
    bannerLines(html),
    [PENDING_LINE(2), SERVER_DOWN_LINE, PROJECT_DOWN_LINE, SILENT_LINE],
    "判断待ち → 応答のないサーバー → 応答のないプロジェクト → ログの無音の順に、異常の文だけを並べる",
  );
  assertNoCountText(html, "異常と同時");
});

test("失敗・中止が無いときのバナーの文面と並び（main の原典）を保つ", () => {
  const html = renderOverview(buildState(allAnomalies([NO_FAILURES, NO_FAILURES], [false, false])));

  assert.deepEqual(bannerLines(html), [PENDING_LINE(2), SERVER_DOWN_LINE, PROJECT_DOWN_LINE, SILENT_LINE]);
});

test("ログの無音だけが異常のとき、失敗と中止があってもバナーは無音の文だけを出す", () => {
  const html = renderOverview(
    buildState({
      pendingWaits: [],
      serverDown: false,
      projectDown: false,
      silent: true,
      failures: MANY_FAILURES,
      aborted: [true, true],
    }),
  );

  assert.deepEqual(bannerLines(html), [SILENT_LINE]);
});

test("滞留した判断待ちだけが異常のとき、失敗と中止があってもバナーは判断待ちの文だけを出す", () => {
  const html = renderOverview(
    buildState({
      pendingWaits: [240],
      serverDown: false,
      projectDown: false,
      silent: false,
      failures: MANY_FAILURES,
      aborted: [true, true],
    }),
  );

  assert.deepEqual(bannerLines(html), [PENDING_LINE(1)]);
});

test("24時間に完了したタスクのタイルは、失敗したタスクがあっても件数の注記を出さない", () => {
  const html = renderOverview(
    buildState({
      pendingWaits: [],
      serverDown: false,
      projectDown: false,
      silent: false,
      failures: MANY_FAILURES,
      aborted: [false, false],
    }),
  );
  const tile = completedTile(html);

  assert.match(tile, /<div class="num">\s*4\s*<small>件<\/small>/, "完了したタスクの合計は main のまま出す");
  assert.deepEqual(tileNotes(tile), [HISTORY_NOTE], "注記は保存済みの履歴の1文だけにする");
});

test("未読のプロジェクトがあるときのタイルの注記は、失敗したタスクがあっても main の2文だけを並べる", () => {
  const base = buildState({
    pendingWaits: [],
    serverDown: false,
    projectDown: false,
    silent: false,
    failures: MANY_FAILURES,
    aborted: [false, false],
  });
  const html = renderOverview({
    ...base,
    projects: base.projects.map((project) => (project.name === "esa" ? { ...project, lastReadAt: undefined } : project)),
  });

  assert.deepEqual(tileNotes(completedTile(html)), [HISTORY_NOTE, UNREAD_NOTE]);
});

test("main のフィクスチャ（判断待ち2件・応答なしのサーバー・失敗1件）のバナーは、失敗の文を除いた main の並びになる", () => {
  const html = renderOverview(createDashboardState());

  assert.deepEqual(bannerLines(html), [PENDING_LINE(2), SERVER_DOWN_LINE]);
  assert.deepEqual(tileNotes(completedTile(html)), [HISTORY_NOTE]);
});

// 種つきの乱数でシナリオを作る（除外なし）。件数は0・1・多数・極端な値を混ぜる。
const generatedScenarios = (): Scenario[] => {
  let seed = 141;
  const next = (): number => {
    seed = (seed * 1_103_515_245 + 12_345) % 2_147_483_648;
    return seed;
  };
  const pick = <T>(values: readonly T[]): T => values[next() % values.length]!;
  const count = (): number => pick([0, 0, 1, 2, 5, 21, 999, 100_000, next() % 60]);
  const failures = (): TaskFailures => ({ failed: count(), exceeded: count(), prFailed: count() });
  const flag = (): boolean => next() % 2 === 0;
  const waits = [0, 1, 30, 239, 240, 241, 300, 10_000];
  return Array.from({ length: 400 }, () => ({
    pendingWaits: Array.from({ length: next() % 4 }, () => pick(waits)),
    serverDown: flag(),
    projectDown: flag(),
    silent: flag(),
    failures: [failures(), failures()],
    aborted: [flag(), flag()],
  }));
};

test("任意の失敗・中止の件数で、件数の文は出ず、バナーの文は異常の判定だけで決まる（生成したシナリオ）", () => {
  for (const scenario of generatedScenarios()) {
    for (const disconnected of [false, true]) {
      const html = renderOverview(buildState(scenario), disconnected);
      const context = JSON.stringify({ ...scenario, disconnected });

      assertNoCountText(html, context);
      assert.deepEqual(bannerLines(html), expectedBannerLines(scenario), context);
      // 同じシナリオから失敗と中止を除いても、バナーは変わらない。
      const withoutFailures = renderOverview(
        buildState({ ...scenario, failures: [NO_FAILURES, NO_FAILURES], aborted: [false, false] }),
        disconnected,
      );
      assert.deepEqual(bannerLines(html), bannerLines(withoutFailures), context);
      for (const note of tileNotes(completedTile(html))) {
        assert.ok([HISTORY_NOTE, UNREAD_NOTE].includes(note), `タイルの注記は main の2文のどちらかにする: ${note} ${context}`);
      }
    }
  }
});

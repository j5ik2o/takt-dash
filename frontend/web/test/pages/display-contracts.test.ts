import test from "node:test";
import assert from "node:assert/strict";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { JSDOM } from "jsdom";
import OverviewPage from "../../app/page.tsx";
import ProjectsPage from "../../app/projects/page.tsx";
import { DecisionsContent } from "../../src/components/decisions-content.tsx";
import { GoalsContent } from "../../src/components/goals-content.tsx";
import { Sidebar } from "../../src/components/sidebar.tsx";
import { OverviewContent } from "../../src/components/overview-content.tsx";
import { ConnectionState, DashboardState, ExecutionStatus, GoalStatus, RunStatus, TaskStatus } from "../../src/proto/taktdash.ts";
import { buildOverviewView } from "../../src/state/dashboard.ts";
import { dashboardStateFromProto } from "../../src/state/restore-state.ts";
import { createDashboardState, createGoal, createHealthyDashboardState, NOW } from "../support/fixtures.ts";
import { assertUnreadRunList, assertRejectsUnreadEmptyClaim } from "../support/run-list.ts";

const overview = (state: DashboardState): Document => new JSDOM(renderToStaticMarkup(
  createElement(OverviewPage, { view: buildOverviewView(state, NOW, true) }),
)).window.document;
const projects = (state: DashboardState): Document => new JSDOM(renderToStaticMarkup(
  createElement(ProjectsPage, { state, now: NOW }),
)).window.document;
const required = (root: ParentNode, selector: string): Element => {
  const element = root.querySelector(selector);
  assert.ok(element, `${selector} を描画する`);
  return element;
};
const text = (element: Element): string => element.textContent?.replace(/\s+/g, " ").trim() ?? "";
const projectSection = (document: Document): Element => required(document, '[aria-labelledby="h-projects"]');
const activeCount = (document: Document): number => {
  const section = text(projectSection(document));
  const match = /進行中のゴール[^\d]*?(\d+)\s*件/.exec(section);
  assert.ok(match, `進行中のゴールの集計を表示する: ${section}`);
  return Number(match[1]);
};

test("概要が数えるゴールの ID 集合はプロジェクトの進行中区画と一致し、他の区画とリンクを保つ", () => {
  const base = createHealthyDashboardState();
  const goals = [
    createGoal({ goalId: "ordinary", taskCounts: { pending: 1, running: 0, completed: 0, failed: 0, exceeded: 0, prFailed: 0 } }),
    createGoal({ goalId: "human-merge", status: GoalStatus.GOAL_STATUS_AWAITING_MERGE }),
    createGoal({ goalId: "finishing-pending", status: GoalStatus.GOAL_STATUS_AWAITING_MERGE, mergeTaskLast: TaskStatus.TASK_STATUS_PENDING }),
    createGoal({ goalId: "paused", executionStatus: ExecutionStatus.EXECUTION_STATUS_PAUSED }),
    createGoal({ goalId: "untouched" }),
    createGoal({ goalId: "completed", status: GoalStatus.GOAL_STATUS_COMPLETED }),
    createGoal({ goalId: "aborted", executionStatus: ExecutionStatus.EXECUTION_STATUS_ABORTED }),
    createGoal({ goalId: "finishing-running", status: GoalStatus.GOAL_STATUS_AWAITING_MERGE, mergeTaskLast: TaskStatus.TASK_STATUS_RUNNING }),
  ].map((goal, index) => ({ ...goal, name: goal.goalId, project: `project-${index}` }));
  const state = DashboardState.create({
    ...base, goals, runs: [], pendingDecisions: [],
    projects: goals.map((goal) => ({ ...base.projects[0], name: goal.project })),
  });
  const expected = ["finishing-pending", "finishing-running", "human-merge", "ordinary", "paused"];
  const page = projects(state);
  const visibleIds = goals.filter((goal) => {
    const card = required(page, `#project-${goal.project}`);
    return Array.from(card.querySelectorAll("h4")).some((heading) =>
      text(heading) === goal.goalId && heading.closest("details") === null);
  }).map((goal) => goal.goalId).sort();
  assert.deepEqual(visibleIds, expected);
  for (const [id, fold] of [["untouched", "goal-untouched"], ["completed", "goal-done"], ["aborted", "goal-aborted"]]) {
    const heading = Array.from(page.querySelectorAll("h4")).find((item) => text(item) === id);
    assert.ok(heading?.closest(`details.${fold}`), `${id} を従来の折りたたみで閲覧できる`);
  }
  const summary = overview(state);
  assert.ok(projectSection(summary).querySelectorAll('a[href^="/projects#"]').length < goals.length);
  assert.equal(activeCount(summary), expected.length, "カード上限の外も集計する");
  // 各 ID を除いたときの表示件数の差から、集計への所属を直接観測する。
  const countedIds = goals.filter((goal) => activeCount(overview(DashboardState.create({
    ...state, goals: goals.filter((item) => item.goalId !== goal.goalId),
  }))) === expected.length - 1).map((goal) => goal.goalId).sort();
  assert.deepEqual(countedIds, expected);
  assert.deepEqual(countedIds, visibleIds);
  assert.ok(summary.querySelector('a[href="/projects#project-project-0"]'));
  assert.ok(summary.querySelector('a[href="/goals?project=project-0"]'));
  assert.ok(summary.querySelector('a[href="/projects"]'));
});

for (const registered of [true, false]) {
  test(registered ? "登録済みで接続済みの run は両画面で実行中になる" : "起動設定だけにある接続先の run は両画面で接続未確認と分かり、詳細を閲覧できる", () => {
    const base = createHealthyDashboardState();
    const state = DashboardState.create({ ...base, servers: registered ? base.servers : [],
      projects: base.projects.filter((project) => project.name === "corporate-site")
        .map((project) => ({ ...project, concurrency: 5 })),
    });
    const view = buildOverviewView(state, NOW, true).runs[0];
    assert.ok(view);
    const summary = overview(state);
    const detail = projects(state);
    const count = registered ? 1 : 0;
    assert.equal(view.run.status, RunStatus.RUN_STATUS_RUNNING);
    assert.match(text(required(summary, ".page-head")), new RegExp(`${count}件の run`));
    const usage = required(summary, '.tile [role="img"]');
    assert.equal(usage.getAttribute("aria-label"), `実行上限 5 件に対して ${count} 件`);
    assert.equal(required(usage, "i").getAttribute("style"), `width:${count * 20}%`);
    assert.match(text(required(summary, 'a[href="/projects#project-corporate-site"]')),
      new RegExp(`実行中の run は ${count} 件`));
    const row = required(summary, ".run-row");
    const projectRun = required(detail, "#project-corporate-site .mini-runs li");
    for (const element of [row, projectRun]) {
      assert.ok(element.querySelector('a[href="/runs/run-aaa"]'), "run を消さず詳細リンクを残す");
    }
    const labels = [text(required(row, ".st")), text(required(projectRun, "span:nth-of-type(2)"))];
    if (registered) {
      assert.deepEqual(labels, ["実行中", "実行中"]);
      assert.equal(view.connected, true);
      assert.equal(view.state, "live");
    } else {
      assert.equal(view.connected, false);
      assert.notEqual(view.state, "live");
      for (const label of labels) {
        assert.match(label, /未登録|未確認|確認でき|接続でき|不明/);
        assert.notEqual(label, "実行中");
      }
    }
  });
}

for (const condition of ["server", "project", "silent"] as const) {
  test(`登録済みrunは${condition}の状態でも件数と詳細リンクを保つ`, () => {
    const base = createHealthyDashboardState();
    const state = DashboardState.create({ ...base,
      servers: base.servers.map((server) => ({ ...server, connected: condition !== "server" })),
      projects: base.projects.map((project) => ({ ...project,
        connectionState: condition === "project"
          ? ConnectionState.CONNECTION_STATE_UNRESPONSIVE : project.connectionState,
      })),
      runs: base.runs.map((run) => condition === "silent"
        ? { ...run, logSilent: true, lastLogAt: new Date(NOW.getTime() - 11 * 60_000) }
        : run),
    });
    const view = buildOverviewView(state, NOW, true);
    assert.equal(view.runningRuns, 1);
    assert.equal(view.projects.find((project) => project.name === "corporate-site")?.runningRuns, 1);
    for (const [page, selector] of [[overview(state), ".run-row"], [projects(state), "#project-corporate-site .mini-runs li"]] as const) {
      const row = required(page, selector);
      assert.ok(row.querySelector('a[href="/runs/run-aaa"]'));
      assert.match(text(row), condition === "silent" ? /ログが止まっています/ : /実行中（.*時点）/);
    }
  });
}

const emptyState = (read: boolean): DashboardState => {
  const base = createHealthyDashboardState();
  return DashboardState.create({
    ...base, readAt: read ? NOW : undefined, goals: [], runs: [], pendingDecisions: [],
    servers: base.servers.map((server) => ({ ...server, connected: read, lastReadAt: read ? NOW : undefined })),
    projects: base.projects.map((project) => ({
      ...project, completedTasks24h: 0, connected: read, lastReadAt: read ? NOW : undefined,
      connectionState: read ? ConnectionState.CONNECTION_STATE_CONNECTED : ConnectionState.CONNECTION_STATE_UNSPECIFIED,
    })),
  });
};
const renderers = {
  概要: overview,
  判断待ち: (state: DashboardState) => new JSDOM(renderToStaticMarkup(createElement(DecisionsContent, { snapshotLoaded: true, state, now: NOW, disconnected: false }))).window.document,
  ゴール: (state: DashboardState) => new JSDOM(renderToStaticMarkup(createElement(GoalsContent, { snapshotLoaded: true, state: dashboardStateFromProto(state), disconnected: false }))).window.document,
  プロジェクト: projects,
};

for (const [name, render] of Object.entries(renderers)) {
  test(`${name}は初回読み取り前の見出し・集計・空表示で0件を確定しない`, () => {
    const document = render(emptyState(false));
    const main = required(document, "main");
    const countText = text(main);
    assert.doesNotMatch(countText, /(?:^|[^\d])0\s*件|0\s*\/|(?:run|ゴール|判断待ち)はありません/);
    assert.match(countText, /未読|まだ.*読|不明|分かりません|確認中/);
    if (name === "概要") {
      assert.match(text(required(document, '[role="status"]')), /応答がありません/);
      assertUnreadRunList(main);
      assertRejectsUnreadEmptyClaim(main);
    }
    for (const image of main.querySelectorAll('[role="img"]')) {
      assert.doesNotMatch(image.getAttribute("aria-label") ?? "", /(?:^|[^\d])0\s*件/);
    }
  });
  test(`${name}は成功して空の一覧を一度だけ示す`, () => {
    const document = render(emptyState(true));
    const scopes = name === "プロジェクト"
      ? [required(document, "#project-corporate-site .mini-runs").parentElement, required(document, "#project-corporate-site .goals").parentElement]
      : name === "概要" ? [required(document, '[aria-labelledby="h-runs"]')]
        : [required(document, "main")];
    for (const scope of scopes) {
      assert.ok(scope);
      const content = text(scope);
      const emptyExpressions = content.match(/(?:^|[^\d])0\s*件|ありません/g) ?? [];
      assert.equal(emptyExpressions.length, 1, content);
      assert.doesNotMatch(content, /分かりません|未読|まだ.*読め/);
    }
  });
  test(`${name}は読み取り成功後の実数とデータ行を表示する`, () => {
    const state = createHealthyDashboardState();
    const document = render(state);
    if (name === "概要") {
      assert.match(text(required(document, ".page-head")), /1件の run/);
      assert.ok(document.querySelector('a[href="/runs/run-aaa"]'));
      assert.match(text(projectSection(document)), /完了したタスクは 3件/);
    } else if (name === "判断待ち") {
      const base = createHealthyDashboardState();
      const withDecision = DashboardState.create({ ...base,
        pendingDecisions: createDashboardState().pendingDecisions.slice(0, 1),
      });
      const page = render(withDecision);
      assert.match(text(required(page, ".page-head")), /1件/);
      assert.match(text(required(page, ".tray-item")), /リリースの日取りはどうしますか/);
    } else if (name === "ゴール") {
      assert.match(text(required(document, ".page-head")), /ゴールを2件/);
      assert.ok(document.querySelector('a[href="/goals/goal-1"]'));
    } else {
      const card = required(document, "#project-corporate-site");
      assert.match(text(card), /run（1件）/);
      assert.match(text(card), /進行中のゴール（1件）/);
      assert.ok(card.querySelector('a[href="/runs/run-aaa"]'));
    }
  });
}

const mixedReadState = (): DashboardState => {
  const base = createHealthyDashboardState();
  return DashboardState.create({ ...base,
    goals: base.goals.filter((goal) => goal.project === "corporate-site"),
    projects: base.projects.map((project) =>
      project.name === "esa" ? { ...project, completedTasks24h: 0, lastReadAt: undefined } : project),
  });
};
for (const [name, render] of [["概要", overview], ["プロジェクト", projects]] as const) {
  test(`${name}の既読カードの件数は別プロジェクトの未読で消えず、未読カードは全体時刻で既読にならない`, () => {
    const document = render(mixedReadState());
    const known = document.querySelector("#project-corporate-site") ?? required(document, 'a[href="/projects#project-corporate-site"]');
    const unread = document.querySelector("#project-esa") ?? required(document, 'a[href="/projects#project-esa"]');
    assert.match(text(known), /run[^\d]*1\s*件/);
    assert.doesNotMatch(text(unread), /(?:^|[^\d])0\s*件|ありません/);
    assert.match(text(unread), /未読|まだ.*読|不明|分かりません|確認中/);
  });
}
test("ゴールの絞り込みでは別プロジェクトが未読でも対象の実数を表示する", () => {
  const filtered = new JSDOM(renderToStaticMarkup(createElement(GoalsContent, { snapshotLoaded: true,
    state: dashboardStateFromProto(mixedReadState()), project: "corporate-site", disconnected: false,
  }))).window.document;
  assert.match(text(required(filtered, ".page-head")), /ゴールを1件/);
  assert.doesNotMatch(text(required(filtered, ".page-head")), /まだ読|不明/);
});

test("共通サイドバーは未読のサーバーをrun 0件とせず、既読で空なら0件を表示する", () => {
  for (const read of [false, true]) {
    const state = emptyState(read);
    const build = { commit: "test", branch: "test" };
    const document = new JSDOM(renderToStaticMarkup(createElement(Sidebar, {
      snapshotLoaded: true,
      servers: state.servers, projects: state.projects, runs: [], pendingCount: 0,
      frontendBuild: build, backendBuild: build,
    }))).window.document;
    const row = required(document, ".srv-list li");
    if (read) assert.match(text(row), /run\s*0件/);
    else {
      assert.doesNotMatch(text(row), /run\s*0件/);
      assert.match(text(row), /未読|まだ.*読|不明|確認中/);
    }
  }
});

const withCoverage = (coverage: "unread" | "partial" | "complete", history: number[]): DashboardState => {
  const base = emptyState(true);
  return DashboardState.create({ ...base,
    servers: base.servers.map((server) => ({ ...server, lastReadAt: coverage === "unread" ? undefined : NOW })),
    projects: base.projects.map((project, index) => ({
      ...project, concurrency: index === 0 ? 3 : 2,
      completedTasks24h: history[index],
      lastReadAt: coverage === "complete" || (coverage === "partial" && index === 0) ? NOW : undefined,
    })),
  });
};

for (const coverage of ["unread", "partial", "complete"] as const) {
  test(`概要は${coverage}でも設定上限5件を目視と読み上げへ残す`, () => {
    const document = overview(withCoverage(coverage, [0, 0]));
    const tile = required(document, ".tile");
    const visible = text(tile);
    assert.match(visible, /(?:上限\s*5|\/\s*5)\s*件/);
    if (coverage === "complete") {
      // #424（オーナーの指摘）: 読み切れて run が0件のときは「0 / 5 件」と出し、上限の語は出さない。
      // ただし 5 という数は残る（上限が消えたのではなく、0 / 5 件として残っている）。
      assert.match(visible, /0\s*\/\s*5\s*件/);
      assert.doesNotMatch(visible, /上限/);
      assert.match(visible, /5\s*件/);
    } else {
      assert.match(visible, /不明|読めた分|上限/);
    }
    if (coverage !== "complete") {
      assert.equal(tile.querySelectorAll('[role="img"]').length, 0, "未確定の全体使用率を描画しない");
      assert.doesNotMatch(visible, /(?:^|[^\d])0\s*\/\s*5/);
    }
  });

  test(`概要は${coverage}でも未読側の保存履歴を合計とカードに表示する`, () => {
    const document = overview(withCoverage(coverage, [2, 1]));
    const tile = required(document, ".tile:nth-child(3)");
    assert.match(text(required(tile, ".num")), /^3\s*件$/);
    assert.match(text(tile), /保存.*履歴/);
    assert.doesNotMatch(text(tile), /読めた分|不明/);
    const card = required(document, 'a[href="/projects#project-esa"]');
    assert.match(text(card), /完了したタスクは 1件/);
    assert.match(text(card), /保存.*履歴/);
    if (coverage !== "complete") assert.match(text(card), /run.*不明/);
    const head = text(required(document, ".page-head"));
    if (coverage !== "complete") assert.match(head, /run・判断待ち・ゴールの件数には未読の分を含めていません/);
    assert.doesNotMatch(head, /(?:^|。)件数には未読の分を含めていません/);
  });
}

test("概要の未読時は保存履歴が0でも動的な完了件数を確定しない", () => {
  const document = overview(withCoverage("unread", [0, 0]));
  const tile = required(document, ".tile:nth-child(3)");
  assert.match(text(tile), /不明|未読|まだ.*読/);
  assert.doesNotMatch(text(required(tile, ".num")), /0\s*件/);
  assert.doesNotMatch(text(required(document, 'a[href="/projects#project-esa"]')), /完了したタスクは 0件/);
});

// issue #424（オーナーの指摘：0件のとき「上限 5 件」と出て5件走っているように読めた）で、
// 既読の空 run 集合のタイルは「0 / 5 件」になり「上限」を出さなくなる。そのため、ループ内の
// 「概要は初回読み取り前の見出し・集計・空表示で0件を確定しない」と同名のテストを別名で
// 1本足す（node --test が同名テストを区別できず、どれが落ちたか読めなくなるため）。
// ここで確かめるのは「初回読み取り前の概要は、run の件数を0件と確定せず、run 一覧も空と
// 断定しない」だけ。期待は既存の概要ケースと一字も変えない。
test("概要は読み取り前の空の一覧でrunの件数を0件と確定しない", () => {
  const document = overview(emptyState(false));
  const main = required(document, "main");
  const countText = text(main);
  assert.doesNotMatch(countText, /(?:^|[^\d])0\s*件|0\s*\/|(?:run|ゴール|判断待ち)はありません/);
  assert.match(countText, /未読|まだ.*読|不明|分かりません|確認中/);
  assert.match(text(required(document, '[role="status"]')), /応答がありません/);
  assertUnreadRunList(main);
  assertRejectsUnreadEmptyClaim(main);
  for (const image of main.querySelectorAll('[role="img"]')) {
    assert.doesNotMatch(image.getAttribute("aria-label") ?? "", /(?:^|[^\d])0\s*件/);
  }
});

test("概要の既読で空のrun集合はヘッダー・タイル・一覧・読み上げを通して一度だけ空と示す", () => {
  const document = overview(withCoverage("complete", [0, 0]));
  const scopes = [required(document, ".page-head"), required(document, ".tile"), required(document, '[aria-labelledby="h-runs"]')];
  const content = scopes.map((scope) => `${text(scope)} ${Array.from(scope.querySelectorAll('[aria-label]')).map((item) => item.getAttribute("aria-label")).join(" ")}`).join(" ");
  // 空の表明は、タイルの「0 / 5 件」と一覧の文がそれぞれ1回ずつだけ出る（退行で二重にならない）。
  // 「0件」「0 /」「ありません」の類をまとめて拾い、この2つのほかに空の表明が無いことを確かめる（#444）。
  // 見出しなどに「0件の run が動いています。」のような空の表明が増えると、ここで落ちる。
  const emptyClaims = (content.match(/(?<!\d)0\s*\/(?:\s*\d+\s*件)?|(?<!\d)0\s*件|ありません/g) ?? []).map((claim) => claim.replace(/\s+/g, ""));
  assert.deepEqual(emptyClaims, ["0/5件", "ありません"], content);
  assert.equal((content.match(/0\s*\/\s*5\s*件/g) ?? []).length, 1, content);
  assert.equal((content.match(/走っている run はありません/g) ?? []).length, 1, content);
  assert.match(text(required(document, ".page-head")), /2台のサーバー/);
  assert.match(text(required(document, ".tile")), /0\s*\/\s*5\s*件/);
  assert.doesNotMatch(text(required(document, ".tile")), /上限/);
  assert.equal(required(document, ".tile").querySelectorAll('[role="img"]').length, 0);
});

test("概要は実行中件数が0でも存在するrunの行と詳細リンクを残す", () => {
  const base = createHealthyDashboardState();
  const state = DashboardState.create({ ...base, runs: base.runs.map((run) => ({ ...run, status: RunStatus.RUN_STATUS_COMPLETED })) });
  const document = overview(state);
  assert.equal(document.querySelectorAll(".run-row").length, 1);
  assert.ok(document.querySelector('a[href="/runs/run-aaa"]'));
  assert.doesNotMatch(text(required(document, '[aria-labelledby="h-runs"]')), /走っている run はありません/);
});

test("概要は初回バックエンド取得失敗の代替状態から設定上限を推定しない", () => {
  const document = new JSDOM(renderToStaticMarkup(createElement(OverviewContent, {
    view: buildOverviewView(DashboardState.create({}), NOW, false), disconnected: true,
    unavailableSituation: { failure: "unreachable", respondedBefore: false },
  }))).window.document;
  const tile = required(document, ".tile");
  assert.match(text(tile), /不明/);
  assert.doesNotMatch(text(tile), /上限\s*0|\/\s*0/);
  assert.equal(tile.querySelectorAll('[role="img"]').length, 0);
  assert.ok(document.querySelector('[role="alert"]'));
});

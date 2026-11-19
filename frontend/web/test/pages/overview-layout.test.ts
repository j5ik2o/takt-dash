// 概要へレイアウトの規則を当てる契約（issue #120、統合した #149・#171・#193 を含む）。
//
// 概要だけページ幅の上限を広げ、タイルは伸ばさず左に詰め、プロジェクトのカードは可変列にする。
// サーバーはカードにせず見出しの横の1行にし、実行中の run は独立した節の全幅の表にする。
// カードの一覧は件数に上限を持ち、超えた分を「ほか M 件」で示す。
// 描画結果の HTML と globals.css を観測する。実際の幅と重なりは e2e（overview-layout.spec.ts）で測る。
import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import {
  createDashboardState,
  createDashboardStateWithProjectConnection,
  createHealthyDashboardState,
  NOW,
} from "../support/fixtures.ts";
import { elementWithAttribute, elementsWithClass, hasClassToken, textOf } from "../support/markup.ts";
import { ConnectionState, DashboardState } from "../../src/proto/taktdash.ts";
import { buildOverviewView } from "../../src/state/dashboard.ts";
import OverviewPage from "../../app/page.tsx";

const cssPath = fileURLToPath(new URL("../../app/globals.css", import.meta.url));

const readCss = (): Promise<string> => readFile(cssPath, "utf8");

const renderOverview = (state: DashboardState): string =>
  renderToStaticMarkup(createElement(OverviewPage, { view: buildOverviewView(state, NOW) }));

const normalize = (value: string): string => value.replace(/\s+/g, " ").trim();

// @media の外にある、入れ子のない規則から宣言を読む。@media の中の規則とコメントは対象にしない。
const topLevelCss = (source: string): string => {
  const css = source.replace(/\/\*[\s\S]*?\*\//g, "");
  let depth = 0;
  let result = "";
  let mediaDepth: number | undefined;
  for (let index = 0; index < css.length; index += 1) {
    const char = css[index];
    if (char === "@" && depth === 0 && css.startsWith("@media", index)) {
      mediaDepth = 0;
    }
    if (char === "{") {
      depth += 1;
    } else if (char === "}") {
      depth -= 1;
      if (mediaDepth !== undefined && depth === 0) {
        mediaDepth = undefined;
        continue;
      }
    }
    if (mediaDepth === undefined) {
      result += char;
    }
  }
  return result;
};

const declarationOf = (css: string, selector: string, property: string): string | undefined => {
  const pattern = /([^{}]+)\{([^{}]*)\}/g;
  let match = pattern.exec(css);
  while (match !== null) {
    const selectors = (match[1] ?? "").split(",").map((token) => normalize(token));
    if (selectors.includes(selector)) {
      const value = new RegExp(`(?:^|;)\\s*${property}\\s*:\\s*([^;]+)`, "i").exec(match[2] ?? "")?.[1];
      if (value !== undefined) {
        return normalize(value);
      }
    }
    match = pattern.exec(css);
  }
  return undefined;
};

const requireDeclaration = (css: string, selector: string, property: string): string => {
  const value = declarationOf(css, selector, property);
  if (value === undefined) {
    assert.fail(`${selector} の ${property} が定義されていない`);
  }
  return value;
};

// var(--name) を :root の値へ解決する。直書きの値はそのまま返す。
const resolveVariable = (css: string, value: string): string => {
  const variable = /^var\((--[\w-]+)\)$/.exec(value)?.[1];
  if (variable === undefined) {
    return value;
  }
  return requireDeclaration(css, ":root", variable);
};

const pixels = (value: string): number => {
  const match = /^(\d+(?:\.\d+)?)px$/.exec(value);
  if (match === null) {
    assert.fail(`${value} は px の長さではない`);
  }
  return Number(match[1]);
};

const runsSection = (html: string): string => elementWithAttribute(html, "aria-labelledby", "h-runs").outer;

const tableHeaders = (section: string): string[] => {
  const thead = /<thead\b[^>]*>([\s\S]*?)<\/thead>/.exec(section)?.[1];
  if (thead === undefined) {
    assert.fail("実行中の run の表に thead が無い");
  }
  return Array.from(thead.matchAll(/<th\b[^>]*>([\s\S]*?)<\/th>/g), (match) => normalize(textOf(match[1] ?? "")));
};

const runRows = (section: string): string[] =>
  elementsWithClass(section, "run-row")
    .filter((element) => element.name === "tr")
    .map((element) => element.outer);

const cells = (row: string): string[] => Array.from(row.matchAll(/<td\b[^>]*>[\s\S]*?<\/td>/g), (match) => match[0]);

const cellFor = (section: string, row: string, header: string): string => {
  const index = tableHeaders(section).indexOf(header);
  assert.ok(index >= 0, `表に「${header}」の列がある`);
  const cell = cells(row)[index];
  if (cell === undefined) {
    assert.fail(`run の行に「${header}」の列のセルが無い`);
  }
  return cell;
};

const projectsSection = (html: string): string => elementWithAttribute(html, "aria-labelledby", "h-projects").outer;

const projectCardCount = (section: string): number => (section.match(/href="\/projects#project-/g) ?? []).length;

const withProjects = (count: number): DashboardState => {
  const base = createHealthyDashboardState();
  const template = base.projects[0]!;
  return DashboardState.create({
    ...base,
    projects: Array.from({ length: count }, (_, index) => ({
      ...template,
      name: `project-${String(index + 1).padStart(2, "0")}`,
      about: `確認用のプロジェクト ${index + 1}`,
    })),
  });
};

test("概要だけページ幅の上限を :root の変数で広げ、共有の .page は 1200px のまま残す", async () => {
  const css = topLevelCss(await readCss());

  assert.equal(requireDeclaration(css, ".page", "max-width"), "1200px", "全ページが共有する .page の上限は変えない");

  const overview = requireDeclaration(css, ".overview", "max-width");
  assert.match(overview, /^var\(--[\w-]+\)$/, "概要の上限は :root の変数で持つ");
  assert.ok(pixels(resolveVariable(css, overview)) > 1200, "概要の上限を共有の 1200px より広げる");
});

test("数字1個のタイルは minmax(160px, 220px) の可変列で左に詰め、1fr で伸ばさない", async () => {
  const css = topLevelCss(await readCss());

  const tiles = requireDeclaration(css, ".tiles", "grid-template-columns");
  assert.match(tiles, /^repeat\(auto-(?:fit|fill), minmax\(160px, 220px\)\)$/, "列数を固定せず、各タイルの幅を 220px までにする");
});

test("概要のプロジェクトのカードは auto-fit と minmax の可変列にし、共有の .cards の2列は変えない", async () => {
  const css = topLevelCss(await readCss());

  const columns = requireDeclaration(css, ".overview-cards", "grid-template-columns");
  assert.match(columns, /^repeat\(auto-fit, minmax\(.+, 1fr\)\)$/, "1件なら全幅、2件なら半分ずつに広がる可変列にする");
  assert.equal(
    requireDeclaration(css, ".cards", "grid-template-columns"),
    "repeat(2, minmax(0, 1fr))",
    "ゴール一覧とプロジェクトのページが使う .cards は変えない",
  );

  const html = renderOverview(createDashboardState());
  const list = elementsWithClass(projectsSection(html), "overview-cards")[0];
  assert.ok(list !== undefined, "概要のプロジェクト一覧は概要専用のカードの列を使う");
  assert.equal(list.name, "ul", "プロジェクトのカードは一覧の要素に並べる");
});

test("サーバーは見出しの節の中の1行で、表示名・サーバー ID・機種・最終読み取り時刻・状態を出す", () => {
  const html = renderOverview(createHealthyDashboardState());
  const section = elementWithAttribute(html, "aria-labelledby", "h-servers").outer;
  const line = elementWithAttribute(section, "id", "server-mini");
  const text = normalize(textOf(line.outer));

  assert.match(text, /表示名: Mac mini/, "表示名を出す");
  assert.match(text, /サーバー ID: mini/, "サーバー ID を出す");
  assert.match(text, /機種: Mac mini/, "機種を出す");
  assert.match(text, /takt-mcp から 16:29 に読みました。/, "最終読み取り時刻を出す");
  assert.ok(
    elementsWithClass(line.outer, "big-badge").some(
      (badge) => hasClassToken(badge.openingTag, "live") && normalize(textOf(badge.outer)) === "動作中",
    ),
    "状態を動作中のバッジで出す",
  );
});

test("サーバーの1行はサーバー別の集計と run を持たず、run は実行中の run の節にだけ出す", () => {
  const html = renderOverview(createDashboardState());
  const line = normalize(textOf(elementWithAttribute(html, "id", "server-mini").outer));

  for (const aggregate of [/run\s*\d+\s*\/\s*\d+/, /進行中のゴール/, /判断待ち\s*\d+件/, /待機中のタスク/]) {
    assert.doesNotMatch(line, aggregate, `サーバーの1行に ${aggregate.source} の集計を出さない`);
  }
  assert.doesNotMatch(elementWithAttribute(html, "id", "server-mini").outer, /href="\/runs\//, "サーバーの1行に run を入れない");
  assert.match(runsSection(html), /href="\/runs\/run-aaa"/, "run は実行中の run の節に出す");
});

test("実行中の run は見出し付きの独立した節で、全幅の表にプロジェクト・ゴール・ステップ・経過・状態の列を持つ", () => {
  const html = renderOverview(createDashboardState());
  const heading = elementWithAttribute(html, "id", "h-runs");
  assert.equal(heading.name, "h2", "実行中の run の見出しは h2");
  assert.equal(normalize(textOf(heading.outer)), "実行中の run");

  const section = runsSection(html);
  const table = elementsWithClass(section, "l-table")[0];
  assert.ok(table !== undefined, "表は部品の .l-table で組む");
  assert.match(table.outer, /<table\b/, ".l-table の中に table がある");
  assert.deepEqual(tableHeaders(section), ["プロジェクト", "ゴール", "ステップ", "経過", "状態"]);
});

test("実行中の run の表は run 1件を1行にし、先頭の列のプロジェクト名から run の詳細へ移動できる", () => {
  const base = createDashboardState();
  const other = { ...base.runs[0]!, runSlug: "run-bbb", sessionId: "session-bbb", project: "esa", goalId: "goal-2" };
  const html = renderOverview(DashboardState.create({ ...base, runs: [...base.runs, other] }));
  const section = runsSection(html);
  const rows = runRows(section);

  assert.equal(rows.length, 2, "run 2件を2行にする");
  for (const [row, project, slug] of [
    [rows[0]!, "corporate-site", "run-aaa"],
    [rows[1]!, "esa", "run-bbb"],
  ] as const) {
    const first = cells(row)[0];
    assert.ok(first !== undefined, "行に先頭のセルがある");
    assert.equal(tableHeaders(section)[0], "プロジェクト", "先頭の列はプロジェクト");
    assert.match(normalize(textOf(first)), new RegExp(project), "先頭のセルにプロジェクト名を出す");
    assert.match(first, new RegExp(`href="/runs/${slug}"`), "先頭のセルから run の詳細へ移動できる");
  }
  assert.match(normalize(textOf(cellFor(section, rows[1]!, "ゴール"))), /v2\.4\.0 を公開する/, "ゴールの列にその run のゴール名を出す");
});

test("run の行はゴール・ステップ・経過・状態をそれぞれの列のセルに分けて出す", () => {
  const html = renderOverview(createDashboardState());
  const section = runsSection(html);
  const row = runRows(section)[0];
  assert.ok(row !== undefined, "run の行がある");

  const goal = normalize(textOf(cellFor(section, row, "ゴール")));
  const step = cellFor(section, row, "ステップ");
  const elapsed = normalize(textOf(cellFor(section, row, "経過")));
  const status = cellFor(section, row, "状態");

  assert.match(goal, /採用ページのレイアウトを直す/, "ゴールの列にゴール名を出す");
  assert.match(normalize(textOf(row)), /タスク「レイアウトを直しています。」/, "run の行にタスクの要約を出す");
  assert.match(step, /ステップ\s*<b>implement<\/b>/, "ステップの列に今のステップ名を出す");
  assert.match(normalize(textOf(step)), /2\/4/, "ステップの列に背骨の位置を出す");
  assert.doesNotMatch(goal, /implement|ステップ中/, "ゴールの列にステップの表示を混ぜない");
  assert.match(elapsed, /最後のログは 1分前/, "経過の列に最後のログからの経過を出す");
  assert.match(status, /class="st live"/, "状態の列に状態のラベルを出す");
  assert.equal(normalize(textOf(status)), "実行中", "状態の列は状態のラベルだけを出す");
});

// 2026-11-16 の本番で実際に表示された、256 バイトで切れた preview（issue #153）。
// test/state/goal-summary.test.ts の TRUNCATED_PREVIEW と同じ文字列。
const TRUNCATED_PREVIEW =
  "taktDash の関門と道具の穴4件を解消する。対象は issue #132（https://github.com/j5ik2o/takt-dash/issues/132 、proto-check が index 依存で、proto を変更するタスクは machine_gate を通過できない）、issue #98（https://";

// main の #153 は旧サーバーカードの run 行で、切り詰められた preview のゴール名に省略記号を付けていた。
// 新しい run の表でもゴールの列がこの表示を保つことを、描画した表のセルで確かめる。
test("run の表のゴールの列は、切り詰められた preview のゴール名に省略記号を付け、完結した名前には付けない", () => {
  const base = createDashboardState();
  const truncated = DashboardState.create({
    ...base,
    goals: base.goals.map((goal) => (goal.goalId === "goal-1" ? { ...goal, name: TRUNCATED_PREVIEW, nameTruncated: true } : goal)),
  });
  const truncatedSection = runsSection(renderOverview(truncated));
  const truncatedRow = runRows(truncatedSection)[0];
  assert.ok(truncatedRow !== undefined, "run の行がある");
  const truncatedGoal = textOf(cellFor(truncatedSection, truncatedRow, "ゴール"));
  assert.ok(truncatedGoal.includes(`${TRUNCATED_PREVIEW}…`), "切り詰められた preview のゴール名に省略記号を付けて出す");

  const completeSection = runsSection(renderOverview(base));
  const completeRow = runRows(completeSection)[0];
  assert.ok(completeRow !== undefined, "run の行がある");
  const completeGoal = textOf(cellFor(completeSection, completeRow, "ゴール"));
  assert.ok(completeGoal.includes("採用ページのレイアウトを直す"), "完結したゴール名を出す");
  assert.ok(!completeGoal.includes("…"), "完結したゴール名には省略記号を付けない");
});

// ゴール側のこのテストは、読み取り遅延（DELAYED）の run が状態「実行中（読み取り遅延）」と class="st delayed" で
// 出ることを前提にしていた。main（issue #194）は読み取り遅延だけでは run の表示を変えない形にし、delayed の
// 状態を撤去した。遅延中でも状態のラベルが状態の列に入り、ゴールの列へ混ざらないことを、main の表示で確かめる。
test("読み取り遅延の run は状態のラベルをゴールの列と別のセルに出す", () => {
  const state = createDashboardStateWithProjectConnection(
    "corporate-site",
    ConnectionState.CONNECTION_STATE_DELAYED,
    new Date(NOW.getTime() - 300 * 1_000),
  );
  const html = renderOverview(state);
  const section = runsSection(html);
  const row = runRows(section)[0];
  assert.ok(row !== undefined, "run の行がある");

  const status = cellFor(section, row, "状態");
  const goal = cellFor(section, row, "ゴール");
  assert.match(status, /class="st live"/, "読み取り遅延だけでは状態を変えず、状態の列に実行中のラベルを出す");
  assert.equal(normalize(textOf(status)), "実行中", "状態の列は状態のラベルだけを出す");
  assert.doesNotMatch(normalize(textOf(goal)), /実行中|読み取り遅延/, "ゴールの列に状態のラベルを混ぜない");
  assert.match(normalize(textOf(goal)), /採用ページのレイアウトを直す/, "ゴールの列にはゴール名を出す");
});

test("正常な run の行は警告の見た目と注記を持たず、ログが止まった run の行だけが持つ", () => {
  const quiet = runRows(runsSection(renderOverview(createHealthyDashboardState())))[0];
  assert.ok(quiet !== undefined, "正常な run の行がある");
  assert.equal(elementsWithClass(quiet, "run-note").length, 0, "正常な run には注記を出さない");
  assert.equal(elementsWithClass(quiet, "warn").length, 0, "正常な run に警告の見た目を付けない");

  const tenMinutesLater = new Date(NOW.getTime() + 10 * 60_000);
  const warnHtml = renderToStaticMarkup(
    createElement(OverviewPage, { view: buildOverviewView(createHealthyDashboardState(), tenMinutesLater) }),
  );
  const warn = runRows(runsSection(warnHtml))[0];
  assert.ok(warn !== undefined, "ログが止まった run の行がある");
  assert.ok(elementsWithClass(warn, "run-note").length > 0, "ログが止まった run には注記を出す");
  assert.ok(elementsWithClass(warn, "warn").length > 0, "ログが止まった run は警告の見た目にする");
});

test("run が無いときは実行中の run の節に表を出さず、走っている run が無いと示す", () => {
  const html = renderOverview(DashboardState.create({ ...createDashboardState(), runs: [] }));
  const section = runsSection(html);

  assert.match(normalize(textOf(section)), /走っている run はありません。/);
  assert.doesNotMatch(section, /<table\b/, "空の表を出さない");
});

test("サーバー ID が実行中の run の見出し id と同じでも、サーバーの1行の id と衝突しない", () => {
  const base = createDashboardState();
  const state = DashboardState.create({
    ...base,
    servers: [{ ...base.servers[0]!, serverId: "h-runs" }],
    runs: base.runs.map((run) => ({ ...run, serverId: "h-runs" })),
    projects: base.projects.map((project) => ({ ...project, serverId: "h-runs" })),
  });
  const html = renderOverview(state);

  const line = elementWithAttribute(html, "id", "server-h-runs");
  assert.match(normalize(textOf(line.outer)), /表示名: Mac mini/, "server- 接頭辞の id はサーバーの1行に付く");
  assert.equal(html.split('id="h-runs"').length - 1, 1, "h-runs の id は実行中の run の見出しに1つだけある");
  assert.equal(elementWithAttribute(html, "id", "h-runs").name, "h2");
});

test("プロジェクトのカードは上限の件数までにし、超えた分を /projects への「ほか M 件」で示す", () => {
  const total = 30;
  const html = renderOverview(withProjects(total));
  const section = projectsSection(html);
  const shown = projectCardCount(section);

  assert.ok(shown > 0, "カードを1件以上出す");
  assert.ok(shown < total, "上限を超える件数のカードを全部は並べない");
  const more = /<a\b[^>]*href="\/projects"[^>]*>([\s\S]*?)<\/a>/.exec(section);
  assert.ok(more !== null, "超えた分は /projects へのリンクで示す");
  const hidden = /ほか\s*(\d+)\s*件/.exec(normalize(textOf(more[1] ?? "")));
  assert.ok(hidden !== null, "リンクの文に「ほか M 件」を出す");
  assert.equal(Number(hidden[1]), total - shown, "M は表示しなかったプロジェクトの件数");
});

test("プロジェクトが上限ちょうどのときは全件をカードにし、「ほか」を出さない", () => {
  const limit = projectCardCount(projectsSection(renderOverview(withProjects(30))));
  const section = projectsSection(renderOverview(withProjects(limit)));

  assert.equal(projectCardCount(section), limit, "上限ちょうどなら全件をカードにする");
  assert.doesNotMatch(normalize(textOf(section)), /ほか\s*\d+\s*件/, "隠れたカードが無ければ「ほか」を出さない");
  assert.doesNotMatch(section, /href="\/projects"/, "隠れたカードが無ければ /projects へのリンクを出さない");
});

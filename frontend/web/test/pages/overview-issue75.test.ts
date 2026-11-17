// 概要の上部を作り直す契約（issue #75）。
//
// 「開いた瞬間に全体が分かり、異常だけが目立つ」ため、上部を実行中 run・判断待ち・
// 24時間完了タスクの3タイルにし、閾値と最終読み取り時刻のタイルを外す。異常がある
// ときだけ警告バナーと判断待ちパネルを出し、サーバーカードを先に置く。描画結果の
// HTML から要素を観測し、ブラウザ自動化は使わない。
import test from "node:test";
import assert from "node:assert/strict";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { createDashboardState, createHealthyDashboardState, NOW } from "../support/fixtures.ts";
import { type DashboardState, ConnectionState, ExecutionStatus } from "../../src/proto/taktdash.ts";
import { buildOverviewView } from "../../src/state/dashboard.ts";
import OverviewPage from "../../app/page.tsx";

const occurrences = (html: string, needle: string): number => html.split(needle).length - 1;

const renderOverview = (state: DashboardState): string =>
  renderToStaticMarkup(createElement(OverviewPage, { view: buildOverviewView(state, NOW) }));

const tilesSection = (html: string): string => {
  const start = html.indexOf('class="tiles');
  if (start < 0) {
    assert.fail("概要のタイル区画が描画されていない");
  }
  const end = html.indexOf("</section>", start);
  return html.slice(start, end < 0 ? html.length : end);
};

const tileBlocks = (html: string): string[] => {
  const section = tilesSection(html);
  const starts = Array.from(
    section.matchAll(/<div[^>]*class="[^"]*\btile\b[^"]*"[^>]*>/g),
    (match) => match.index ?? 0,
  );
  return starts.map((start, index) => section.slice(start, starts[index + 1] ?? section.length));
};

const tileWithLabel = (html: string, label: string): string => {
  const block = tileBlocks(html).find((candidate) => candidate.includes(label));
  if (block === undefined) {
    assert.fail(`「${label}」のタイルが描画されていない`);
  }
  return block;
};

const healthBlock = (html: string): string | undefined => {
  const start = html.indexOf('class="health');
  if (start < 0) {
    return undefined;
  }
  const tagStart = html.lastIndexOf("<", start);
  const end = html.indexOf("</div>", start);
  return html.slice(tagStart, end < 0 ? html.length : end);
};

const pageHead = (html: string): string => {
  const start = html.indexOf('class="page-head');
  if (start < 0) {
    assert.fail("概要のヘッダーが描画されていない");
  }
  const end = html.indexOf("</header>", start);
  return html.slice(start, end < 0 ? html.length : end);
};

const calloutSection = (html: string): string | undefined => {
  const start = html.indexOf('class="callout');
  if (start < 0) {
    return undefined;
  }
  const end = html.indexOf("</section>", start);
  return html.slice(start, end < 0 ? html.length : end);
};

const serverCard = (html: string, serverId: string): string => {
  const marker = `id="server-${serverId}"`;
  const at = html.indexOf(marker);
  if (at < 0) {
    assert.fail(`サーバー ${serverId} のカードが描画されていない`);
  }
  const start = html.lastIndexOf("<article", at);
  const end = html.indexOf("</article>", start);
  return html.slice(start, end < 0 ? html.length : end);
};

test("概要の上部は実行中 run・判断待ち・24時間完了タスクの3タイルで、閾値と最終読み取り時刻のタイルを持たない", () => {
  const html = renderOverview(createDashboardState());

  assert.equal(tileBlocks(html).length, 3, "タイルは3枚にする");
  assert.ok(tilesSection(html).includes("実行中の run"), "実行中 run のタイルを出す");
  assert.ok(tilesSection(html).includes("判断待ち"), "判断待ちのタイルを出す");
  assert.ok(tilesSection(html).includes("24時間に完了したタスク"), "24時間完了タスクのタイルを出す");
  assert.doesNotMatch(html, /異常判定の閾値/, "閾値のタイルを概要から外す");
  assert.doesNotMatch(html, /最終読み取り時刻/, "最終読み取り時刻のタイルを概要から外す");
});

test("実行中 run のタイルはプロジェクトの concurrency の合計に対する件数と使用率バーを出す", () => {
  const html = renderOverview(createDashboardState());
  const tile = tileWithLabel(html, "実行中の run");

  assert.match(tile, /1\s*<small>\/ 8 件<\/small>/, "1件とプロジェクトの concurrency 合計8件を出す");
  assert.match(tile, /class="[^"]*\bbar\b[^"]*"/, "使用率バーを出す");
  assert.match(tile, /width:13%/, "1 / 8 の使用率をバーの幅に反映する");
});

test("プロジェクトの concurrency の合計が0でも使用率バーを壊さない", () => {
  const base = createDashboardState();
  const state = {
    ...base,
    projects: base.projects.map((project) => ({ ...project, concurrency: 0 })),
  };
  const html = renderOverview(state);
  const tile = tileWithLabel(html, "実行中の run");

  assert.doesNotMatch(html, /NaN|Infinity/, "0除算の結果を表示しない");
  assert.match(tile, /width:0%/, "上限が0のときバーの幅は0にする");
});

test("判断待ちのタイルは件数と最長の待ち時間を警告の見た目で出す", () => {
  const html = renderOverview(createDashboardState());
  const tile = tileWithLabel(html, "判断待ち（全サーバー）");

  assert.match(tile, /class="tile[^"]*\battention\b/, "判断待ちがあるときタイルを注意の見た目にする");
  assert.match(tile, /2\s*<small>件<\/small>/, "判断待ちの件数を出す");
  assert.match(tile, /最も長いものは\s*5時間\s*待っています/, "滞留した最長の待ち時間を出す");
});

test("24時間に完了したタスクのタイルは合計と失敗系の件数を出す", () => {
  const html = renderOverview(createDashboardState());
  const tile = tileWithLabel(html, "24時間に完了したタスク");

  assert.match(tile, /<div class="num">\s*4\s*<small>件<\/small>/, "完了したタスクの合計4件を出す");
  assert.match(tile, /失敗・上限超過・PR 失敗のタスクが\s*1件残っています/, "失敗系の件数を添える");
});

test("異常があるときだけ警告バナーを出し、該当する文だけを並べる", () => {
  const html = renderOverview(createDashboardState());
  const health = healthBlock(html);

  assert.ok(health !== undefined, "異常があるので警告バナーを出す");
  assert.match(health, /class="[^"]*\battention\b/, "注意の見た目にする");
  assert.match(health, /判断待ちが\s*2件あります/, "判断待ちの文を出す");
  assert.match(health, /Mac Studio から応答がありません/, "応答のないサーバーの文を出す");
  assert.match(health, /失敗・上限超過・PR 失敗のタスクが/, "失敗系の内訳を明記した文を出す");
  assert.match(health, /1件/, "失敗系の件数を出す");
});

test("ログが止まっている run だけが異常のときはその文だけを出す", () => {
  const base = createHealthyDashboardState();
  const state = {
    ...base,
    runs: base.runs.map((run) => ({
      ...run,
      logSilent: true,
      lastLogAt: new Date(NOW.getTime() - (10 * 60_000 + 1_000)),
    })),
  };
  const health = healthBlock(renderOverview(state));

  assert.ok(health !== undefined, "無音の run があるので警告バナーを出す");
  assert.match(health, /ログが止まっている run が\s*1件あります/, "無音の run の文を出す");
  assert.doesNotMatch(health, /判断待ちが/, "0件の判断待ちの文を出さない");
  assert.doesNotMatch(health, /から応答がありません/, "応答のないサーバーが無ければその文を出さない");
});

test("中止したゴールだけが異常のときはその文だけを出す", () => {
  const base = createHealthyDashboardState();
  const state = {
    ...base,
    goals: base.goals.map((goal, index) =>
      index === 0 ? { ...goal, executionStatus: ExecutionStatus.EXECUTION_STATUS_ABORTED } : goal,
    ),
  };
  const health = healthBlock(renderOverview(state));

  assert.ok(health !== undefined, "中止したゴールがあるので警告バナーを出す");
  assert.match(health, /中止したゴールが\s*1件あります/, "中止したゴールの文を出す");
  assert.doesNotMatch(health, /判断待ちが/, "0件の判断待ちの文を出さない");
  assert.doesNotMatch(health, /から応答がありません/, "応答のない対象が無ければその文を出さない");
  assert.doesNotMatch(health, /失敗・上限超過・PR 失敗のタスクが/, "失敗が無ければその文を出さない");
});

test("プロジェクトの応答なしだけが異常のときはそのプロジェクトの文だけを出す", () => {
  const base = createHealthyDashboardState();
  const state = {
    ...base,
    projects: base.projects.map((project) =>
      project.name === "esa"
        ? { ...project, connected: false, connectionState: ConnectionState.CONNECTION_STATE_UNRESPONSIVE }
        : project,
    ),
  };
  const health = healthBlock(renderOverview(state));

  assert.ok(health !== undefined, "応答なしのプロジェクトがあるので警告バナーを出す");
  assert.match(health, /esa から応答がありません/, "応答なしのプロジェクト名を出す");
  assert.doesNotMatch(health, /判断待ちが/, "0件の判断待ちの文を出さない");
  assert.doesNotMatch(health, /中止したゴール/, "中止が無ければその文を出さない");
  assert.doesNotMatch(health, /失敗・上限超過・PR 失敗のタスクが/, "失敗が無ければその文を出さない");
});

test("異常が無いときは警告バナーと判断待ちパネルを描画しない", () => {
  const html = renderOverview(createHealthyDashboardState());

  assert.equal(tileBlocks(html).length, 3, "異常が無くても概要の3タイルは描画する");
  assert.equal(healthBlock(html), undefined, "警告バナーを描画しない");
  assert.equal(calloutSection(html), undefined, "判断待ちパネルを描画しない");
  assert.doesNotMatch(html, /判断待ちが\s*\d+件あります/, "判断待ちの警告文を出さない");
});

test("判断待ちパネルは項目ごとに1行で出し、滞留を強調し、内容を見るリンクを該当項目へ向ける", () => {
  const html = renderOverview(createDashboardState());
  const callout = calloutSection(html);

  assert.ok(callout !== undefined, "判断待ちがあるのでパネルを出す");
  assert.equal((callout.match(/<li\b/g) ?? []).length, 2, "項目ごとに1行（2件）を出す");
  assert.equal(occurrences(callout, "内容を見る"), 2, "各項目に内容を見るリンクを付ける");

  const mergeRow = /<li\b[\s\S]*?<\/li>/g.exec(callout)?.[0];
  assert.ok(mergeRow !== undefined, "判断待ちの行が描画されている");
  const staleRow = (callout.match(/<li\b[\s\S]*?<\/li>/g) ?? []).find((row) =>
    row.includes("/decisions#decision-merge:mini:corporate-site:goal-1"),
  );
  assert.ok(staleRow !== undefined, "滞留したマージ待ちの行がある");
  assert.match(staleRow, /class="[^"]*\bstale\b/, "滞留した項目を強調する");

  assert.match(callout, /href="\/decisions#decision-inquiry:mini:esa:goal-2:inquiry-schedule"/, "問い合わせの項目へ移動できる");
  assert.match(callout, /href="\/decisions#decision-merge:mini:corporate-site:goal-1"/, "マージ待ちの項目へ移動できる");
});

test("サーバーの区画はプロジェクトの区画より先に現れる", () => {
  const html = renderOverview(createDashboardState());
  const servers = html.indexOf('id="h-servers"');
  const projects = html.indexOf('id="h-projects"');

  assert.ok(servers >= 0, "サーバーの見出しがある");
  assert.ok(projects >= 0, "プロジェクトの見出しがある");
  assert.ok(servers < projects, "サーバーを最初の1画面に置く");
});

test("サーバーカードの下端は run 件数・進行中ゴール・判断待ち・待機中タスクを出す", () => {
  const html = renderOverview(createDashboardState());
  const mini = serverCard(html, "mini");
  const down = serverCard(html, "down");

  assert.match(mini, /run\s*1\s*\/\s*8/, "実行中 run の件数と、配下のプロジェクトの上限の合計を出す");
  assert.match(mini, /進行中のゴール\s*1件/, "進行中のゴール数を出す");
  assert.match(mini, /判断待ち\s*2件/, "そのサーバーの判断待ち数を出す");
  assert.match(mini, /待機中のタスク\s*1件/, "待機中のタスク数を出す");

  assert.match(down, /run\s*0\s*\/\s*0/, "プロジェクトが無いサーバーは上限0で run 0件として出す");
});

test("応答なしのときは最後に読めた時点の件数であることと時刻を示す", () => {
  const base = createDashboardState();
  const state = {
    ...base,
    runs: base.runs.map((run) => ({ ...run, serverId: "down" })),
  };
  const html = renderOverview(state);
  const head = pageHead(html);
  const tile = tileWithLabel(html, "実行中の run");

  assert.match(head, /最後に読めた時点で\s*1\s*件/, "ヘッダーで件数を現在値として断定せず、読めた時点の値と示す");
  assert.match(head, /16:09/, "ヘッダーに最後に読めた時刻を添える");
  assert.match(tile, /最後に読めた時点で\s*1\s*件/, "タイルでも読めた時点の値と示す");
  assert.match(tile, /16:09/, "タイルにも最後に読めた時刻を添える");

  const down = serverCard(html, "down");
  assert.match(down, /class="[^"]*\bstale-note\b/, "応答なしのカードに時点の注記を出す");
  assert.match(down, /16:09/, "カードにも最後に読めた時刻を出す");
  assert.match(down, /時点/, "カードの内容が読めた時点のものであると示す");
});

test("最後に読めた時刻が1件も無いときは不明と表示し、時刻を捏造しない", () => {
  const base = createDashboardState();
  const state = {
    ...base,
    servers: base.servers.map((server) =>
      server.serverId === "down" ? { ...server, lastReadAt: undefined } : server,
    ),
  };
  const html = renderOverview(state);
  const head = pageHead(html);
  const tile = tileWithLabel(html, "実行中の run");

  assert.match(head, /最後に読めた時刻は不明/, "ヘッダーで時刻を不明と示す");
  assert.doesNotMatch(head, /\d{2}:\d{2}/, "ヘッダーに実在しない時刻を出さない");
  assert.match(tile, /最後に読めた時刻は不明/, "タイルで時刻を不明と示す");
  assert.doesNotMatch(tile, /\d{2}:\d{2}/, "タイルに実在しない時刻を出さない");

  const down = serverCard(html, "down");
  assert.match(down, /まだ一度も読めていません/, "カードはまだ読めていないと示す");
  assert.doesNotMatch(down, /最後に読めた時点の状態/, "読めていないのに時点の状態と併記しない");
});

test("プロジェクトだけが応答なしのときはヘッダーとタイルがサーバーを主語にしない", () => {
  const base = createHealthyDashboardState();
  const state = {
    ...base,
    projects: base.projects.map((project) =>
      project.name === "esa"
        ? { ...project, connected: false, connectionState: ConnectionState.CONNECTION_STATE_UNRESPONSIVE }
        : project,
    ),
  };
  const html = renderOverview(state);
  const head = pageHead(html);
  const tile = tileWithLabel(html, "実行中の run");

  assert.match(head, /応答のないプロジェクトがあります/, "ヘッダーの主語をプロジェクトにする");
  assert.doesNotMatch(head, /応答のないサーバー/, "サーバーが応答なしと主張しない");
  assert.match(tile, /応答のないプロジェクトがあるため/, "タイルの主語をプロジェクトにする");
  assert.doesNotMatch(tile, /応答のないサーバー/, "タイルでサーバーが応答なしと主張しない");
});

test("サーバーが応答なしのときはヘッダーとタイルがサーバーを主語にする", () => {
  const html = renderOverview(createDashboardState());
  const head = pageHead(html);
  const tile = tileWithLabel(html, "実行中の run");

  assert.match(head, /応答のないサーバーがあります/, "サーバー応答なしの現行文言を維持する");
  assert.match(tile, /応答のないサーバーがあるため/, "タイルもサーバーを主語にする");
});

test("失敗系の件数は失敗・上限超過・PR 失敗の合計を内訳の文言でタイルに出す", () => {
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
  const html = renderOverview(state);
  const tile = tileWithLabel(html, "24時間に完了したタスク");

  assert.match(tile, /失敗・上限超過・PR 失敗のタスクが\s*6件残っています/, "failed 1 + exceeded 2 + prFailed 3 を合計する");
});

test("トークン消費のタイルは描画しない", () => {
  const html = renderOverview(createDashboardState());

  assert.equal(tileBlocks(html).length, 3, "概要の3タイルは描画されている");
  assert.doesNotMatch(html, /トークン/, "値を推測したトークン消費のタイルを出さない");
});

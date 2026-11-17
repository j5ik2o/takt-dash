// 見た目の契約（AC77-1 / AC77-3 / AC77-4）。
//
// モック（docs/taktdash.html）の :root トークン、状態ごとのクラスと色、幅の切り替え、
// run 行の列、識別子・時刻の等幅書体を観測する。描画結果の HTML と globals.css を
// 直接読み、ブラウザ自動化は使わない（実測は未実施として報告する）。
import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { createElement, type ComponentType, type ReactElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import {
  createActivityResponse,
  createDashboardState,
  createProjectsResponse,
  createRunDetailResponse,
  NOW,
} from "../support/fixtures.ts";
import { ActivityKind, DashboardState, ExecutionStatus, GoalStatus, type GetActivityResponse } from "../../src/proto/taktdash.ts";
import { buildOverviewView } from "../../src/state/dashboard.ts";
import { RunDetailContent } from "../../src/components/run-detail-content.tsx";
import { Sidebar } from "../../src/components/sidebar.tsx";
import ActivityPage from "../../app/activity/page.tsx";
import OverviewPage from "../../app/page.tsx";
import ProjectsPage from "../../app/projects/page.tsx";

const cssPath = fileURLToPath(new URL("../../app/globals.css", import.meta.url));

const readCss = (): Promise<string> => readFile(cssPath, "utf8");

const renderPage = (Page: ComponentType<never>, props: unknown): string =>
  renderToStaticMarkup(createElement(Page as ComponentType<never>, props as never) as ReactElement);

const escapeRegExp = (value: string): string => value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

type CssRule = { selectors: string; body: string };

// 入れ子のない通常の規則だけを取り出す。@media の中の規則は、そのブロックを
// 切り出してから同じ関数へ渡す。
// 規則の直前のコメントをセレクタに含めないよう、コメントを除いてから取り出す。
const cssRules = (css: string): CssRule[] => {
  const rules: CssRule[] = [];
  const source = css.replace(/\/\*[\s\S]*?\*\//g, "");
  const pattern = /([^{}]+)\{([^{}]*)\}/g;
  let match = pattern.exec(source);
  while (match !== null) {
    rules.push({ selectors: match[1] ?? "", body: match[2] ?? "" });
    match = pattern.exec(source);
  }
  return rules;
};

const blockAt = (css: string, matchIndex: number): string | undefined => {
  const open = css.indexOf("{", matchIndex);
  if (open < 0) {
    return undefined;
  }
  let depth = 0;
  for (let index = open; index < css.length; index += 1) {
    if (css[index] === "{") {
      depth += 1;
    } else if (css[index] === "}") {
      depth -= 1;
      if (depth === 0) {
        return css.slice(open + 1, index);
      }
    }
  }
  return undefined;
};

const mediaBlockFor = (css: string, width: number): string | undefined => {
  const pattern = new RegExp(`@media[^{]*max-width:\\s*${width}px[^{]*\\{`, "i");
  const match = pattern.exec(css);
  return match === null ? undefined : blockAt(css, match.index);
};

const mediaBlockMatching = (css: string, feature: RegExp): string | undefined => {
  const pattern = new RegExp(`@media[^{]*${feature.source}[^{]*\\{`, "i");
  const match = pattern.exec(css);
  return match === null ? undefined : blockAt(css, match.index);
};

const declaration = (body: string, property: string): string | undefined => {
  const match = new RegExp(`(?:^|;)\\s*${escapeRegExp(property)}\\s*:\\s*([^;]+)`, "i").exec(body);
  return match?.[1]?.trim();
};

const declarationMatching = (css: string, selector: RegExp, property: string): string | undefined => {
  for (const rule of cssRules(css)) {
    const matched = rule.selectors.split(",").some((token) => selector.test(token.trim()));
    if (!matched) {
      continue;
    }
    const value = declaration(rule.body, property);
    if (value !== undefined) {
      return value;
    }
  }
  return undefined;
};

const requireDeclaration = (css: string, selector: RegExp, property: string, label: string): string => {
  const value = declarationMatching(css, selector, property);
  if (value === undefined) {
    assert.fail(`${label} の ${property} が定義されていない`);
  }
  return value;
};

const selectorExists = (css: string, selector: RegExp): boolean =>
  cssRules(css).some((rule) => rule.selectors.split(",").some((token) => selector.test(token.trim())));

const goalBadgeElements = (html: string): string[] =>
  html.match(/<span\b[^>]*class="[^"]*\bgoal-badge\b[^"]*"[^>]*>[\s\S]*?<\/span>/g) ?? [];

const goalBadgeFor = (html: string, label: string): string => {
  const block = goalBadgeElements(html).find(
    (candidate) => candidate.replace(/<[^>]+>/g, "").trim() === label,
  );
  if (block === undefined) {
    assert.fail(`ゴールのバッジに ${label} がある`);
  }
  return block;
};

const fieldHtml = (html: string, label: string): string => {
  const labelHtml = `<div class="k">${label}</div>`;
  const index = html.indexOf(labelHtml);
  if (index < 0) {
    assert.fail(`${label} の項目が描画されていない`);
  }
  const nextLabel = html.indexOf('<div class="k">', index + labelHtml.length);
  return html.slice(index, nextLabel < 0 ? html.length : nextLabel);
};

test(":root はモックの色トークンを1か所に定義し、既存の変数名も残す", async () => {
  const css = await readCss();

  const mockTokens = [
    "--bg",
    "--surface",
    "--line",
    "--line2",
    "--text",
    "--muted",
    "--faint",
    "--teal",
    "--teal-d",
    "--teal-soft",
    "--track",
    "--orange",
    "--orange-d",
    "--orange-soft",
    "--orange-line",
    "--gray-soft",
    "--mono",
  ];
  for (const token of mockTokens) {
    requireDeclaration(css, /^:root$/, token, ":root");
  }

  const legacyTokens = [
    "--background",
    "--foreground",
    "--muted",
    "--line",
    "--surface",
    "--accent",
    "--warn",
    "--warn-bg",
    "--live",
    "--down",
    "--track",
  ];
  for (const token of legacyTokens) {
    requireDeclaration(css, /^:root$/, token, ":root");
  }

  assert.equal(requireDeclaration(css, /^:root$/, "--bg", ":root").toLowerCase(), "#f6f8f8");
  assert.equal(requireDeclaration(css, /^:root$/, "--teal", ":root").toLowerCase(), "#0f8b8d");
  assert.equal(requireDeclaration(css, /^:root$/, "--orange", ":root").toLowerCase(), "#d9661a");
  assert.match(requireDeclaration(css, /^:root$/, "--mono", ":root"), /monospace/);
});

test("状態の色は状態ごとのトークンで定義し、待機中の点を実行中と同じ色にしない", async () => {
  const css = await readCss();

  const stColor = (state: string): string =>
    requireDeclaration(css, new RegExp(`\\.st\\.${state}$`), "color", `.st.${state}`);
  const live = stColor("live");
  const warn = stColor("warn");
  const idle = stColor("idle");
  for (const value of [live, warn, idle]) {
    assert.match(value, /var\(--/, "状態の色はトークンを参照する");
  }
  assert.notEqual(live, warn, "実行中とログ停止は別の色");
  assert.notEqual(live, idle, "実行中と待機は別の色");
  assert.notEqual(warn, idle, "ログ停止と待機は別の色");

  const dotLive = requireDeclaration(css, /^\.dot\.live$/, "background", ".dot.live");
  const dotWarn = requireDeclaration(css, /\.dot\.warn$/, "background", ".dot.warn");
  const dotIdle = requireDeclaration(css, /^\.dot\.idle$/, "background", ".dot.idle");
  assert.doesNotMatch(dotIdle, /var\(--live\)/, "待機中の点を実行中と同じ色にしない");
  assert.notEqual(dotIdle, dotLive, "待機中の点と実行中の点は別の色");
  assert.notEqual(dotIdle, dotWarn, "待機中の点と警告の点は別の色");
});

test("ログ停止の run 行は警告の状態クラスを描画する", () => {
  const tenMinutesLater = new Date(NOW.getTime() + 10 * 60_000);
  const html = renderPage(OverviewPage, {
    view: buildOverviewView(createDashboardState(), tenMinutesLater),
  });

  assert.match(html, /class="st warn"/, "状態ラベルを警告の見た目で出す");
  assert.match(html, /class="dot warn"/, "状態の点を警告の見た目で出す");
});

test("応答なしのサーバーの run 行は待機の状態クラスを描画する", () => {
  const base = createDashboardState();
  const state = DashboardState.create({
    ...base,
    runs: base.runs.map((run) => ({ ...run, serverId: "down" })),
  });
  const html = renderPage(OverviewPage, { view: buildOverviewView(state, NOW) });

  const row = /<tr class="run-row">([\s\S]*?)<\/tr>/.exec(html)?.[1];
  assert.ok(row !== undefined, "run 行が描画されている");
  assert.match(row, /class="st idle"/, "状態ラベルを待機の見た目で出す");
  assert.match(row, /class="dot idle"/, "状態の点を待機の見た目で出す");
});

test("サーバーの状態バッジは状態ごとの色クラスを持つ", async () => {
  const html = renderPage(OverviewPage, { view: buildOverviewView(createDashboardState(), NOW) });
  assert.match(html, /class="big-badge live"/, "動作中のバッジを実行中の見た目で出す");
  assert.match(html, /class="big-badge down"/, "応答なしのバッジを応答なしの見た目で出す");

  const css = await readCss();
  const live = requireDeclaration(css, /\.big-badge\.live$/, "background", ".big-badge.live");
  assert.match(live, /var\(--/, "動作中のバッジはトークンの背景色を持つ");

  const down = requireDeclaration(css, /\.big-badge\.down$/, "color", ".big-badge.down");
  assert.match(down, /var\(--/, "応答なしのバッジはトークンの色を持つ");
});

test("ゴールの見出しは1行に収めてあふれを省略記号にし、行の最小幅を0にする", async () => {
  const css = await readCss();

  assert.equal(requireDeclaration(css, /^\.goal-head$/, "white-space", ".goal-head"), "nowrap");
  assert.equal(requireDeclaration(css, /^\.goal-head$/, "overflow", ".goal-head"), "hidden");
  assert.equal(
    requireDeclaration(css, /^\.goal-head$/, "text-overflow", ".goal-head"),
    "ellipsis",
  );

  const minWidth =
    declarationMatching(css, /^\.goals\s+li$/, "min-width") ??
    declarationMatching(css, /^\.goal$/, "min-width");
  assert.equal(minWidth, "0", "ゴール行の最小幅を0にしてカードの幅に収める");
});

test("PR リンクは本文と区別できるリンク色を持ち、hover で下線を付ける", async () => {
  const css = await readCss();

  assert.equal(
    requireDeclaration(css, /^\.goal-pr\s+a$/, "color", ".goal-pr a"),
    "var(--teal-d)",
    "PR リンクは他のページ内リンクと同じトークン色を持つ",
  );
  assert.equal(
    requireDeclaration(css, /^\.goal-pr\s+a:hover$/, "text-decoration", ".goal-pr a:hover"),
    "underline",
    "PR リンクは hover で下線を付けてリンクと分かる",
  );
});

test("一時停止のゴールは待機の色のバッジを付け、進行中のゴールと区別する", () => {
  const base = createProjectsResponse();
  const state = DashboardState.create({
    ...base.state!,
    goals: base.state!.goals.map((goal) =>
      goal.goalId === "goal-2"
        ? goal
        : {
            ...goal,
            status: GoalStatus.GOAL_STATUS_CREATED,
            executionStatus: ExecutionStatus.EXECUTION_STATUS_ACTIVE,
          },
    ),
  });
  const html = renderPage(ProjectsPage, { response: { ...base, state }, now: NOW });

  const paused = goalBadgeFor(html, "一時停止");
  assert.match(paused, /class="[^"]*\bidle\b[^"]*"/, "一時停止のバッジは待機の色クラスを持つ");

  const active = goalBadgeFor(html, "進行中");
  assert.match(active, /class="[^"]*\blive\b[^"]*"/, "進行中のバッジは実行中の色クラスを持つ");
  assert.doesNotMatch(active, /class="[^"]*\bidle\b[^"]*"/, "進行中を待機の色にしない");
});

test("760px 以下でサイドバーと本文を1列にし、旧 720px の切り替えを残さない", async () => {
  const css = await readCss();

  const block = mediaBlockFor(css, 760);
  if (block === undefined) {
    assert.fail("@media (max-width: 760px) の定義がない");
  }
  const shell = requireDeclaration(block, /^\.shell$/, "grid-template-columns", "760px 以下の .shell");
  assert.match(shell, /^(?:minmax\(0,\s*1fr\)|1fr)$/, "サイドバーと本文を1列にする");

  assert.equal(mediaBlockFor(css, 720), undefined, "旧 720px の切り替えを残さない");
});

test("サイドバーはモックの幅を持ち、タイルはモックの角丸と数字の大きさを持つ", async () => {
  const css = await readCss();

  const shell = requireDeclaration(css, /^\.shell$/, "grid-template-columns", ".shell");
  assert.match(shell, /^252px\s+/, "サイドバーの幅をモックの 252px にする");

  assert.equal(requireDeclaration(css, /^\.tile$/, "border-radius", ".tile"), "12px");
  assert.equal(requireDeclaration(css, /^\.tile\s+\.num$/, "font-size", ".tile .num"), "32px");
});

test("概要はタイルの余白を詰め、警告と判断待ちパネルの見た目を持つ", async () => {
  const css = await readCss();

  assert.ok(selectorExists(css, /^\.overview\s+\.tile$/), "概要のタイルの余白を詰める規則がある");

  assert.ok(selectorExists(css, /^\.health\.attention$/), "警告バナーの見た目がある");
  const attention = requireDeclaration(css, /^\.health\.attention$/, "background", ".health.attention");
  assert.match(attention, /var\(--/, "警告バナーは警告色のトークンを使う");
  assert.ok(selectorExists(css, /^\.tile\.attention$/), "判断待ちのタイルの注意の見た目がある");
  assert.ok(selectorExists(css, /^\.callout$/), "判断待ちパネルの見た目がある");
  assert.ok(selectorExists(css, /^\.side-foot$/), "サイドバー下端の見た目がある");
});

test("判断待ちは橙の左罫線の容器になり、要約は4列で wide が2列を占める", async () => {
  const css = await readCss();

  const tray = requireDeclaration(css, /^\.tray$/, "border-left", ".tray");
  assert.match(tray, /var\(--orange/, "判断待ちの容器は橙の左罫線を持つ");

  const sumGrid = requireDeclaration(css, /^\.sum-grid$/, "grid-template-columns", ".sum-grid");
  assert.match(sumGrid, /^repeat\(4,/, "要約は4列");
  const wide = requireDeclaration(css, /^\.sum-grid\s+\.f\.wide$/, "grid-column", ".sum-grid .f.wide");
  assert.match(wide, /span\s+2/, "wide の項目は2列を占める");

  assert.equal(requireDeclaration(css, /^pre$/, "max-height", "pre"), "360px", "ログの高さを制限する");
});

// var(--name) を :root の値へ解決する。直接の値はそのまま返す。
const resolveRootVariable = (css: string, value: string): string => {
  const reference = /^var\(\s*(--[a-z0-9-]+)\s*\)$/i.exec(value.trim());
  if (reference === null) {
    return value.trim();
  }
  return requireDeclaration(css, /^:root$/, reference[1]!, ":root");
};

test("判断待ちの項目は列の grid をやめ、どの幅でも列の指定を持たない", async () => {
  const css = await readCss();

  for (const rule of cssRules(css)) {
    const trayItemSelectors = rule.selectors
      .split(",")
      .map((token) => token.trim())
      .filter((token) => /^\.tray-item\b/.test(token));
    if (trayItemSelectors.length === 0) {
      continue;
    }
    for (const property of ["grid-template-columns", "grid-column"]) {
      assert.equal(
        declaration(rule.body, property),
        undefined,
        `${trayItemSelectors.join(", ")} に ${property} を残さない（列に分けると内容量で位置がずれる）`,
      );
    }
  }
});

test("判断待ちの見出し行は左の群と右端の待ち時間を両端に分ける", async () => {
  const css = await readCss();

  assert.ok(selectorExists(css, /^\.tray-item\s+\.decision-head$/), "見出し行の規則がある");
  const justify = declarationMatching(css, /^\.tray-item\s+\.decision-head$/, "justify-content");
  const waitMargin = declarationMatching(css, /^\.tray-item\s+\.wait$/, "margin-left");
  assert.ok(
    justify === "space-between" || waitMargin === "auto",
    "待ち時間を見出し行の右端へ寄せる（justify-content: space-between か margin-left: auto）",
  );
});

test("判断待ちの本文は1行の上限を 80ch 前後にし、改行を保持する", async () => {
  const css = await readCss();

  const maxWidth = resolveRootVariable(
    css,
    requireDeclaration(css, /^\.tray-item\s+\.decision-body$/, "max-width", ".tray-item .decision-body"),
  );
  const ch = /^(\d+(?:\.\d+)?)ch$/.exec(maxWidth);
  assert.ok(ch !== null, `本文の上限は ch で指定する: ${maxWidth}`);
  const value = Number(ch[1]);
  assert.ok(value >= 70 && value <= 90, `本文の上限は 80ch 前後: ${maxWidth}`);

  assert.equal(
    requireDeclaration(css, /^\.tray-item\s+\.desc$/, "white-space", ".tray-item .desc"),
    "pre-wrap",
    "本文の改行を保持する",
  );
});

test("判断待ちが0件の空の状態は橙の警告の線を持たず、通常の線を使う", async () => {
  const css = await readCss();

  const rule = cssRules(css).find((candidate) =>
    candidate.selectors.split(",").some((token) => /^\.tray\.tray-empty$/.test(token.trim())),
  );
  if (rule === undefined) {
    assert.fail(".tray.tray-empty の規則が定義されていない");
  }
  const border = declaration(rule.body, "border");
  const borderLeft = declaration(rule.body, "border-left");
  const borderColor = declaration(rule.body, "border-color");
  assert.ok(border !== undefined || borderColor !== undefined, "空の状態は .tray の四辺の線を上書きする");
  for (const value of [border, borderLeft, borderColor]) {
    if (value !== undefined) {
      assert.doesNotMatch(value, /orange/, `空の状態の線に橙を使わない: ${value}`);
    }
  }
  assert.match(border ?? borderColor ?? "", /var\(--line\)/, "空の状態は通常の線の色を使う");
  if (border !== undefined && borderLeft === undefined) {
    assert.doesNotMatch(border, /5px/, "左端だけ太い警告の線にしない");
  }
});

test("時刻と識別子は等幅書体になり、run 詳細の識別子に mono が付く", async () => {
  const css = await readCss();

  assert.match(
    requireDeclaration(css, /^\.ev\s+time$/, "font-family", ".ev time"),
    /var\(--mono\)/,
    "出来事の時刻を等幅にする",
  );
  assert.match(
    requireDeclaration(css, /^\.mono$/, "font-family", ".mono"),
    /var\(--mono\)/,
    "mono クラスは等幅にする",
  );
  assert.match(
    requireDeclaration(css, /\.target$/, "font-family", ".target"),
    /var\(--mono\)/,
    "判断待ちの対象の識別子を等幅にする",
  );

  const detail = renderToStaticMarkup(
    createElement(RunDetailContent, { detail: createRunDetailResponse("run-aaa"), now: NOW }),
  );
  assert.match(fieldHtml(detail, "サーバー"), /class="[^"]*\bmono\b[^"]*"/, "サーバー ID を等幅にする");
  assert.match(fieldHtml(detail, "最後のログ"), /class="[^"]*\bmono\b[^"]*"/, "最後のログの時刻を等幅にする");
  assert.match(fieldHtml(detail, "開始と経過"), /<span\b[^>]*class="[^"]*\bmono\b[^"]*"/, "開始時刻の span を等幅にする");
  assert.match(fieldHtml(detail, "今のステップ"), /<span\b[^>]*class="[^"]*\bmono\b[^"]*"/, "今のステップの span を等幅にする");
  assert.match(
    fieldHtml(detail, "このステップの開始"),
    /<span\b[^>]*class="[^"]*\bmono\b[^"]*"/,
    "このステップの開始の span を等幅にする",
  );

  const projects = renderPage(ProjectsPage, { response: createProjectsResponse(), now: NOW });
  const runLink = /<a\b[^>]*href="\/runs\/run-aaa"[^>]*>/.exec(projects)?.[0];
  assert.ok(runLink !== undefined, "run のリンクが描画されている");
  assert.match(runLink, /class="[^"]*\bmono\b[^"]*"/, "run の識別子を等幅にする");
});

test("ゴールのバッジの色クラスは状態ごとのトークンで描画色を変える", async () => {
  const css = await readCss();

  const colorOf = (tone: string): string =>
    requireDeclaration(css, new RegExp(`^\\.goal-badge\\.${tone}$`), "color", `.goal-badge.${tone}`);
  const live = colorOf("live");
  const idle = colorOf("idle");
  const warn = colorOf("warn");
  for (const value of [live, idle, warn]) {
    assert.match(value, /var\(--/, "ゴールのバッジの色はトークンを参照する");
  }
  assert.notEqual(live, idle, "進行中と一時停止は別の色");
  assert.notEqual(live, warn, "進行中と中止は別の色");
  assert.notEqual(idle, warn, "一時停止と中止は別の色");
  assert.equal(
    requireDeclaration(css, /^\.goal-badge\.warn$/, "font-weight", ".goal-badge.warn"),
    "600",
  );

  const base = createProjectsResponse();
  const state = DashboardState.create({
    ...base.state!,
    goals: base.state!.goals.map((goal) => ({
      ...goal,
      executionStatus: ExecutionStatus.EXECUTION_STATUS_ABORTED,
    })),
  });
  const html = renderPage(ProjectsPage, { response: { ...base, state }, now: NOW });
  const aborted = goalBadgeFor(html, "中止");
  assert.match(aborted, /class="[^"]*\bwarn\b[^"]*"/, "中止のバッジは警告の色クラスを持つ");
});

test("760px 以下でもサーバーの状態テキストを隠さない", async () => {
  const css = await readCss();
  const block = mediaBlockFor(css, 760);
  if (block === undefined) {
    assert.fail("@media (max-width: 760px) の定義がない");
  }
  assert.notEqual(
    declarationMatching(block, /^\.srv-state$/, "display"),
    "none",
    "760px 以下でサーバーの状態を消さない",
  );

  const html = renderToStaticMarkup(
    createElement(Sidebar, {
      servers: createDashboardState().servers,
      pendingCount: 0,
      frontendBuild: { commit: "a1b2c3d", branch: "takt/frontend" },
      backendBuild: { commit: "a1b2c3d", branch: "takt/backend" },
    }),
  );
  assert.match(html, /class="srv-state">動作中</, "動作中の状態テキストが DOM に残る");
  assert.match(html, /class="srv-state">応答なし</, "応答なしの状態テキストが DOM に残る");
});

test("prefers-reduced-motion では全画面の実行中の点を静止させる", async () => {
  const css = await readCss();
  const block = mediaBlockMatching(css, /prefers-reduced-motion\s*:\s*reduce/);
  if (block === undefined) {
    assert.fail("@media (prefers-reduced-motion: reduce) の定義がない");
  }
  const rule = cssRules(block).find((candidate) =>
    candidate.selectors.split(",").some((token) => token.trim() === ".dot.live"),
  );
  if (rule === undefined) {
    assert.fail(".dot.live を reduced motion の対象にしていない");
  }
  assert.match(rule.body, /animation:\s*none/, "reduced motion では実行中の点を止める");

  const selectors = cssRules(block).flatMap((candidate) =>
    candidate.selectors.split(",").map((token) => token.trim()),
  );
  for (const expected of [".run-row .dot.live", ".run-row .bar.live > i", ".run-row.flash"]) {
    assert.ok(selectors.includes(expected), `${expected} を reduced motion の対象に保つ`);
  }
});

test("アクティビティの出来事行の点は静止させる", async () => {
  const css = await readCss();
  const animation = requireDeclaration(css, /^\.ev\s+\.kl\s+\.dot$/, "animation", ".ev .kl .dot");
  assert.equal(animation.toLowerCase(), "none", "出来事行の点はアニメーションしない");
});

const activityRowFor = (kind: ActivityKind): string => {
  const base = createActivityResponse();
  const response: GetActivityResponse = {
    ...base,
    events: [
      {
        occurredAt: new Date(NOW.getTime() - 5 * 60_000),
        serverId: "mini",
        project: "esa",
        kind,
        body: "確認用の出来事です。",
      },
    ],
  };
  const html = renderPage(ActivityPage, { response, now: NOW });
  const row = /<li class="ev[\s\S]*?<\/li>/.exec(html)?.[0];
  if (row === undefined) {
    assert.fail(`kind ${kind} の出来事行が描画されていない`);
  }
  return row;
};

test("アクティビティの出来事行は種類ごとの状態を点とラベルへ描画する", () => {
  const live = activityRowFor(ActivityKind.ACTIVITY_KIND_RUN_STARTED);
  assert.match(live, /class="dot live"/, "run の開始は実行中の点にする");
  assert.doesNotMatch(live, /class="kl warn"/, "run の開始は警告のラベルにしない");
  assert.doesNotMatch(live, /class="ev warn"/, "run の開始は注意の行にしない");

  const warn = activityRowFor(ActivityKind.ACTIVITY_KIND_INQUIRY);
  assert.match(warn, /class="ev warn"/, "問い合わせは注意の行にする");
  assert.match(warn, /class="kl warn"/, "問い合わせは警告のラベルにする");
  assert.match(warn, /class="dot warn"/, "問い合わせは警告の点にする");

  const idle = activityRowFor(ActivityKind.ACTIVITY_KIND_PAUSED);
  assert.match(idle, /class="dot idle"/, "一時停止は待機の点にする");
  assert.doesNotMatch(idle, /class="kl warn"/, "一時停止は警告のラベルにしない");
  assert.doesNotMatch(idle, /class="ev warn"/, "一時停止は注意の行にしない");
});

test("接続の切断から復帰をまとめた行は最新の復帰の状態に揃える", () => {
  const base = createActivityResponse();
  const response: GetActivityResponse = {
    ...base,
    events: [
      {
        occurredAt: new Date(NOW.getTime() - 5 * 60_000),
        serverId: "mini",
        project: "esa",
        kind: ActivityKind.ACTIVITY_KIND_CONNECTION_RESTORED,
        body: "接続が復帰しました。",
      },
      {
        occurredAt: new Date(NOW.getTime() - 10 * 60_000),
        serverId: "mini",
        project: "esa",
        kind: ActivityKind.ACTIVITY_KIND_CONNECTION_LOST,
        body: "接続が切れました。",
      },
    ],
  };
  const html = renderPage(ActivityPage, { response, now: NOW });
  const rows = html.match(/<li class="ev[\s\S]*?<\/li>/g) ?? [];
  assert.equal(rows.length, 1, "隣接する接続イベントは1行にまとまる");
  const row = rows[0]!;
  assert.doesNotMatch(row, /class="ev warn"/, "最新が復帰なら注意の行にしない");
  assert.doesNotMatch(row, /class="kl warn"/, "最新が復帰なら警告のラベルにしない");
  assert.match(row, /class="dot live"/, "最新が復帰なら実行中の点にする");
});

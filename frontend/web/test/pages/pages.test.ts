// ページの描画の契約（C4/C5/C6/C7/C8/C9/C10/C12）。
//
// ダミー gRPC サーバーから取得した proto のメッセージ型を、サーバー側の
// 読み込み処理（load*）で表示用の形へ変換し、各ページが描画に使う。
// 描画結果は HTML 文字列として観測し、モックに沿う要素が現れることを確かめる。
import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { createElement, type ComponentType, type ReactElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import {
  createActionableDashboardState,
  createActivityResponse,
  createDashboardState,
  createDashboardStateWithProjectConnection,
  createNestedStepDashboardState,
  createProjectsResponse,
  NOW,
} from "../support/fixtures.ts";
import { startTaktDashServer } from "../support/grpc-server.ts";
import { elementWithAttribute, elementsWithClass, hasClassToken, textOf } from "../support/markup.ts";
import { createBackendClient } from "../../src/grpc/client.ts";
import { buildOverviewView, type OverviewView } from "../../src/state/dashboard.ts";
import { OverviewContent } from "../../src/components/overview-content.tsx";
import { ActivityKind, AnomalyThresholds, ConnectionState, DashboardState, InquiryKind, PendingDecisionType, TaskStatus } from "../../src/proto/taktdash.ts";
import OverviewPage from "../../app/page.tsx";
import ProjectsPage from "../../app/projects/page.tsx";
import DecisionsPage from "../../app/decisions/page.tsx";
import ActivityPage from "../../app/activity/page.tsx";

const webRoot = fileURLToPath(new URL("../../", import.meta.url));

const renderPage = (Page: ComponentType<never>, props: unknown): string =>
  renderToStaticMarkup(createElement(Page as ComponentType<never>, props as never) as ReactElement);

// 判断待ちを1件だけ持つ状態を作る。本文と選択肢などを表示の検証に必要な値だけ上書きする。
const stateWithDecision = (
  overrides: { body: string; options?: string[]; inquiryId?: string },
): DashboardState => {
  const base = createDashboardState();
  return DashboardState.create({
    ...base,
    pendingDecisions: [
      {
        type: PendingDecisionType.PENDING_DECISION_TYPE_INQUIRY,
        serverId: "mini",
        project: "esa",
        goalId: "goal-2",
        inquiryKind: InquiryKind.INQUIRY_KIND_QUESTION,
        inquiryId: overrides.inquiryId,
        createdAt: new Date(NOW.getTime() - 30 * 60_000),
        body: overrides.body,
        options: overrides.options ?? [],
        stale: false,
      },
    ],
  });
};

// class 属性に指定名を class トークンとして含む要素の開始タグを文書順に取り出す。
// 部分一致（opts と opts-label など）を拾わないよう、空白か引用符で囲まれた完全なトークンで照合する。
const elementsByClass = (html: string, className: string): string[] =>
  html.match(new RegExp(`<[a-z]+[^>]*class="(?:[^"]*\\s)?${className}(?:\\s[^"]*)?"[^>]*>`, "g")) ?? [];

const occurrences = (html: string, needle: string): number => html.split(needle).length - 1;

// 指定 class を持つ最初の要素を、開始タグから対応する終了タグまで取り出す。
// 項目内の要素の並び順に依存せず、その要素の中だけを観測する。
const blockOf = (html: string, className: string): string => {
  const element = elementsWithClass(html, className)[0];
  if (element === undefined) {
    assert.fail(`class="${className}" の要素が描画されていない`);
  }
  return element.outer;
};

// 要素の直下の子要素を文書順に返す。孫要素は含めない。
const childElementsOf = (outer: string): { openingTag: string; outer: string }[] => {
  const innerStart = outer.indexOf(">") + 1;
  const innerEnd = outer.lastIndexOf("</");
  const inner = outer.slice(innerStart, innerEnd);
  const children: { openingTag: string; outer: string }[] = [];
  const tags = /<(\/?)([a-z][a-z0-9]*)\b[^>]*?(\/?)>/gi;
  const voidElements = new Set(["br", "hr", "img", "input", "meta", "link", "wbr"]);
  let depth = 0;
  let start = -1;
  let openingTag = "";
  for (const match of inner.matchAll(tags)) {
    const closing = match[1] === "/";
    const selfClosing = match[3] === "/" || voidElements.has(match[2]!.toLowerCase());
    if (!closing && depth === 0) {
      start = match.index ?? 0;
      openingTag = match[0];
      if (selfClosing) {
        children.push({ openingTag, outer: openingTag });
        continue;
      }
    }
    if (selfClosing && !closing) {
      continue;
    }
    depth += closing ? -1 : 1;
    if (closing && depth === 0) {
      children.push({ openingTag, outer: inner.slice(start, (match.index ?? 0) + match[0].length) });
    }
  }
  return children;
};

// 判断待ちの各項目（li.tray-item）を文書順に返す。選択肢欄の li は含めない。
const decisionItems = (html: string) => elementsWithClass(html, "tray-item");

// 進捗バーの開始タグから閉じタグまでを取り出す。概要の上部には使用率バーも現れるため、
// run 行の中のバーだけを対象にし、バー自身の属性と内側の幅指定を他の要素の一致に
// 紛れずに観測する。
const barMarkup = (html: string): string => {
  const row = /<tr class="run-row[^"]*">([\s\S]*?)<\/tr>/.exec(html);
  if (row === null) {
    assert.fail("run 行が描画されていない");
  }
  const match = /<div[^>]*class="[^"]*\bbar\b[^"]*"[^>]*>[\s\S]*?<\/div>/.exec(row[1]!);
  if (match === null) {
    assert.fail("進捗バーが描画されていない");
  }
  return match[0];
};

// run 行の進捗バー（bar thick）の開始タグを数える。上部のタイルの使用率バー（bar）とは
// 区別し、位置と総数が無い run 行がバーを捏造しないことを観測する。
const thickBars = (html: string): string[] =>
  html.match(/<div[^>]*class="[^"]*\bbar\b[^"]*\bthick\b[^"]*"/g) ?? [];

// 指定したメディアクエリの本体を波括弧の対応で切り出す。
const mediaBlock = (css: string, query: string): string | undefined => {
  const start = css.indexOf(query);
  if (start < 0) {
    return undefined;
  }
  const open = css.indexOf("{", start);
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

const withServerEnv = async <T>(
  address: string,
  run: () => Promise<T>,
): Promise<T> => {
  const previous = process.env.TAKTDASH_BACKEND_ADDR;
  process.env.TAKTDASH_BACKEND_ADDR = address;
  try {
    return await run();
  } finally {
    if (previous === undefined) {
      delete process.env.TAKTDASH_BACKEND_ADDR;
    } else {
      process.env.TAKTDASH_BACKEND_ADDR = previous;
    }
  }
};

test("概要ページは実行中 run と判断待ちと24時間完了タスクのタイルを描画し、閾値と最終読み取り時刻を出さない", async () => {
  const state = createDashboardState();
  const server = await startTaktDashServer({ overview: state });
  try {
    await withServerEnv(server.address, async () => {
      const client = createBackendClient();
      try {
        const view: OverviewView = buildOverviewView(await client.getOverview(), NOW);
        const html = renderPage(OverviewPage, { view });
        assert.match(html, /Mac mini/);
        assert.match(html, /corporate-site/);
        assert.match(html, /採用ページのレイアウトを直す/);
        assert.match(html, /実行中の run/);
        assert.match(html, /判断待ち/);
        assert.match(html, /24時間に完了したタスク/);
        assert.doesNotMatch(html, /異常判定の閾値/, "閾値のタイルを概要から外す");
        assert.doesNotMatch(html, /最終読み取り時刻/, "最終読み取り時刻はサイドバーへ移す");
      } finally {
        await client.close();
      }
    });
  } finally {
    await server.shutdown();
  }
});

test("概要ページは24時間のトークン消費のタイルを描画しない", async () => {
  const view = buildOverviewView(createDashboardState(), NOW);
  const html = renderPage(OverviewPage, { view });
  assert.doesNotMatch(html, /トークン/);
  assert.doesNotMatch(html, /24時間のトークン消費/);
});

test("プロジェクトページはゴール・タスク・run・判断待ちを描画する", async () => {
  const server = await startTaktDashServer({ projects: createProjectsResponse() });
  try {
    await withServerEnv(server.address, async () => {
      const client = createBackendClient();
      try {
        const response = await client.getProjects();
        const html = renderPage(ProjectsPage, { response, now: NOW });
        assert.match(html, /corporate-site/);
        assert.match(html, /採用ページのレイアウトを直す/);
        assert.match(html, /run-aaa/);
        assert.match(html, /判断待ち/);
      } finally {
        await client.close();
      }
    });
  } finally {
    await server.shutdown();
  }
});

test("プロジェクトページは読み取りに失敗したプロジェクトの run を時点付きで描画する", () => {
  const response = createProjectsResponse();
  const state = response.state!;
  const failedState = {
    ...state,
    projects: state.projects.map((project) =>
      project.name === "esa"
        ? {
            ...project,
            connected: false,
            connectionState: ConnectionState.CONNECTION_STATE_UNRESPONSIVE,
            lastReadAt: new Date(NOW.getTime() - 21 * 60_000),
          }
        : project,
    ),
    runs: [...state.runs, { ...state.runs[0]!, runSlug: "run-esa", project: "esa" }],
  };
  const html = renderPage(ProjectsPage, { response: { ...response, state: failedState }, now: NOW });
  assert.match(html, /run-esa/);
  assert.match(html, /実行中（.+時点）/);
});

test("バックエンドが無音と判定した run の警告を概要ページとプロジェクトページに表示する", () => {
  const state = createDashboardState();
  const base = state.runs[0]!;
  const justOverThreshold = new Date(NOW.getTime() - (10 * 60_000 + 1_000));
  const silentState = {
    ...state,
    runs: [{ ...base, logSilent: true, lastLogAt: justOverThreshold }],
  };

  const overviewHtml = renderPage(OverviewPage, { view: buildOverviewView(silentState, NOW) });
  assert.match(overviewHtml, /ログが止まっています/, "概要の run 行に警告を出します");

  const response = createProjectsResponse();
  const projectsHtml = renderPage(ProjectsPage, {
    response: { ...response, state: silentState },
    now: NOW,
  });
  assert.match(projectsHtml, /ログが止まっています/, "プロジェクトの run 行に警告を出します");
});

test("判断待ちページは待ち時間順に描画し、滞留の項目に滞留の表示を付ける", async () => {
  const state = createDashboardState();
  const html = renderPage(DecisionsPage, { state, now: NOW });
  const mergeIndex = html.indexOf("人のマージ待ち");
  const inquiryIndex = html.indexOf("問い合わせ");
  assert.ok(mergeIndex >= 0 && inquiryIndex >= 0);
  assert.ok(mergeIndex < inquiryIndex, "待ち時間の長いマージ待ちが先に現れる");
  assert.match(html, /滞留/);
});

test("判断待ちページの注記に滞留240分とログ無音10分の閾値を残す", () => {
  const html = renderPage(DecisionsPage, { state: createDashboardState(), now: NOW });

  assert.match(html, /240分/, "判断待ちの滞留の閾値を注記に残す");
  assert.match(html, /10分/, "ログの無音の閾値も注記に残す");
});

test("判断待ちページの注記は設定された閾値を反映し、既定値に固定しない", () => {
  const base = createDashboardState();
  const state = DashboardState.create({
    ...base,
    thresholds: AnomalyThresholds.create({ pendingDecisionStaleMinutes: 90, runLogSilentMinutes: 5 }),
  });
  const html = renderPage(DecisionsPage, { state, now: NOW });

  assert.match(html, /90分/, "設定された滞留の閾値を注記に出す");
  assert.match(html, /5分/, "設定されたログ無音の閾値を注記に出す");
  assert.doesNotMatch(html, /240分/, "既定値の240分に固定しない");
  assert.doesNotMatch(html, /10分/, "既定値の10分に固定しない");
});

test("判断待ちページは種別ごとの操作要素を一覧に描画する", () => {
  const html = renderPage(DecisionsPage, { state: createActionableDashboardState(), now: NOW });
  assert.match(html, /<button/i);
  assert.match(html, /<input[^>]*type="text"/);
  assert.match(html, /実施した/);
  assert.match(html, /対応できない/);
  assert.match(html, /マージを承認/);
  assert.match(html, /却下/);
});

test("判断待ちページは本文と選択肢を HTML として解釈せず文字として描画する", () => {
  const html = renderPage(DecisionsPage, {
    state: stateWithDecision({ body: "<script>alert(1)</script>", options: ["<b>今週</b>"] }),
    now: NOW,
  });
  assert.ok(html.includes("&lt;script&gt;alert(1)&lt;/script&gt;"), "本文を HTML として解釈しない");
  assert.ok(!html.includes("<script>"), "script 要素を生成しない");
  assert.ok(html.includes("&lt;b&gt;今週&lt;/b&gt;"), "選択肢を HTML として解釈しない");
  assert.ok(!html.includes("<b>今週</b>"), "選択肢のタグを生成しない");
});

test("判断待ちページは本文の空行を段落の区切りとして描画する", () => {
  const body = "リリースの日取りを決めたい。\n今週は手が空いています。\n\n来週は火曜だけ空いています。\n\nどちらにしますか。";
  const html = renderPage(DecisionsPage, {
    state: stateWithDecision({ body }),
    now: NOW,
  });
  const descBlocks = elementsByClass(html, "desc");
  assert.equal(descBlocks.length, 1, "本文は1つの本文ブロックにまとまる");
  const descHtml = blockOf(html, "desc");
  const paragraphItems = descHtml.match(/<p[^>]*>[\s\S]*?<\/p>/g) ?? [];
  assert.equal(paragraphItems.length, 3, "空行で区切られた3つの段落がそれぞれ別の要素になる");
  assert.match(paragraphItems[0]!, /リリースの日取りを決めたい。\n今週は手が空いています。/);
  assert.match(paragraphItems[1]!, /来週は火曜だけ空いています。/);
  assert.match(paragraphItems[2]!, /どちらにしますか。/);
  assert.equal(
    paragraphItems[0]!.includes("来週は火曜だけ空いています。"),
    false,
    "異なる段落を1つの要素へ連結しない",
  );
});

test("判断待ちページの本文は単一改行を畳まない CSS 指定を持つ", async () => {
  const html = renderPage(DecisionsPage, {
    state: stateWithDecision({ body: "1行目\n2行目" }),
    now: NOW,
  });
  assert.ok(elementsByClass(html, "desc")[0] !== undefined, "本文ブロックが描画される");
  const css = await readFile(`${webRoot}app/globals.css`, "utf8");
  assert.match(
    css,
    /\.tray-item\s+\.desc[^{}]*\{[^}]*white-space:\s*pre-wrap/,
    ".tray-item .desc は改行を保持する",
  );
});

test("判断待ちページは本文の単一改行で段落を分けない", () => {
  const html = renderPage(DecisionsPage, {
    state: stateWithDecision({ body: "1行目\n2行目" }),
    now: NOW,
  });
  const descHtml = blockOf(html, "desc");
  const paragraphs = descHtml.match(/<p[^>]*>[\s\S]*?<\/p>/g) ?? [];
  assert.equal(paragraphs.length, 1, "単一改行では段落を分けない");
  assert.match(paragraphs[0]!, /1行目\n2行目/, "改行は段落内に残る");
});

test("判断待ちページは選択肢を本文の塊とは別の要素に1件ずつ描画する", () => {
  const html = renderPage(DecisionsPage, {
    state: stateWithDecision({ body: "リリースの日取りはどうしますか。", options: ["今週", "来週"] }),
    now: NOW,
  });
  const descBlock = elementsByClass(html, "desc")[0];
  const optsBlocks = elementsByClass(html, "opts");
  assert.ok(descBlock !== undefined, "本文ブロックが描画される");
  assert.equal(optsBlocks.length, 1, "選択肢は1つの選択肢ブロックにまとまる");
  const descIndex = html.indexOf(descBlock);
  const optsIndex = html.indexOf(optsBlocks[0]!);
  assert.ok(descIndex < optsIndex, "選択肢ブロックは本文ブロックより後に現れる");
  const between = html.slice(descIndex, optsIndex);
  assert.doesNotMatch(between, /選択肢/, "選択肢は本文ブロックに混ざらない");

  const optsHtml = blockOf(html, "opts");
  const items = optsHtml.match(/<li[^>]*>[\s\S]*?<\/li>/g) ?? [];
  assert.equal(items.length, 2, "選択肢は1件ずつ別の要素になる");
  assert.match(items[0]!, /今週/);
  assert.match(items[1]!, /来週/);
});

test("判断待ちページは本文中の選択肢の記述を残したまま、選択肢欄とは別に描画する", () => {
  const html = renderPage(DecisionsPage, {
    state: stateWithDecision({ body: "候補は (a) 今週、(b) 来週 です。", options: ["今週", "来週"] }),
    now: NOW,
  });
  const descBlock = elementsByClass(html, "desc")[0];
  const optsBlocks = elementsByClass(html, "opts");
  assert.ok(descBlock !== undefined && optsBlocks.length === 1);
  const descIndex = html.indexOf(descBlock);
  const optsIndex = html.indexOf(optsBlocks[0]!);
  const descHtml = html.slice(descIndex, optsIndex);
  assert.match(descHtml, /\(a\) 今週、\(b\) 来週/, "本文の記述はそのまま残る");
  const optsHtml = blockOf(html, "opts");
  assert.equal(occurrences(descHtml, "今週"), 1, "本文中に選択肢欄の内容が二重に現れない");
  const items = optsHtml.match(/<li[^>]*>[\s\S]*?<\/li>/g) ?? [];
  assert.equal(items.length, 2, "選択肢欄は本文の記述とは別に各選択肢を並べる");
});

test("判断待ちページは選択肢から作る文を表示せず、本文の選択肢の節をそのまま表示する", () => {
  // 本文が選択肢に触れない場合と、本文自身が「選択肢は」で始まる場合の両方で、
  // 本文は残し、構造化された選択肢から作る文だけを出さないことを確かめる。
  const bodies = [
    "候補は (a) 今週、(b) 来週 です。",
    "選択肢は本文に書きます。候補は (a) 今週、(b) 来週 です。",
  ];
  for (const body of bodies) {
    const html = renderPage(DecisionsPage, {
      state: stateWithDecision({ body, options: ["今週", "来週"], inquiryId: "inquiry-options-rendered" }),
      now: NOW,
    });

    const descBlock = elementsByClass(html, "desc")[0];
    assert.ok(descBlock !== undefined, "本文ブロックが描画される");
    const descIndex = html.indexOf(descBlock);
    const descEnd = html.indexOf("</div>", descIndex);
    const descHtml = html.slice(descIndex, descEnd);
    assert.ok(descHtml.includes(body), `本文を切り取らずにそのまま表示する: ${body}`);

    assert.doesNotMatch(
      html,
      /選択肢は\s*今週\s*と\s*来週\s*です/,
      "構造化された選択肢から作る「選択肢は … です。」の文を表示しない",
    );
    const between = html.slice(descEnd, html.indexOf('class="actions"', descEnd));
    assert.doesNotMatch(between, /選択肢は/, `本文ブロックの後に生成文を重ねて表示しない: ${body}`);

    assert.match(html, /<button[^>]*>今週<\/button>/, "1つ目の選択肢の回答ボタンを表示する");
    assert.match(html, /<button[^>]*>来週<\/button>/, "2つ目の選択肢の回答ボタンも表示する");
  }
});

test("判断待ちページの各項目は見出し行・本文・回答ボタンの3つの子だけをこの順に持つ", () => {
  const html = renderPage(DecisionsPage, {
    state: stateWithDecision({
      body: "リリースの日取りはどうしますか。",
      options: ["今週", "来週"],
      inquiryId: "inquiry-structure",
    }),
    now: NOW,
  });
  const items = decisionItems(html);
  assert.equal(items.length, 1, "判断待ちの項目が1件描画される");
  const children = childElementsOf(items[0]!.outer);
  assert.equal(children.length, 3, "項目の直下は見出し行・本文・回答ボタンの3つだけ");
  assert.ok(hasClassToken(children[0]!.openingTag, "decision-head"), "1つ目は見出し行");
  assert.ok(hasClassToken(children[1]!.openingTag, "decision-body"), "2つ目は本文");
  assert.ok(hasClassToken(children[2]!.openingTag, "actions"), "3つ目は回答ボタン");

  const body = children[1]!.outer;
  const descIndex = body.indexOf(blockOf(body, "desc"));
  const optsIndex = body.indexOf(blockOf(body, "opts"));
  assert.ok(descIndex >= 0 && optsIndex > descIndex, "本文の中で、本文の後ろに選択肢欄が来る");
  assert.match(children[2]!.outer, /<button[^>]*>今週<\/button>/, "本文の直後の行に回答ボタンがある");
});

// main の #231 は、仕上げタスクが失敗系で終わったマージ待ちの判断に、止まった理由の注記を出していた。
// T6 の3行の骨格へ注記を置き直したため、本文の行（decision-body）の中で、本文（desc）の後、
// 選択肢欄（opts）の前に出ることを描画で確かめる。backend はマージ待ちに選択肢を付けないため
// （backend/src/store.rs の options: Vec::new()）、入力は fixture のまま選択肢を持たない。
test("判断待ちページは仕上げタスクが失敗したマージ待ちの本文の後に失敗の注記を出し、失敗していない項目には出さない", () => {
  const note = "仕上げタスクが失敗しました。";
  const withMergeTaskLast = (mergeTaskLast: TaskStatus | undefined): DashboardState => {
    const base = createDashboardState();
    return DashboardState.create({
      ...base,
      pendingDecisions: base.pendingDecisions.map((decision) =>
        decision.type === PendingDecisionType.PENDING_DECISION_TYPE_AWAITING_MERGE ? { ...decision, mergeTaskLast } : decision,
      ),
    });
  };
  const mergeItemBody = (state: DashboardState): string => {
    const items = decisionItems(renderPage(DecisionsPage, { state, now: NOW }));
    const item = items.find((candidate) => textOf(candidate.outer).includes("プロジェクト: corporate-site"));
    if (item === undefined) {
      assert.fail("マージ待ちの項目が描画される");
    }
    const body = childElementsOf(item.outer)[1];
    assert.ok(body !== undefined && hasClassToken(body.openingTag, "decision-body"), "2つ目は本文の行");
    return body.outer;
  };

  const failedBody = mergeItemBody(withMergeTaskLast(TaskStatus.TASK_STATUS_FAILED));
  const desc = blockOf(failedBody, "desc");
  const descEnd = failedBody.indexOf(desc) + desc.length;
  const noteIndex = failedBody.indexOf(note);
  assert.ok(noteIndex >= descEnd, "失敗の注記は本文の行の中で、本文の後に出る");
  assert.ok(!desc.includes(note), "失敗の注記を本文の中に混ぜない");
  assert.ok(!failedBody.slice(0, noteIndex).includes('class="opts"'), "失敗の注記より前に選択肢欄を置かない");

  for (const [label, mergeTaskLast] of [
    ["仕上げタスクが完了したマージ待ち", TaskStatus.TASK_STATUS_COMPLETED],
    ["仕上げタスクの状態が無いマージ待ち", undefined],
  ] as const) {
    assert.ok(!mergeItemBody(withMergeTaskLast(mergeTaskLast)).includes(note), `${label}には失敗の注記を出さない`);
  }
});

test("判断待ちページの見出し行は左にプロジェクト・種別・対象、その後ろに待ち時間を持ち、本文や選択肢を含まない", () => {
  const html = renderPage(DecisionsPage, {
    state: stateWithDecision({ body: "リリースの日取りはどうしますか。", options: ["今週", "来週"] }),
    now: NOW,
  });
  const head = childElementsOf(decisionItems(html)[0]!.outer)[0]!;
  assert.ok(hasClassToken(head.openingTag, "decision-head"));
  const groups = childElementsOf(head.outer);
  assert.equal(groups.length, 2, "見出し行は左の群と右の群の2つから成る");
  assert.ok(hasClassToken(groups[0]!.openingTag, "proj"), "左はプロジェクトの群");
  assert.ok(hasClassToken(groups[1]!.openingTag, "wait"), "右端は待ち時間の群");

  const left = textOf(groups[0]!.outer);
  assert.match(left, /プロジェクト: esa/);
  assert.match(left, /種別: 問い合わせ（質問）/);
  assert.match(left, /対象: ゴール goal-2/);
  assert.match(textOf(groups[1]!.outer), /30分 待ち/, "右の群に待ち時間を出す");
  assert.doesNotMatch(textOf(head.outer), /リリースの日取り/, "見出し行に本文を含めない");
  assert.doesNotMatch(textOf(head.outer), /今週/, "見出し行に選択肢を含めない");
});

test("判断待ちページは滞留の印を見出し行の待ち時間の隣に出し、滞留でない項目には出さない", () => {
  const html = renderPage(DecisionsPage, { state: createDashboardState(), now: NOW });
  const items = decisionItems(html);
  assert.equal(items.length, 2);
  const byProject = (project: string) => {
    const item = items.find((candidate) => textOf(candidate.outer).includes(`プロジェクト: ${project}`));
    if (item === undefined) {
      assert.fail(`${project} の項目が描画される`);
    }
    return item;
  };

  const stale = byProject("corporate-site");
  assert.ok(hasClassToken(stale.openingTag, "stale"), "滞留の項目に stale を付ける");
  const staleHead = childElementsOf(stale.outer)[0]!;
  assert.ok(hasClassToken(staleHead.openingTag, "decision-head"));
  const staleWait = blockOf(staleHead.outer, "wait");
  assert.match(textOf(staleWait), /5時間 待ち/);
  assert.match(textOf(staleWait), /滞留しています。/, "滞留の印は見出し行の待ち時間の群にある");
  const staleBody = childElementsOf(stale.outer)[1]!;
  assert.doesNotMatch(textOf(staleBody.outer), /5時間 待ち|滞留しています/, "本文の行に待ち時間や滞留の印を置かない");

  const fresh = byProject("esa");
  assert.ok(!hasClassToken(fresh.openingTag, "stale"));
  assert.doesNotMatch(textOf(fresh.outer), /滞留しています。/, "滞留でない項目に滞留の印を出さない");
});

test("判断待ちページは種別に関係なく、どの項目も回答の行を本文のすぐ後ろに置く", () => {
  const html = renderPage(DecisionsPage, { state: createActionableDashboardState(), now: NOW });
  const items = decisionItems(html);
  assert.ok(items.length >= 3, "複数の種別の項目が描画される");
  for (const item of items) {
    const children = childElementsOf(item.outer);
    const label = textOf(children[0]?.outer ?? "");
    assert.equal(children.length, 3, `項目の直下は3つ: ${label}`);
    assert.ok(hasClassToken(children[0]!.openingTag, "decision-head"), `1つ目は見出し行: ${label}`);
    assert.ok(hasClassToken(children[1]!.openingTag, "decision-body"), `2つ目は本文: ${label}`);
    assert.ok(
      hasClassToken(children[2]!.openingTag, "actions") || hasClassToken(children[2]!.openingTag, "note"),
      `3つ目は回答の行: ${label}`,
    );
  }
});

test("判断待ちページは本文を折りたたまず、常に描画する", () => {
  const body = "1行目\n2行目\n3行目";
  const html = renderPage(DecisionsPage, { state: stateWithDecision({ body }), now: NOW });
  const item = decisionItems(html)[0]!.outer;
  assert.doesNotMatch(item, /<details|<summary|hidden=""|aria-expanded/, "本文を隠す仕組みを持たない");
  assert.ok(blockOf(item, "desc").includes(body), "本文を原文のまま描画する");
});

test("判断待ちが0件のときは空の状態を警告ではない容器に出し、1件以上では付けない", () => {
  const base = createDashboardState();
  const emptyHtml = renderPage(DecisionsPage, { state: { ...base, pendingDecisions: [] }, now: NOW });
  const emptyTray = elementWithAttribute(emptyHtml, "aria-label", "判断待ちの一覧");
  assert.ok(hasClassToken(emptyTray.openingTag, "tray"), "一覧の容器は tray のまま");
  assert.ok(hasClassToken(emptyTray.openingTag, "tray-empty"), "0件のときは空の状態の見た目にする");
  assert.match(textOf(emptyTray.outer), /判断待ちはありません。/);

  const html = renderPage(DecisionsPage, { state: base, now: NOW });
  const tray = elementWithAttribute(html, "aria-label", "判断待ちの一覧");
  assert.ok(hasClassToken(tray.openingTag, "tray"));
  assert.ok(!hasClassToken(tray.openingTag, "tray-empty"), "項目があるときは空の状態の見た目にしない");
});

test("判断待ちページは本文が空白だけなら本文ブロックの段落を描画しない", () => {
  const html = renderPage(DecisionsPage, {
    state: stateWithDecision({ body: "  \n\t\n  " }),
    now: NOW,
  });
  const descHtml = blockOf(html, "desc");
  assert.equal(descHtml.match(/<p[^>]*>/g)?.length ?? 0, 0, "空の段落は描画しない");
});

test("判断待ちページは CRLF の改行も段落の区切りとして扱う", () => {
  const html = renderPage(DecisionsPage, {
    state: stateWithDecision({ body: "第1段落。\r\n\r\n第2段落。" }),
    now: NOW,
  });
  const descHtml = blockOf(html, "desc");
  const textParagraphs = descHtml.match(/<p[^>]*>[\s\S]*?<\/p>/g) ?? [];
  assert.equal(textParagraphs.length, 2, "CRLF の空行も段落の区切りになる");
  assert.equal(textParagraphs[0]!.includes("\r"), false, "正規化後の段落に復帰文字を残さない");
  assert.match(textParagraphs[0]!, /第1段落。/);
  assert.match(textParagraphs[1]!, /第2段落。/);
});

test("判断待ちページは本文の先頭の字下げを残して描画する", () => {
  const html = renderPage(DecisionsPage, {
    state: stateWithDecision({ body: "  字下げした行です。\n同じ段落の2行目です。" }),
    now: NOW,
  });
  // main のこのテストは、本文の範囲を .desc の開始タグから class="wait" の要素までとして取っていた。
  // 判断待ちの骨格（issue #121）では待ち時間が本文より前の見出し行にあり、class も "wait l-cluster" に
  // 変わったため、その範囲は空になるか文書の末尾まで広がる。本文の範囲は .desc の要素そのものから取る。
  const descHtml = blockOf(html, "desc");
  const paragraphs = descHtml.match(/<p[^>]*>[\s\S]*?<\/p>/g) ?? [];
  assert.equal(paragraphs.length, 1, "単一改行では段落を分けない");
  assert.match(
    paragraphs[0]!,
    /^<p> {2}字下げした行です。\n同じ段落の2行目です。<\/p>$/,
    "段落の先頭の空白を切り取らずに表示する",
  );
});

test("判断待ちページは空行で段落を分け、空行直後の段落の字下げも残す", () => {
  const body = "第1段落です。\n\n　字下げした第2段落です。\n\n  字下げした第3段落です。";
  const html = renderPage(DecisionsPage, {
    state: stateWithDecision({ body }),
    now: NOW,
  });
  // main のこのテストは、本文の範囲を .desc の開始タグから class="wait" の要素までとして取っていた。
  // 判断待ちの骨格（issue #121）では待ち時間が本文より前の見出し行にあり、class も "wait l-cluster" に
  // 変わったため、その範囲は空になるか文書の末尾まで広がる。本文の範囲は .desc の要素そのものから取る。
  const descHtml = blockOf(html, "desc");
  const paragraphs = descHtml.match(/<p[^>]*>[\s\S]*?<\/p>/g) ?? [];
  assert.equal(paragraphs.length, 3, "空行で区切られた3つの段落がそれぞれ別の要素になる");
  assert.match(paragraphs[0]!, /^<p>第1段落です。<\/p>$/);
  assert.match(paragraphs[1]!, /^<p>　字下げした第2段落です。<\/p>$/, "空行直後の段落の全角の字下げを残す");
  assert.match(paragraphs[2]!, /^<p> {2}字下げした第3段落です。<\/p>$/, "空行直後の段落の半角の字下げを残す");
});

// 各項目の見出し行の左の群（class に proj を含む要素）。ほかの class が並んでも拾う。
const headingBlocks = (html: string): string[] => elementsWithClass(html, "proj").map((element) => element.outer);

const headingTexts = (html: string): string[] =>
  headingBlocks(html).map((block) => block.replace(/<[^>]+>/g, ""));

const headingTextFor = (html: string, needle: string): string => {
  const heading = headingTexts(html).find((text) => text.includes(needle));
  if (heading === undefined) {
    assert.fail(`見出しに ${needle} を含む項目がある`);
  }
  return heading;
};

const withServerName = (name: string) => {
  const state = createDashboardState();
  return { ...state, servers: state.servers.map((server) => ({ ...server, name })) };
};

// サーバーの1行（server- 接頭辞の id を持つ要素）を取り出す。
const serverLine = (html: string, serverId: string): string => elementWithAttribute(html, "id", `server-${serverId}`).outer;

const withServerIdentity = (identity: { serverId: string; name: string; kind: string }) => {
  const base = createDashboardState();
  return {
    ...base,
    servers: [{ ...base.servers[0]!, ...identity }],
    runs: [],
  };
};

test("概要ページのサーバーの1行は表示名・サーバー ID・機種をラベル付きで判別できる", () => {
  const state = withServerIdentity({ serverId: "mac-mini", name: "j5ik2o-mac-mini", kind: "Mac mini" });
  const html = renderPage(OverviewPage, { view: buildOverviewView(state, NOW) });
  const heading = textOf(serverLine(html, "mac-mini"));

  assert.match(heading, /表示名: j5ik2o-mac-mini/, "表示名を主として示す");
  assert.match(heading, /サーバー ID: mac-mini/, "サーバー ID を副として示す");
  assert.doesNotMatch(heading, /サーバー ID: j5ik2o-mac-mini/, "表示名をサーバー ID として示さない");
  assert.match(heading, /機種: Mac mini/, "機種を失わない");
  assert.doesNotMatch(heading, /j5ik2o-mac-miniMac mini/, "表示名と機種を区切らずに連結しない");
});

test("概要ページのサーバーの1行も状態表示・アンカー・見出しの id を維持する", () => {
  const state = withServerIdentity({ serverId: "mac-mini", name: "j5ik2o-mac-mini", kind: "Mac mini" });
  const html = renderPage(OverviewPage, { view: buildOverviewView(state, NOW) });
  const heading = serverLine(html, "mac-mini");

  assert.match(heading, /j5ik2o-mac-mini/, "サーバーのアンカーは表示名を持つ1行に付く");
  assert.match(html, /id="h-servers"/, "サーバー区画の見出し id を維持する");
  assert.match(heading, /aria-hidden="true"/, "状態の点は読み上げ対象から外したままにする");
});

test("判断待ちページは各項目の見出しでプロジェクト・種別・対象を判別できる", () => {
  const html = renderPage(DecisionsPage, { state: createDashboardState(), now: NOW });

  const mergeHeading = headingTextFor(html, "corporate-site");
  assert.match(mergeHeading, /プロジェクト: corporate-site/);
  assert.match(mergeHeading, /種別: 人のマージ待ち/);
  assert.match(mergeHeading, /#418/);

  const inquiryHeading = headingTextFor(html, "esa");
  assert.match(inquiryHeading, /プロジェクト: esa/);
  assert.match(inquiryHeading, /種別: 問い合わせ（質問）/);
  assert.match(inquiryHeading, /goal-2/);
});

test("判断待ちページの項目には、表示名の有無に関係なくサーバーの ID も表示名も出さない", () => {
  for (const name of ["j5ik2o-mac-mini", ""]) {
    const html = renderPage(DecisionsPage, { state: withServerName(name), now: NOW });
    const items = decisionItems(html);
    assert.equal(items.length, 2, "判断待ちの項目が描画される");
    for (const item of items) {
      const text = textOf(item.outer);
      assert.doesNotMatch(text, /サーバー/, `項目にサーバーの表示を出さない（表示名: "${name}"）`);
      assert.doesNotMatch(text, /mini/, `項目にサーバー ID も表示名も出さない（表示名: "${name}"）`);
    }
  }
});

test("判断待ちページは項目ごとにサーバーが違っても、どのサーバーの ID も表示名も項目に出さない", () => {
  const base = createDashboardState();
  const state = {
    ...base,
    servers: base.servers.map((server) =>
      server.serverId === "mini" ? { ...server, name: "j5ik2o-mac-mini" } : { ...server, name: "studio-2" },
    ),
    pendingDecisions: base.pendingDecisions.map((decision) =>
      decision.type === PendingDecisionType.PENDING_DECISION_TYPE_INQUIRY
        ? { ...decision, serverId: "down" }
        : decision,
    ),
  };
  const html = renderPage(DecisionsPage, { state, now: NOW });
  const items = decisionItems(html);
  assert.equal(items.length, 2);
  for (const item of items) {
    const text = textOf(item.outer);
    for (const forbidden of ["サーバー", "mini", "j5ik2o-mac-mini", "studio-2", "down"]) {
      assert.ok(!text.includes(forbidden), `項目に ${forbidden} を出さない`);
    }
  }
});

test("判断待ちページは見出しのプロジェクト名を HTML エスケープする", () => {
  const base = createDashboardState();
  const state = {
    ...base,
    pendingDecisions: base.pendingDecisions.map((decision) =>
      decision.type === PendingDecisionType.PENDING_DECISION_TYPE_AWAITING_MERGE
        ? { ...decision, project: "<script>alert(1)</script>" }
        : decision,
    ),
  };
  const html = renderPage(DecisionsPage, { state, now: NOW });
  const block = headingBlocks(html).find((candidate) => candidate.includes("&lt;script&gt;"));
  if (block === undefined) {
    assert.fail("エスケープされたプロジェクト名を見出しに表示する");
  }
  assert.match(block, /alert\(1\)/);
  assert.doesNotMatch(block, /<script>/);
});

test("判断待ちページは対象を持たない項目の見出しに架空の ID を出さない", () => {
  const base = createDashboardState();
  const state = {
    ...base,
    pendingDecisions: base.pendingDecisions.map((decision) => ({
      ...decision,
      goalId: undefined,
      pullRequest: undefined,
    })),
  };
  const html = renderPage(DecisionsPage, { state, now: NOW });
  const headings = headingTexts(html);
  assert.ok(headings.length > 0, "判断待ちの見出しが描画される");
  for (const heading of headings) {
    assert.doesNotMatch(heading, /undefined/);
    assert.doesNotMatch(heading, /NaN/);
  }
});

test("アクティビティページは出来事を新しい順に並べ、7日分の集計とサーバー別の当日件数を描画する", async () => {
  const server = await startTaktDashServer({ activity: createActivityResponse() });
  try {
    await withServerEnv(server.address, async () => {
      const client = createBackendClient();
      try {
        const response = await client.getActivity();
        const html = renderPage(ActivityPage, { response, now: NOW });
        const mergeIndex = html.indexOf("人のマージ待ち");
        const inquiryIndex = html.indexOf("問い合わせ");
        assert.ok(mergeIndex >= 0 && inquiryIndex >= 0);
        assert.ok(mergeIndex < inquiryIndex, "新しい出来事が先に現れる");
        assert.match(html, /2026-11-07/);
        assert.match(html, /2026-11-13/);
        assert.match(html, /Mac mini/);

        const newestRowStart = html.lastIndexOf("<li", html.indexOf("ゴール「採用ページのレイアウトを直す」が人のマージ待ちになりました。"));
        const newestRow = html.slice(newestRowStart, html.indexOf("</li>", newestRowStart));
        assert.match(newestRow, /class="ev warn"/);
        assert.match(newestRow, /16:25/);
        assert.match(newestRow, /人のマージ待ち/);
        assert.match(newestRow, /Mac mini/);
      } finally {
        await client.close();
      }
    });
  } finally {
    await server.shutdown();
  }
});

test("アクティビティページは最新の出来事の一覧を見出しの直後に描画する", () => {
  const html = renderPage(ActivityPage, { response: createActivityResponse(), now: NOW });

  const eventsIndex = html.indexOf("出来事の一覧");
  const dailyIndex = html.indexOf("日ごとの出来事");
  const todayIndex = html.indexOf("今日の出来事");
  assert.ok(eventsIndex >= 0 && dailyIndex >= 0 && todayIndex >= 0);
  assert.ok(eventsIndex < dailyIndex, "出来事の一覧が日ごとの出来事より先に現れる");
  assert.ok(dailyIndex < todayIndex, "日ごとの出来事が今日の出来事より先に現れる");
});

test("アクティビティページは日ごとの出来事を新しい日から並べる", () => {
  const html = renderPage(ActivityPage, { response: createActivityResponse(), now: NOW });

  const orderedDates = [
    "2026-11-13",
    "2026-11-12",
    "2026-11-11",
    "2026-11-10",
    "2026-11-09",
    "2026-11-08",
    "2026-11-07",
  ];
  const positions = orderedDates.map((date) => html.indexOf(date));
  for (const [index, position] of positions.entries()) {
    assert.ok(position >= 0, `${orderedDates[index]} が描画される`);
    if (index > 0) {
      assert.ok(positions[index - 1] < position, `${orderedDates[index - 1]} が ${orderedDates[index]} より先に現れる`);
    }
  }
});

test("概要の run 行はステップ名・位置・反復・進捗バー・モデル・ゴールの目的・要約・経過を描画する", () => {
  const html = renderPage(OverviewPage, { view: buildOverviewView(createDashboardState(), NOW) });

  assert.match(html, /class="st live"/, "状態のラベルを実行中の見た目で出す");
  assert.match(html, /class="dot live"/, "状態の点を実行中の見た目で出す");
  assert.match(html, /ステップ\s*<b>implement<\/b>/, "今のステップ名を出す");
  assert.match(html, /5 ステップ中 2 つ目/, "何ステップ中の何番目かを出す");
  assert.match(html, /反復 2 回目/, "反復回数を出す");
  assert.match(html, /モデルは GLM 5\.3 です。/, "モデルの表示名を出す");
  assert.match(html, /最後のログは 1分前/, "最後のログからの経過を出す");
  assert.match(html, /採用ページのレイアウトを直す/, "ゴールの目的を出す");
  assert.match(html, /レイアウトを直しています。/, "タスクの要約を出す");

  const bar = barMarkup(html);
  assert.match(bar, /role="img"/, "進捗バーは意味を持つ画像として扱う");
  assert.match(bar, /aria-label="5 ステップ中 2 つ目"/, "進捗バーの読み上げ名に位置を出す");
  assert.match(bar, /style="width:40%"/, "2/5 の進み具合をバーの幅に反映する");
});

test("入れ子 workflow の内側ログがある run 行は名前と位置を同じ currentStep から描画する", () => {
  const html = renderPage(OverviewPage, { view: buildOverviewView(createNestedStepDashboardState(), NOW) });

  assert.match(html, /ステップ\s*<b>develop<\/b>/, "今のトップレベル step 名を出す");
  assert.doesNotMatch(html, /<b>write_tests<\/b>/, "入れ子 workflow の内側の step 名を現在ステップとして出さない");
  assert.match(html, /1 ステップ中 1 つ目/, "位置は同じ currentStep 由来の 1 つ目を出す");
  assert.match(html, /反復 3 回目/, "反復回数を出す");
  const bar = barMarkup(html);
  assert.match(bar, /aria-label="1 ステップ中 1 つ目"/, "バーの読み上げ名も同じ step 集合の位置にする");
  assert.match(bar, /style="width:100%"/, "1/1 の進み具合をバーの幅に反映する");
});

test("同じ状態でも時刻が進めば run 行の経過表示が進む", () => {
  const state = createDashboardState();
  const atNow = renderPage(OverviewPage, { view: buildOverviewView(state, NOW) });
  const oneMinuteLater = renderPage(OverviewPage, {
    view: buildOverviewView(state, new Date(NOW.getTime() + 60_000)),
  });

  assert.match(atNow, /最後のログは 1分前/);
  assert.match(oneMinuteLater, /最後のログは 2分前/, "状態の再配信が無くても表示だけが進む");
});

test("ログが止まった run 行は警告の点とバーになり、無音の注記を出す", () => {
  const tenMinutesLater = new Date(NOW.getTime() + 10 * 60_000);
  const html = renderPage(OverviewPage, { view: buildOverviewView(createDashboardState(), tenMinutesLater) });

  assert.match(html, /ログが止まっています/, "警告の状態ラベルを出す");
  assert.match(html, /class="dot warn"/, "点を警告の見た目にする");
  assert.match(html, /ログが 11分 出ていません。/, "無音の分数を注記に出す");

  const bar = barMarkup(html);
  assert.match(bar, /class="[^"]*\bbar\b[^"]*\bwarn\b/, "バーを警告の見た目にする");
  assert.doesNotMatch(bar, /\blive\b/, "警告のバーは実行中の見た目にしない");
});

test("タスクの要約が空の run 行はタスク名を出す", () => {
  const base = createDashboardState();
  const state = { ...base, runs: base.runs.map((run) => ({ ...run, summary: "" })) };
  const html = renderPage(OverviewPage, { view: buildOverviewView(state, NOW) });

  const row = /<tr class="run-row[^"]*">([\s\S]*?)<\/tr>/.exec(html)?.[1];
  assert.ok(row !== undefined, "run の行がある");
  const task = textOf(row);
  assert.match(task, /タスク「採用ページのレイアウトを直す」/, "要約が無ければタスク名を出す");
  assert.doesNotMatch(task, /タスク「」/, "空の要約をそのまま出さない");
});

test("ステップ位置が分からない run 行は進捗バーを出さない", () => {
  const base = createDashboardState();
  const state = {
    ...base,
    runs: base.runs.map((run) => ({ ...run, currentStepNumber: undefined, totalSteps: undefined })),
  };
  const html = renderPage(OverviewPage, { view: buildOverviewView(state, NOW) });

  assert.match(html, /implement/, "ステップ名は出す");
  assert.equal(thickBars(html).length, 0, "位置と総数が無ければ run 行のバーを捏造しない");
});

test("実行中の run の点とバーは動き、prefers-reduced-motion で止まる", async () => {
  const css = await readFile(`${webRoot}app/globals.css`, "utf8");

  assert.match(css, /\.run-row\s+\.dot\.live\s*\{[^}]*animation:\s*pulse/, "実行中の点は脈打つ");
  assert.match(css, /\.run-row\s+\.bar\.live\s*>\s*i\s*\{[^}]*animation:\s*flow/, "実行中のバーは流れる");
  assert.match(css, /\.run-row\s+\.dot\.warn\s*\{[^}]*background:\s*var\(--warn\)/, "警告の点は警告色になる");
  assert.match(css, /\.bar\.warn\s*>\s*i\s*\{[^}]*background:\s*var\(--warn\)/, "警告のバーは警告色になる");
  assert.doesNotMatch(css, /\.bar\.warn\s*>\s*i\s*\{[^}]*animation/, "警告のバーは流れ続けない");

  const reduced = mediaBlock(css, "@media (prefers-reduced-motion: reduce)");
  assert.ok(reduced !== undefined, "prefers-reduced-motion の指定がある");
  for (const selector of [".run-row .dot.live", ".run-row .bar.live > i", ".run-row.flash"]) {
    assert.ok(reduced.includes(selector), `${selector} を reduced motion の対象にする`);
  }
  assert.match(reduced, /animation:\s*none/, "reduced motion ではアニメーションを止める");
});

test("新着ログで発光する run 行だけが flash クラスを持ち、基本規則は1秒のアニメーションになる", async () => {
  const base = createDashboardState();
  const otherRun = { ...base.runs[0]!, runSlug: "run-bbb", sessionId: "session-bbb" };
  const state = DashboardState.create({ ...base, runs: [...base.runs, otherRun] });
  const view = buildOverviewView(state, NOW);

  const html = renderToStaticMarkup(
    createElement(OverviewContent, { view, flashingRunSlugs: new Set(["run-aaa"]), disconnected: false }),
  );

  assert.equal(elementsByClass(html, "flash").length, 1, "発光するのは flashingRunSlugs に入った行だけ");
  const flashRow = /<tr class="run-row flash">([\s\S]*?)<\/tr>/.exec(html)?.[1];
  assert.ok(flashRow?.includes("/runs/run-aaa"), "発光する行は対象 slug の run 行になる");
  const plainRow = /<tr class="run-row">([\s\S]*?)<\/tr>/.exec(html)?.[1];
  assert.ok(plainRow?.includes("/runs/run-bbb"), "flash を持たない行は対象外の slug の run 行になる");

  const css = await readFile(`${webRoot}app/globals.css`, "utf8");
  assert.match(css, /\.run-row\.flash\s*\{[^}]*animation:\s*flash\s+1s/, "基本規則は1秒の flash アニメーション");
});

test("遅延中の run 行は idle にせず、ログが出続けていれば実行中として描画する", () => {
  const state = createDashboardStateWithProjectConnection(
    "corporate-site",
    ConnectionState.CONNECTION_STATE_DELAYED,
    new Date(NOW.getTime() - 300 * 1_000),
  );
  const html = renderPage(OverviewPage, { view: buildOverviewView(state, NOW) });

  assert.match(html, /class="st live"/, "ログが出続けている run は実行中の見た目で出す");
  assert.match(html, /class="dot live"/, "状態の点も実行中の見た目で出す");
  assert.doesNotMatch(html, /run-note delayed/, "読み取り遅延の注記は出さない");
  assert.doesNotMatch(html, /読み取りが遅れています/, "読み取り時刻の注記は出さない");
  assert.doesNotMatch(html, /class="st idle"/, "遅延中の run 行を応答なしの見た目にしない");
});

test("アクティビティページは切断と復帰を別の行にし、同種の接続イベントだけを件数と期間の1行にまとめる", () => {
  const base = createActivityResponse();
  const response = {
    ...base,
    events: [
      {
        occurredAt: new Date(NOW.getTime() - 3 * 60_000),
        serverId: "mini",
        project: "esa",
        kind: ActivityKind.ACTIVITY_KIND_CONNECTION_RESTORED,
        body: "接続が復帰しました。切断していた時間は 90 秒です。",
      },
      {
        occurredAt: new Date(NOW.getTime() - 5 * 60_000),
        serverId: "mini",
        project: "esa",
        kind: ActivityKind.ACTIVITY_KIND_CONNECTION_RESTORED,
        body: "接続が復帰しました。切断していた時間は 150 秒です。",
      },
      {
        occurredAt: new Date(NOW.getTime() - 10 * 60_000),
        serverId: "mini",
        project: "esa",
        kind: ActivityKind.ACTIVITY_KIND_CONNECTION_LOST,
        body: "接続が切れました。",
      },
      {
        occurredAt: new Date(NOW.getTime() - 15 * 60_000),
        serverId: "mini",
        project: "esa",
        kind: ActivityKind.ACTIVITY_KIND_CONNECTION_LOST,
        body: "接続が切れました。",
      },
    ],
  };

  const html = renderPage(ActivityPage, { response, now: NOW });
  const rows = html.match(/<li class="ev[\s\S]*?<\/li>/g) ?? [];

  assert.equal(rows.length, 2, "切断と復帰は別の行になり、同種の接続イベントだけが1行にまとまる");
  const restoredRow = rows[0]!;
  assert.match(restoredRow, /接続の復帰/, "復帰は復帰だけの行に出す");
  assert.match(restoredRow, /2件/, "復帰も同種ならまとめた件数を出す");
  assert.match(restoredRow, /切断していた時間は 90 秒です。/, "まとめた復帰の行は最新の出来事の本文を出す");
  assert.doesNotMatch(restoredRow, /150 秒/, "まとめた復帰の行に古い出来事の本文は出さない");
  assert.doesNotMatch(restoredRow, /接続の状態が変わりました。/, "まとめた行に汎用の文言は出さない");
  assert.match(restoredRow, /16:25/, "復帰の期間の始まりを出す");
  assert.match(restoredRow, /16:27/, "復帰の期間の終わりを出す");
  assert.ok(restoredRow.indexOf("16:25") < restoredRow.indexOf("16:27"), "復帰の期間を最古から最新の順に出す");
  const lostRow = rows[1]!;
  assert.match(lostRow, /esa/, "まとめた行にプロジェクト名を出す");
  assert.match(lostRow, /接続の切断/, "切断は切断だけの行に出す");
  assert.match(lostRow, /2件/, "まとめた件数を出す");
  assert.match(lostRow, /接続が切れました。/, "まとめた切断の行も最新の出来事の具体的な本文を出す");
  assert.doesNotMatch(lostRow, /接続の状態が変わりました。/, "まとめた行に汎用の文言は出さない");
  assert.match(lostRow, /16:15/, "期間の始まりを出す");
  assert.match(lostRow, /16:20/, "期間の終わりを出す");
  assert.ok(lostRow.indexOf("16:15") < lostRow.indexOf("16:20"), "期間を最古から最新の順に出す");
});

test("アクティビティページは接続の復帰イベントを種類と切断時間つきで描画する", () => {
  const base = createActivityResponse();
  const response = {
    ...base,
    events: [
      {
        occurredAt: new Date(NOW.getTime() - 5 * 60_000),
        serverId: "mini",
        project: "esa",
        kind: ActivityKind.ACTIVITY_KIND_CONNECTION_RESTORED,
        body: "接続が復帰しました。切断していた時間は 150 秒です。",
      },
    ],
  };

  const html = renderPage(ActivityPage, { response, now: NOW });

  assert.match(html, /接続の復帰/, "復帰イベントの種類を表示する");
  assert.match(html, /切断していた時間は 150 秒です。/, "本文の切断時間をそのまま表示する");
});

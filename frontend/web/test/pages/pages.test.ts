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
  createActivityResponse,
  createDashboardState,
  createProjectsResponse,
  NOW,
} from "../support/fixtures.ts";
import { startTaktDashServer } from "../support/grpc-server.ts";
import { createBackendClient } from "../../src/grpc/client.ts";
import { buildOverviewView, type OverviewView } from "../../src/state/dashboard.ts";
import { DashboardState, InquiryKind, PendingDecisionType } from "../../src/proto/taktdash.ts";
import OverviewPage from "../../app/page.tsx";
import ProjectsPage from "../../app/projects/page.tsx";
import DecisionsPage from "../../app/decisions/page.tsx";
import ActivityPage from "../../app/activity/page.tsx";

const webRoot = fileURLToPath(new URL("../../", import.meta.url));

const renderPage = (Page: ComponentType<never>, props: unknown): string =>
  renderToStaticMarkup(createElement(Page as ComponentType<never>, props as never) as ReactElement);

// 判断待ちを1件だけ持つ状態を作る。本文と選択肢を表示の検証に必要な値だけ上書きする。
const stateWithDecision = (
  overrides: { body: string; options?: string[] },
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

test("概要ページは読み取り状態・最終読み取り時刻・プロジェクト・run・判断待ち件数・閾値を描画する", async () => {
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
        assert.match(html, /判断待ち/);
        assert.match(html, /240/);
        assert.match(html, /10/);
        assert.match(html, /最終/);
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

test("判断待ちページは待ち時間順に描画し、滞留の項目に滞留の表示を付ける", async () => {
  const state = createDashboardState();
  const html = renderPage(DecisionsPage, { state, now: NOW });
  const mergeIndex = html.indexOf("人のマージ待ち");
  const inquiryIndex = html.indexOf("問い合わせ");
  assert.ok(mergeIndex >= 0 && inquiryIndex >= 0);
  assert.ok(mergeIndex < inquiryIndex, "待ち時間の長いマージ待ちが先に現れる");
  assert.match(html, /滞留/);
});

test("判断待ちページは回答・承認・却下の操作要素を持たない", () => {
  const html = renderPage(DecisionsPage, { state: createDashboardState(), now: NOW });
  assert.doesNotMatch(html, /<button/i);
  assert.doesNotMatch(html, /<form/i);
  assert.doesNotMatch(html, /<input/i);
  assert.doesNotMatch(html, /実施した/);
  assert.doesNotMatch(html, /承認/);
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
  const descHtml = html.slice(html.indexOf(descBlocks[0]!), html.indexOf('class="wait"'));
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
  const descBlock = elementsByClass(html, "desc")[0];
  const descHtml = html.slice(html.indexOf(descBlock!), html.indexOf('class="wait"'));
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

  const optsHtml = html.slice(optsIndex, html.indexOf('class="wait"'));
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
  const optsHtml = html.slice(optsIndex, html.indexOf('class="wait"'));
  assert.equal(occurrences(descHtml, "今週"), 1, "本文中に選択肢欄の内容が二重に現れない");
  const items = optsHtml.match(/<li[^>]*>[\s\S]*?<\/li>/g) ?? [];
  assert.equal(items.length, 2, "選択肢欄は本文の記述とは別に各選択肢を並べる");
});

test("判断待ちページは見出し・本文・選択肢・待ち時間をそれぞれ別のブロックとして描画する", async () => {
  const html = renderPage(DecisionsPage, {
    state: stateWithDecision({ body: "リリースの日取りはどうしますか。", options: ["今週", "来週"] }),
    now: NOW,
  });
  const projIndex = html.indexOf(elementsByClass(html, "proj")[0] ?? "");
  const descIndex = html.indexOf(elementsByClass(html, "desc")[0] ?? "");
  const optsIndex = html.indexOf(elementsByClass(html, "opts")[0] ?? "");
  const waitIndex = html.indexOf(elementsByClass(html, "wait")[0] ?? "");
  assert.ok(projIndex >= 0 && descIndex >= 0 && optsIndex >= 0 && waitIndex >= 0);
  assert.ok(
    projIndex < descIndex && descIndex < optsIndex && optsIndex < waitIndex,
    "見出し・本文・選択肢・待ち時間の順に並ぶ",
  );

  const css = await readFile(`${webRoot}app/globals.css`, "utf8");
  for (const className of ["proj", "desc", "opts", "wait"]) {
    assert.match(
      css,
      new RegExp(`\\.tray-item\\s+\\.${className}[,\\s][^{}]*\\{[^}]*display:\\s*block`),
      `.tray-item .${className} は行内に連結せずブロックとして積まれる`,
    );
  }
});

test("判断待ちページは本文が空白だけなら本文ブロックの段落を描画しない", () => {
  const html = renderPage(DecisionsPage, {
    state: stateWithDecision({ body: "  \n\t\n  " }),
    now: NOW,
  });
  const descBlock = elementsByClass(html, "desc")[0];
  const descHtml = html.slice(html.indexOf(descBlock!), html.indexOf('class="wait"'));
  assert.equal(descHtml.match(/<p[^>]*>/g)?.length ?? 0, 0, "空の段落は描画しない");
});

test("判断待ちページは CRLF の改行も段落の区切りとして扱う", () => {
  const html = renderPage(DecisionsPage, {
    state: stateWithDecision({ body: "第1段落。\r\n\r\n第2段落。" }),
    now: NOW,
  });
  const descBlock = elementsByClass(html, "desc")[0];
  const descHtml = html.slice(html.indexOf(descBlock!), html.indexOf('class="wait"'));
  const textParagraphs = descHtml.match(/<p[^>]*>[\s\S]*?<\/p>/g) ?? [];
  assert.equal(textParagraphs.length, 2, "CRLF の空行も段落の区切りになる");
  assert.equal(textParagraphs[0]!.includes("\r"), false, "正規化後の段落に復帰文字を残さない");
  assert.match(textParagraphs[0]!, /第1段落。/);
  assert.match(textParagraphs[1]!, /第2段落。/);
});

const headingBlocks = (html: string): string[] =>
  Array.from(html.matchAll(/<div class="proj">([\s\S]*?)<\/div>/g), (match) => match[1]);

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

test("判断待ちページは各項目の見出しでプロジェクト・サーバーの表示名・種別・対象を判別できる", () => {
  const html = renderPage(DecisionsPage, { state: withServerName("j5ik2o-mac-mini"), now: NOW });

  const mergeHeading = headingTextFor(html, "corporate-site");
  assert.match(mergeHeading, /プロジェクト: corporate-site/);
  assert.match(mergeHeading, /サーバー: j5ik2o-mac-mini/);
  assert.doesNotMatch(mergeHeading, /サーバー: mini/);
  assert.match(mergeHeading, /種別: 人のマージ待ち/);
  assert.match(mergeHeading, /#418/);

  const inquiryHeading = headingTextFor(html, "esa");
  assert.match(inquiryHeading, /プロジェクト: esa/);
  assert.match(inquiryHeading, /サーバー: j5ik2o-mac-mini/);
  assert.match(inquiryHeading, /種別: 問い合わせ（質問）/);
  assert.match(inquiryHeading, /goal-2/);
});

test("判断待ちページは各項目の見出しにその項目のサーバーの表示名を出し、他のサーバーの表示名を出さない", () => {
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

  const mergeHeading = headingTextFor(html, "corporate-site");
  assert.match(mergeHeading, /サーバー: j5ik2o-mac-mini/);
  assert.doesNotMatch(mergeHeading, /サーバー: studio-2/);
  assert.doesNotMatch(mergeHeading, /サーバー: mini/);

  const inquiryHeading = headingTextFor(html, "esa");
  assert.match(inquiryHeading, /サーバー: studio-2/);
  assert.doesNotMatch(inquiryHeading, /サーバー: j5ik2o-mac-mini/);
  assert.doesNotMatch(inquiryHeading, /サーバー: down/);
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

// ページの描画の契約（C4/C5/C6/C7/C8/C9/C10/C12）。
//
// ダミー gRPC サーバーから取得した proto のメッセージ型を、サーバー側の
// 読み込み処理（load*）で表示用の形へ変換し、各ページが描画に使う。
// 描画結果は HTML 文字列として観測し、モックに沿う要素が現れることを確かめる。
import test from "node:test";
import assert from "node:assert/strict";
import { createElement, type ComponentType, type ReactElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import {
  createActionableDashboardState,
  createActivityResponse,
  createDashboardState,
  createProjectsResponse,
  NOW,
} from "../support/fixtures.ts";
import { startTaktDashServer } from "../support/grpc-server.ts";
import { createBackendClient } from "../../src/grpc/client.ts";
import { buildOverviewView, type OverviewView } from "../../src/state/dashboard.ts";
import OverviewPage from "../../app/page.tsx";
import ProjectsPage from "../../app/projects/page.tsx";
import DecisionsPage from "../../app/decisions/page.tsx";
import ActivityPage from "../../app/activity/page.tsx";

const renderPage = (Page: ComponentType<never>, props: unknown): string =>
  renderToStaticMarkup(createElement(Page as ComponentType<never>, props as never) as ReactElement);

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

test("プロジェクトページは読み取りに失敗したプロジェクトの run を時点付きで描画する", () => {
  const response = createProjectsResponse();
  const state = response.state!;
  const failedState = {
    ...state,
    projects: state.projects.map((project) =>
      project.name === "esa"
        ? { ...project, connected: false, lastReadAt: new Date(NOW.getTime() - 21 * 60_000) }
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

test("判断待ちページは種別ごとの操作要素を一覧に描画する", () => {
  const html = renderPage(DecisionsPage, { state: createActionableDashboardState(), now: NOW });
  assert.match(html, /<button/i);
  assert.match(html, /<input[^>]*type="text"/);
  assert.match(html, /実施した/);
  assert.match(html, /対応できない/);
  assert.match(html, /マージを承認/);
  assert.match(html, /却下/);
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
      } finally {
        await client.close();
      }
    });
  } finally {
    await server.shutdown();
  }
});

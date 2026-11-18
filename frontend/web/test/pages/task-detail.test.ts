// タスク詳細の契約（TASK-01 から TASK-05）。
//
// content は proto のタスク詳細応答を表示し、loader は本番と同じく RSC で取得して
// 404 と接続不可を既存の run 詳細と同じ流儀で扱う。
import test from "node:test";
import assert from "node:assert/strict";
import { createElement, type ReactElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import * as grpc from "@grpc/grpc-js";
import { createTaskDetailResponse } from "../support/fixtures.ts";
import { reserveClosedAddress, startTaktDashServer } from "../support/grpc-server.ts";
import { formatDateTime } from "../../src/state/format.ts";
import { resetBackendResponded } from "../../src/grpc/client.ts";
import { TaskDetailContent } from "../../src/components/task-detail-content.tsx";
import { TaskDetailLoader } from "../../src/components/task-detail-loader.tsx";
import TaskDetailPage from "../../app/tasks/[project]/page.tsx";
import { closeBackendClient } from "../../src/grpc/client.ts";

const NOT_FOUND_DIGEST = "NEXT_HTTP_ERROR_FALLBACK;404";
const TASK_NAME = "採用ページのレイアウトを直す";

const withServerEnv = async <T>(address: string, run: () => Promise<T>): Promise<T> => {
  await closeBackendClient();
  const previous = process.env.TAKTDASH_BACKEND_ADDR;
  process.env.TAKTDASH_BACKEND_ADDR = address;
  try {
    return await run();
  } finally {
    await closeBackendClient();
    if (previous === undefined) {
      delete process.env.TAKTDASH_BACKEND_ADDR;
    } else {
      process.env.TAKTDASH_BACKEND_ADDR = previous;
    }
  }
};

const renderDetail = (detail = createTaskDetailResponse()): string =>
  renderToStaticMarkup(createElement(TaskDetailContent, { detail }));

const loadTask = (task = TASK_NAME, goal?: string): Promise<ReactElement> =>
  TaskDetailLoader({
    params: Promise.resolve({ project: "corporate-site" }),
    searchParams: Promise.resolve(goal === undefined ? { task } : { task, goal }),
  });

const countOf = (html: string, needle: string): number => html.split(needle).length - 1;

test("タスク詳細は状態・目的・要約を表示する", () => {
  const html = renderDetail();
  assert.match(html, /完了/);
  assert.match(html, /目的/);
  assert.match(html, new RegExp(TASK_NAME));
  assert.match(html, /要約/);
  assert.match(html, /レイアウトを直して、確認を待っています。/);
});

test("タスク詳細はブランチとプルリクエストを表示する", () => {
  const html = renderDetail();
  assert.match(html, /goal\/45-frontend/);
  assert.match(html, /418/);
  assert.match(html, /https:\/\/example\.test\/pr\/418/);
});

test("タスク詳細はブランチやプルリクエストが省略されたとき、欠落が分かる表示にする", () => {
  const html = renderDetail(
    createTaskDetailResponse({
      branch: undefined,
      pullRequestNumber: undefined,
      pullRequestUrl: undefined,
      runSlug: undefined,
      linkedRunSlugs: [],
    }),
  );
  assert.match(html, /未取得/);
  assert.doesNotMatch(html, /undefined/);
});

test("タスク詳細は紐づく run を表示し、run 詳細へのリンクを持つ", () => {
  const html = renderDetail();
  assert.match(html, /href="\/runs\/run-aaa"/);
  assert.match(html, /href="\/runs\/run-bbb"/);
});

test("タスク詳細は開始時刻が応答にあれば実値を表示し、作成・完了は未取得と表示する", () => {
  const detail = createTaskDetailResponse();
  const html = renderDetail(detail);
  assert.ok(html.includes(formatDateTime(detail.startedAt!)), "開始時刻を表示する");
  assert.equal(countOf(html, "未取得"), 2, "作成と完了の2件だけが未取得になる");
});

test("タスク詳細は作成・開始・完了の時刻が無いとき、3項目とも未取得と表示する", () => {
  const html = renderDetail(createTaskDetailResponse({ startedAt: undefined }));
  assert.match(html, /作成時刻/);
  assert.match(html, /開始時刻/);
  assert.match(html, /完了時刻/);
  assert.equal(countOf(html, "未取得"), 3);
});

test("タスク詳細は作成・開始・完了の時刻が揃えば3件とも実値を表示し、未取得を出さない", () => {
  const createdAt = new Date("2026-11-13T06:00:00Z");
  const startedAt = new Date("2026-11-13T06:30:00Z");
  const completedAt = new Date("2026-11-13T07:00:00Z");
  const html = renderDetail(createTaskDetailResponse({ createdAt, startedAt, completedAt }));
  assert.ok(html.includes(formatDateTime(createdAt)), "作成時刻を表示する");
  assert.ok(html.includes(formatDateTime(startedAt)), "開始時刻を表示する");
  assert.ok(html.includes(formatDateTime(completedAt)), "完了時刻を表示する");
  assert.ok(
    html.indexOf(formatDateTime(createdAt)) < html.indexOf(formatDateTime(startedAt)) &&
      html.indexOf(formatDateTime(startedAt)) < html.indexOf(formatDateTime(completedAt)),
    "時刻は作成・開始・完了の順に表示する",
  );
  assert.equal(countOf(html, "未取得"), 0, "3時刻が揃っていれば未取得を出さない");
});

test("タスク詳細は sourceRunSlug 由来の run を run 詳細へのリンクとして表示する", () => {
  const html = renderDetail(
    createTaskDetailResponse({
      runSlug: "run-main",
      linkedRunSlugs: ["run-main", "20261113-164757-taktdash-no-source-run"],
    }),
  );
  assert.match(html, /href="\/runs\/run-main"/);
  assert.match(html, /href="\/runs\/20261113-164757-taktdash-no-source-run"/);
});

test("タスク詳細ページは params とクエリで本番経路を通り、詳細を描画する", async () => {
  const server = await startTaktDashServer({ taskDetail: createTaskDetailResponse() });
  try {
    await withServerEnv(server.address, async () => {
      const pageElement = TaskDetailPage({
        params: Promise.resolve({ project: "corporate-site" }),
        searchParams: Promise.resolve({ task: TASK_NAME, goal: "goal-1" }),
      });
      assert.equal(pageElement.type, TaskDetailLoader, "page は TaskDetailLoader に params とクエリを渡す");
      const html = renderToStaticMarkup(await loadTask(TASK_NAME, "goal-1"));
      assert.match(html, /レイアウトを直して、確認を待っています。/);
    });
  } finally {
    await server.shutdown();
  }
});

test("見つからないタスクは本番経路で notFound()（404 の digest）に到達する", async () => {
  const server = await startTaktDashServer({
    taskDetail: { code: grpc.status.NOT_FOUND, details: "missing" },
  });
  try {
    await withServerEnv(server.address, async () => {
      await assert.rejects(
        () => loadTask("no-such-task"),
        (error: Error & { digest?: string }) => error.digest === NOT_FOUND_DIGEST,
      );
    });
  } finally {
    await server.shutdown();
  }
});

test("task が空文字のときは未指定として扱い、backend へ聞かずに notFound()（404 の digest）に到達する", async () => {
  let called = false;
  const server = await startTaktDashServer({
    taskDetail: { code: grpc.status.INVALID_ARGUMENT, details: "タスク名が空白です" },
    onGetTaskDetail: () => {
      called = true;
    },
  });
  try {
    await withServerEnv(server.address, async () => {
      await assert.rejects(
        () => loadTask(""),
        (error: Error & { digest?: string }) => error.digest === NOT_FOUND_DIGEST,
      );
    });
    assert.equal(called, false, "空文字は未指定として扱い、backend へ問い合わせない");
  } finally {
    await server.shutdown();
  }
});

test("task が空白のみのときは未指定として扱い、backend へ聞かずに notFound()（404 の digest）に到達する", async () => {
  let called = false;
  const server = await startTaktDashServer({
    taskDetail: { code: grpc.status.INVALID_ARGUMENT, details: "タスク名が空白です" },
    onGetTaskDetail: () => {
      called = true;
    },
  });
  try {
    await withServerEnv(server.address, async () => {
      await assert.rejects(
        () => loadTask(" "),
        (error: Error & { digest?: string }) => error.digest === NOT_FOUND_DIGEST,
      );
    });
    assert.equal(called, false, "空白のみは未指定として扱い、backend へ問い合わせない");
  } finally {
    await server.shutdown();
  }
});

test("応答のタスク名が要求と一致しないときも本番経路で notFound()（404 の digest）に到達する", async () => {
  const server = await startTaktDashServer({
    taskDetail: createTaskDetailResponse({ taskName: "別のタスク" }),
  });
  try {
    await withServerEnv(server.address, async () => {
      await assert.rejects(
        () => loadTask(TASK_NAME),
        (error: Error & { digest?: string }) => error.digest === NOT_FOUND_DIGEST,
      );
    });
  } finally {
    await server.shutdown();
  }
});

test("応答のプロジェクトが要求と一致しないときも本番経路で notFound()（404 の digest）に到達する", async () => {
  const server = await startTaktDashServer({
    taskDetail: createTaskDetailResponse({ project: "esa" }),
  });
  try {
    await withServerEnv(server.address, async () => {
      await assert.rejects(
        () => loadTask(TASK_NAME),
        (error: Error & { digest?: string }) => error.digest === NOT_FOUND_DIGEST,
      );
    });
  } finally {
    await server.shutdown();
  }
});

test("backend に接続できないときタスク詳細のローダーは落ちず不通の旨を描画する", async () => {
  // このテストは、一度も backend の応答を受けていない Web プロセスを前提にする。
  // 同じファイルの成功ケースで残った接続履歴の影響を受けないよう、前提を明示する。
  resetBackendResponded();
  const closedAddress = await reserveClosedAddress();
  await withServerEnv(closedAddress, async () => {
    const html = renderToStaticMarkup(await loadTask(TASK_NAME));
    assert.match(html, /backend に接続できません/);
    assert.match(html, /TAKTDASH_BACKEND_ADDR/);
  });
});

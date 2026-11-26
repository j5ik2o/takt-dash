// run 詳細ページの契約（C10/C11/C12）。
//
// 本番経路は app/runs/[slug]/page.tsx → RunDetailLoader → getRunDetail で、
// 404 は RunNotFoundError を捕捉した RunDetailLoader の notFound() で起きる。
// ここでは page に slug 付き params を渡して本番経路を通し、描画と 404 到達を観測する。
import test from "node:test";
import assert from "node:assert/strict";
import { renderToStaticMarkup } from "react-dom/server";
import * as grpc from "@grpc/grpc-js";
import { createRunDetailResponse } from "../support/fixtures.ts";
import { startTaktDashServer } from "../support/grpc-server.ts";
import { closeBackendClient } from "../../src/grpc/client.ts";
import RunDetailPage from "../../app/runs/[slug]/page.tsx";
import { RunDetailLoader } from "../../src/components/run-detail-loader.tsx";

const NOT_FOUND_DIGEST = "NEXT_HTTP_ERROR_FALLBACK;404";

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

// page.tsx は params を受けて RunDetailLoader（非同期コンポーネント）を描画する。非同期
// コンポーネントは renderToStaticMarkup で直接描画すると Suspense で中断するため、本番と
// 同じ RunDetailLoader を params 付きで await して要素を得てから同期描画する。
// page.tsx が params を loader へ渡す配線は、page が返す要素の型で確認する。
const renderViaPage = async (slug: string): Promise<string> => {
  const pageElement = RunDetailPage({ params: Promise.resolve({ slug }) });
  assert.equal(pageElement.type, RunDetailLoader, "page は RunDetailLoader に params を渡す");
  const loaderElement = await RunDetailLoader({ params: Promise.resolve({ slug }) });
  return renderToStaticMarkup(loaderElement);
};

test("run 詳細ページは slug 付き params で本番経路を通り、メタデータ・ステップ・ログ末尾・レポート・指示書・今のステップを描画する", async () => {
  const server = await startTaktDashServer({ runDetail: createRunDetailResponse("run-aaa") });
  try {
    const html = await withServerEnv(server.address, () => renderViaPage("run-aaa"));
    assert.match(html, /採用ページのレイアウトを直す/);
    assert.match(html, /simple-mini/);
    assert.match(html, /計画をまとめました。/);
    assert.match(html, /最後のログの行です。/);
    assert.match(html, /review\.md/);
    assert.match(html, /#7/);
    assert.match(html, /implement/);
  } finally {
    await server.shutdown();
  }
});

test("run 詳細ページはレポートと指示書の本文を最初の描画に含めず、項目ごとに本文を開く操作を持つ", async () => {
  // 本文は開いたときに GetRunContent で取得する。最初の描画で backend へ本文を要求しないことを、
  // ダミーサーバーが受けた RPC の記録で確かめる。
  const calls: string[] = [];
  const server = await startTaktDashServer({ runDetail: createRunDetailResponse("run-aaa"), calls });
  try {
    const html = await withServerEnv(server.address, () => renderViaPage("run-aaa"));
    assert.match(html, /review\.md（1200 バイト）/);
    assert.match(html, /配信済み（次のステップ）/);
    assert.equal((html.match(/<details\b/g) ?? []).length, 2, "レポート1件と指示書1件にそれぞれ本文を開く操作がある");
    // サーバーの表示名を引くための GetOverview は呼んでよい（issue #315 D16。本番では layout と共有する）。
    assert.ok(calls.includes("GetRunDetail"), "run 詳細を読む");
    assert.ok(!calls.includes("GetRunContent"), "最初の描画では GetRunContent を呼ばない");
  } finally {
    await server.shutdown();
  }
});

test("ログの末尾は、log_tail の各行を「ログの行」の一覧の項目として描画し、追記を足せる形にする", async () => {
  // 開いている間に届く追記分は、この一覧の末尾へ項目として足す。最初の描画は log_tail の行です。
  const response = {
    ...createRunDetailResponse("run-aaa"),
    logTail: "2026-11-13T09:01:00Z step_complete plan\n2026-11-13T09:05:00Z step_start implement",
  };
  const server = await startTaktDashServer({ runDetail: response });
  try {
    const html = await withServerEnv(server.address, () => renderViaPage("run-aaa"));
    const list = html.match(/<ol[^>]*aria-label="ログの行"[^>]*>([\s\S]*?)<\/ol>/);
    assert.ok(list, "ログの末尾に「ログの行」の一覧がある");
    const items = [...list[1].matchAll(/<li[^>]*>([\s\S]*?)<\/li>/g)].map((match) => match[1]);
    assert.deepEqual(items, ["2026-11-13T09:01:00Z step_complete plan", "2026-11-13T09:05:00Z step_start implement"]);
    assert.match(html, /セッションログの活動はまだありません。/, "活動の印が届く前はまだ無いことを示す");
  } finally {
    await server.shutdown();
  }
});

test("ログが残っていない run は、ログの末尾の節に「ログは残っていません。」と表示する", async () => {
  const response = {
    ...createRunDetailResponse("run-aaa"),
    logsRemoved: true,
    stepLogs: [],
    logTail: "",
    reports: [],
  };
  const server = await startTaktDashServer({ runDetail: response });
  try {
    const html = await withServerEnv(server.address, () => renderViaPage("run-aaa"));
    assert.match(html, /ログは残っていません。/);
    assert.match(html, /採用ページのレイアウトを直す/, "状態ストアの値（タスク名）は表示する");
  } finally {
    await server.shutdown();
  }
});

test("ログが残っている run には「ログは残っていません。」を表示しない", async () => {
  const server = await startTaktDashServer({ runDetail: createRunDetailResponse("run-aaa") });
  try {
    const html = await withServerEnv(server.address, () => renderViaPage("run-aaa"));
    assert.doesNotMatch(html, /ログは残っていません。/);
  } finally {
    await server.shutdown();
  }
});

test("run 詳細ページは本番経路でも JSONL の直接表示や生ログの参照を持たない", async () => {
  const server = await startTaktDashServer({ runDetail: createRunDetailResponse("run-aaa") });
  try {
    const html = await withServerEnv(server.address, () => renderViaPage("run-aaa"));
    assert.doesNotMatch(html, /\.jsonl/);
    assert.doesNotMatch(html, /元の JSON/);
  } finally {
    await server.shutdown();
  }
});

test("開始時刻が無い run でも本番経路で描画し、開始時刻は不明と表示する", async () => {
  const response = { ...createRunDetailResponse("run-aaa"), startedAt: undefined };
  const server = await startTaktDashServer({ runDetail: response });
  try {
    const html = await withServerEnv(server.address, () => renderViaPage("run-aaa"));
    assert.match(html, /開始時刻は不明です。/);
    assert.match(html, /採用ページのレイアウトを直す/);
    assert.doesNotMatch(html, /1970-01-01/);
  } finally {
    await server.shutdown();
  }
});

test("見つからない runSlug は本番経路で notFound()（404 の digest）に到達する", async () => {
  const server = await startTaktDashServer({ runDetail: { code: grpc.status.NOT_FOUND, details: "missing" } });
  try {
    await withServerEnv(server.address, async () => {
      await assert.rejects(
        () => renderViaPage("no-such-run"),
        (error: Error & { digest?: string }) => error.digest === NOT_FOUND_DIGEST,
      );
    });
  } finally {
    await server.shutdown();
  }
});

test("runSlug が応答と一致しないときも本番経路で notFound()（404 の digest）に到達する", async () => {
  const server = await startTaktDashServer({ runDetail: createRunDetailResponse("run-other") });
  try {
    await withServerEnv(server.address, async () => {
      await assert.rejects(
        () => renderViaPage("run-aaa"),
        (error: Error & { digest?: string }) => error.digest === NOT_FOUND_DIGEST,
      );
    });
  } finally {
    await server.shutdown();
  }
});

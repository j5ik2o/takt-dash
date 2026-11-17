// backend 不通用のページ表示の契約（order.md:21）。
//
// 概要はレイアウトが、それ以外の3ページは各ローダーが backend 不通用の旨を表示する。
// ここでは activity / projects / run 詳細のローダーを、接続できないアドレスで実行し、
// 例外で落ちず同じ旨を描画することを観測する。
import test from "node:test";
import assert from "node:assert/strict";
import * as grpc from "@grpc/grpc-js";
import { renderToStaticMarkup } from "react-dom/server";
import { reserveClosedAddress, startTaktDashServer } from "../support/grpc-server.ts";
import { createDashboardState } from "../support/fixtures.ts";
import {
  createBackendClient,
  hasBackendResponded,
  resetBackendResponded,
} from "../../src/grpc/client.ts";
import { ActivityLoader } from "../../src/components/activity-loader.tsx";
import { ConversationsLoader } from "../../src/components/conversations-loader.tsx";
import { ProjectsLoader } from "../../src/components/projects-loader.tsx";
import { RunDetailLoader } from "../../src/components/run-detail-loader.tsx";

const withClosedBackend = async <T>(run: () => Promise<T>): Promise<T> => {
  const closedAddress = await reserveClosedAddress();
  const previous = process.env.TAKTDASH_BACKEND_ADDR;
  process.env.TAKTDASH_BACKEND_ADDR = closedAddress;
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

const assertUnavailableNotice = (html: string): void => {
  assert.match(html, /backend に接続できません/);
  assert.match(html, /TAKTDASH_BACKEND_ADDR/);
};

test("backend に接続できないときアクティビティのローダーは落ちず不通の旨を描画する", async () => {
  const html = await withClosedBackend(async () => renderToStaticMarkup(await ActivityLoader()));
  assertUnavailableNotice(html);
});

test("backend に接続できないときプロジェクトのローダーは落ちず不通の旨を描画する", async () => {
  const html = await withClosedBackend(async () => renderToStaticMarkup(await ProjectsLoader()));
  assertUnavailableNotice(html);
});

test("backend に接続できないとき run 詳細のローダーは落ちず不通の旨を描画する", async () => {
  const html = await withClosedBackend(async () =>
    renderToStaticMarkup(await RunDetailLoader({ params: Promise.resolve({ slug: "run-aaa" }) })),
  );
  assertUnavailableNotice(html);
});

test("backend に接続できないとき会話のローダーは落ちず不通の旨を描画する", async () => {
  const html = await withClosedBackend(async () => renderToStaticMarkup(await ConversationsLoader()));
  assertUnavailableNotice(html);
});

const withBackendAddress = async <T>(address: string, run: () => Promise<T>): Promise<T> => {
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

test("backend が応答しないとき run 詳細のローダーは接続不可と違う文面を描画する", async () => {
  const server = await startTaktDashServer({
    runDetail: { code: grpc.status.DEADLINE_EXCEEDED, details: "期限切れ" },
  });
  try {
    const html = await withBackendAddress(server.address, async () =>
      renderToStaticMarkup(await RunDetailLoader({ params: Promise.resolve({ slug: "run-aaa" }) })),
    );
    assert.match(html, /backend が応答しません/);
    assert.doesNotMatch(html, /TAKTDASH_BACKEND_ADDR/);
  } finally {
    await server.shutdown();
  }
});

test("一度接続できた後の失敗ではローダーは設定の確認を促す文面を描画しない", async () => {
  resetBackendResponded();
  try {
    const server = await startTaktDashServer({ overview: createDashboardState() });
    try {
      const client = createBackendClient({ env: { TAKTDASH_BACKEND_ADDR: server.address } });
      try {
        await client.getOverview();
      } finally {
        await client.close();
      }
    } finally {
      await server.shutdown();
    }
    assert.equal(hasBackendResponded(), true, "応答を受けて接続履歴が残っている");

    const html = await withClosedBackend(async () => renderToStaticMarkup(await ProjectsLoader()));
    assert.match(html, /backend に接続できません/);
    assert.doesNotMatch(html, /TAKTDASH_BACKEND_ADDR/);
  } finally {
    resetBackendResponded();
  }
});

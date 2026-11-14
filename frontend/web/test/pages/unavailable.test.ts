// backend 不通用のページ表示の契約（order.md:21）。
//
// 概要はレイアウトが、それ以外の3ページは各ローダーが backend 不通用の旨を表示する。
// ここでは activity / projects / run 詳細のローダーを、接続できないアドレスで実行し、
// 例外で落ちず同じ旨を描画することを観測する。
import test from "node:test";
import assert from "node:assert/strict";
import { renderToStaticMarkup } from "react-dom/server";
import { reserveClosedAddress } from "../support/grpc-server.ts";
import { ActivityLoader } from "../../src/components/activity-loader.tsx";
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

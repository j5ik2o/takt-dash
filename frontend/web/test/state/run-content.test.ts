// run 詳細の本文を開いたときに取得する状態遷移と、Route Handler の応答の復元の契約。
//
// 本文は初めて開いたときだけ取得し、取得中・取得済みの項目を開き直しても取得し直さない。
// 取得に失敗した項目は、次に開いたときに取得し直す。
import test from "node:test";
import assert from "node:assert/strict";
import {
  initialRunContentView,
  restoreRunContentResponse,
  runContentFailed,
  runContentLoaded,
  runContentOpened,
  runContentUrl,
} from "../../src/state/run-content.ts";
import { GetRunContentResponse } from "../../src/proto/taktdash.ts";

const BODY = { content: "レビュー本文", sizeBytes: 18, truncated: false };

test("まだ取得していない項目を開くと取得中になり、取得を要求する", () => {
  assert.deepEqual(runContentOpened(initialRunContentView), { view: { status: "loading" }, fetch: true });
});

test("取得中と取得済みの項目を開き直しても、状態を変えず取得を要求しない", () => {
  const loading = { status: "loading" } as const;
  assert.deepEqual(runContentOpened(loading), { view: loading, fetch: false });
  const loaded = runContentLoaded(BODY);
  assert.deepEqual(runContentOpened(loaded), { view: loaded, fetch: false });
});

test("取得に失敗した項目は、次に開いたときに取得し直す", () => {
  const failed = runContentFailed("本文が見つかりません。");
  assert.deepEqual(failed, { status: "failed", error: "本文が見つかりません。" });
  assert.deepEqual(runContentOpened(failed), { view: { status: "loading" }, fetch: true });
});

test("Route Handler の URL は対象に応じて report か instruction の一方だけを付ける", () => {
  assert.equal(
    runContentUrl("run-aaa", { kind: "report", filename: "sub/review.md" }),
    "/api/runs/run-aaa/content?report=sub%2Freview.md",
  );
  assert.equal(runContentUrl("run-aaa", { kind: "instruction", instructionId: 7 }), "/api/runs/run-aaa/content?instruction=7");
});

test("応答の JSON から本文・大きさ・切り詰めの有無を復元する", () => {
  assert.deepEqual(
    restoreRunContentResponse({ runSlug: "run-aaa", content: "本文", sizeBytes: 6, truncated: true }),
    { content: "本文", sizeBytes: 6, truncated: true },
  );
  assert.deepEqual(restoreRunContentResponse({ runSlug: "run-aaa", content: "本文", sizeBytes: "6" }), {
    content: "本文",
    sizeBytes: 6,
    truncated: false,
  });
});

test("本文が空の応答は、proto の JSON 表現が content を省いても空の本文として復元する", () => {
  const empty = { content: "", sizeBytes: 0, truncated: false };
  assert.deepEqual(restoreRunContentResponse({ runSlug: "run-aaa" }), empty);
  assert.deepEqual(
    restoreRunContentResponse(
      GetRunContentResponse.toJSON({ runSlug: "run-aaa", content: "", sizeBytes: 0, truncated: false }),
    ),
    empty,
  );
});

test("本文が文字列でない応答と object でない応答は、表示できない応答として拒否する", () => {
  assert.throws(() => restoreRunContentResponse(null));
  assert.throws(() => restoreRunContentResponse("本文"));
  assert.throws(() => restoreRunContentResponse({ runSlug: "run-aaa", content: 1 }));
  assert.deepEqual(restoreRunContentResponse({ runSlug: "run-aaa", content: "本文" }).content, "本文");
});

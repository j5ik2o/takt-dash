// ゴール詳細の契約（DETAIL-01 から DETAIL-08）。
//
// content は proto の詳細応答を表示し、loader は本番と同じく RSC で取得して
// 404 と接続不可を既存の run 詳細と同じ流儀で扱う。
import test from "node:test";
import assert from "node:assert/strict";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import * as grpc from "@grpc/grpc-js";
import { ExecutionStatus, GoalStatus, TaskStatus } from "../../src/proto/taktdash.ts";
import { createGoalDetailResponse } from "../support/fixtures.ts";
import { reserveClosedAddress, startTaktDashServer } from "../support/grpc-server.ts";
import { formatDateTime } from "../../src/state/format.ts";
import { resetBackendResponded } from "../../src/grpc/client.ts";
import { GoalDetailContent } from "../../src/components/goal-detail-content.tsx";
import { GoalDetailLoader } from "../../src/components/goal-detail-loader.tsx";
import GoalDetailPage from "../../app/goals/[goalId]/page.tsx";
import { closeBackendClient } from "../../src/grpc/client.ts";

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

const renderDetail = (detail = createGoalDetailResponse()): string =>
  renderToStaticMarkup(createElement(GoalDetailContent, { detail }));

test("ゴール詳細は目的・対象外・受け入れ条件・制約を表示する", () => {
  const html = renderDetail();
  assert.match(html, /目的/);
  assert.match(html, /採用ページのレイアウトを直す目的です。/);
  assert.match(html, /対象外/);
  assert.match(html, /管理画面の変更は対象外です。/);
  assert.match(html, /受け入れ条件/);
  assert.match(html, /採用ページのレイアウトが崩れないこと。/);
  assert.match(html, /スマートフォンで読めること。/);
  assert.match(html, /制約/);
  assert.match(html, /既存の配色を変えないこと。/);
});

test("ゴール詳細は空の対象外・受け入れ条件・制約も、空であることが分かる表示にする", () => {
  const html = renderDetail(createGoalDetailResponse({ outOfScope: [], acceptanceCriteria: [], constraints: [] }));
  assert.match(html, /対象外はありません/);
  assert.match(html, /受け入れ条件はありません/);
  assert.match(html, /制約はありません/);
});

test("ゴール詳細は統合ブランチ・ゴールブランチを表示し、proto に無い開始ブランチの行は出さない", () => {
  const html = renderDetail();
  assert.doesNotMatch(html, /開始ブランチ/);
  assert.match(html, /統合ブランチ/);
  assert.match(html, /main/);
  assert.match(html, /ゴールブランチ/);
  assert.match(html, /goal\/45-frontend/);
});

test("ゴール詳細は状態と実行状態が矛盾しない文面を表示する", () => {
  const html = renderDetail();
  assert.match(html, /人のマージ待ち/);
  assert.match(html, /ゴールは人のマージ待ちです/);
  assert.doesNotMatch(html, /稼働中/);
});

test("ゴール詳細は作業記録を応答順のまま時刻と本文つきで表示する", () => {
  const detail = createGoalDetailResponse();
  const html = renderDetail(detail);
  const first = html.indexOf("最初の作業記録です。");
  const second = html.indexOf("次の作業記録です。");
  assert.ok(first >= 0 && second >= 0);
  assert.ok(first < second, "応答順を保つ");
  assert.ok(html.includes(formatDateTime(detail.workLog[0]!.recordedAt!)), "1件目の時刻を表示する");
  assert.ok(html.includes(formatDateTime(detail.workLog[1]!.recordedAt!)), "2件目の時刻を表示する");
});

test("ゴール詳細は manager の質問と回答を表示し、未回答は未回答と分かる表示にする", () => {
  const html = renderDetail();
  assert.match(html, /配色は変えてよいですか。/);
  assert.match(html, /変えないでください。/);
  assert.match(html, /スクリーンショットを撮ってよいですか。/);
  assert.match(html, /未回答/);
});

test("ゴール詳細は回答本文が無くても回答の判断と作業の報告を表示する", () => {
  const detail = createGoalDetailResponse();
  const answered = detail.questions[0]!;
  const html = renderDetail({
    ...detail,
    questions: [
      { ...answered, questionId: "q-decision", body: "配色の変更を許可しますか。", answerText: undefined, answerDecision: "allow", answerReport: undefined },
      { ...answered, questionId: "q-report", body: "スクリーンショットは撮れましたか。", answerText: undefined, answerDecision: undefined, answerReport: "completed" },
    ],
  });
  assert.match(html, /回答の判断は allow です。/);
  assert.match(html, /作業の報告は completed です。/);
  assert.doesNotMatch(html, /未回答/);
});

test("ゴール詳細は取り下げ済みの質問を未回答と表示せず、取り下げとして表示する", () => {
  const detail = createGoalDetailResponse();
  const pending = detail.questions[1]!;
  const html = renderDetail({
    ...detail,
    questions: [
      { ...pending, questionId: "q-withdrawn", body: "この質問は後で出し直します。", status: "withdrawn" },
    ],
  });
  assert.match(html, /取り下げ済み/);
  assert.doesNotMatch(html, /未回答/);
});

test("ゴール詳細は未回答のままの通常の質問を引き続き未回答と表示する", () => {
  const detail = createGoalDetailResponse();
  const pending = detail.questions[1]!;
  const html = renderDetail({
    ...detail,
    questions: [{ ...pending, questionId: "q-pending", status: "pending" }],
  });
  assert.match(html, /未回答/);
  assert.doesNotMatch(html, /取り下げ済み/);
});

test("ゴール詳細はタスク一覧を表示し、タスク名とゴール ID をエスケープしたタスク詳細へのリンクを持つ", () => {
  const detail = createGoalDetailResponse();
  const html = renderDetail(detail);
  const completedHref = `/tasks/corporate-site?task=${encodeURIComponent("採用ページのレイアウトを直す")}&goal=${encodeURIComponent("goal-1")}`;
  const specialHref = `/tasks/corporate-site?task=${encodeURIComponent("タスク/2")}&goal=${encodeURIComponent("goal-1")}`;
  assert.ok(html.includes(`href="${completedHref.replaceAll("&", "&amp;")}"`));
  assert.ok(html.includes(`href="${specialHref.replaceAll("&", "&amp;")}"`));
  assert.match(html, /タスク\/2/);
});

test("ゴール詳細は紐づく run を表示し、run 詳細へのリンクを持つ", () => {
  const html = renderDetail();
  assert.match(html, /href="\/runs\/run-aaa"/);
  assert.match(html, /href="\/runs\/run-bbb"/);
});

test("ゴール詳細ページは params の goalId で本番経路を通り、詳細を描画する", async () => {
  const server = await startTaktDashServer({ goalDetail: createGoalDetailResponse() });
  try {
    await withServerEnv(server.address, async () => {
      const pageElement = GoalDetailPage({ params: Promise.resolve({ goalId: "goal-1" }) });
      assert.equal(pageElement.type, GoalDetailLoader, "page は GoalDetailLoader に params を渡す");
      const loaderElement = await GoalDetailLoader({ params: Promise.resolve({ goalId: "goal-1" }) });
      const html = renderToStaticMarkup(loaderElement);
      assert.match(html, /採用ページのレイアウトを直す目的です。/);
    });
  } finally {
    await server.shutdown();
  }
});

test("見つからない goalId は本番経路で notFound()（404 の digest）に到達する", async () => {
  const server = await startTaktDashServer({
    goalDetail: { code: grpc.status.NOT_FOUND, details: "missing" },
  });
  try {
    await withServerEnv(server.address, async () => {
      await assert.rejects(
        () => GoalDetailLoader({ params: Promise.resolve({ goalId: "no-such-goal" }) }),
        (error: Error & { digest?: string }) => error.digest === NOT_FOUND_DIGEST,
      );
    });
  } finally {
    await server.shutdown();
  }
});

test("応答の goalId が要求と一致しないときも本番経路で notFound()（404 の digest）に到達する", async () => {
  const server = await startTaktDashServer({
    goalDetail: createGoalDetailResponse({ goalId: "goal-other" }),
  });
  try {
    await withServerEnv(server.address, async () => {
      await assert.rejects(
        () => GoalDetailLoader({ params: Promise.resolve({ goalId: "goal-1" }) }),
        (error: Error & { digest?: string }) => error.digest === NOT_FOUND_DIGEST,
      );
    });
  } finally {
    await server.shutdown();
  }
});

test("backend に接続できないときゴール詳細のローダーは落ちず不通の旨を描画する", async () => {
  // このテストは、一度も backend の応答を受けていない Web プロセスを前提にする。
  // 同じファイルの成功ケースで残った接続履歴の影響を受けないよう、前提を明示する。
  // 前に読めたゴールは切断中も内容を残すため（issue #309）、まだ読んでいないゴールを開く。
  resetBackendResponded();
  const closedAddress = await reserveClosedAddress();
  await withServerEnv(closedAddress, async () => {
    const html = renderToStaticMarkup(
      await GoalDetailLoader({ params: Promise.resolve({ goalId: "goal-never-read" }) }),
    );
    assert.match(html, /backend に接続できません/);
    assert.match(html, /TAKTDASH_BACKEND_ADDR/);
  });
});

test("ゴール詳細は完了したゴールに稼働中と出さず、実行中のタスクが残れば件数を出す", () => {
  const completed = renderDetail(
    createGoalDetailResponse({
      status: GoalStatus.GOAL_STATUS_COMPLETED,
      tasks: [{ taskName: "仕上げ", status: TaskStatus.TASK_STATUS_COMPLETED, runSlug: "run-zzz" }],
    }),
  );
  assert.match(completed, /ゴールは完了しています/);
  assert.doesNotMatch(completed, /稼働中/);
  assert.doesNotMatch(completed, /残っています/);

  const withRunning = renderDetail(
    createGoalDetailResponse({
      status: GoalStatus.GOAL_STATUS_COMPLETED,
      tasks: [
        { taskName: "仕上げ", status: TaskStatus.TASK_STATUS_COMPLETED, runSlug: "run-zzz" },
        { taskName: "後片付け", status: TaskStatus.TASK_STATUS_RUNNING, runSlug: "run-yyy" },
      ],
    }),
  );
  assert.match(withRunning, /ゴールは完了しています。実行中のタスクが 1 件残っています/);
  assert.doesNotMatch(withRunning, /稼働中/);
});

test("ゴール詳細は中止したゴールをゴール自体の中止として表示する", () => {
  const html = renderDetail(
    createGoalDetailResponse({
      status: GoalStatus.GOAL_STATUS_CREATED,
      executionStatus: ExecutionStatus.EXECUTION_STATUS_ABORTED,
    }),
  );
  assert.match(html, /ゴールは中止されました/);
  assert.doesNotMatch(html, /作成済みで、実行状態は中止/);
});

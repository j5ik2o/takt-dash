// ゴール系 Route Handler の JSON を画面の内部型へ復元する契約。
//
// 送信側は proto の *Response.toJSON の JSON（時刻は ISO 文字列、列挙型は名称文字列）を送る。
// クライアント側は生成コードの fromJSON を使えないため、必要なフィールドを内部型へ復元する。
import test from "node:test";
import assert from "node:assert/strict";
import {
  GetGoalDetailResponse,
  GetTaskDetailResponse,
} from "../../src/proto/taktdash.ts";
import {
  createGoalDetailResponse,
  createTaskDetailResponse,
} from "../support/fixtures.ts";
import {
  restoreGoalDetail,
  restoreTaskDetail,
} from "../../src/state/restore-state.ts";

test("ゴール詳細の JSON を内部型へ復元し、作業記録・質問・タスクを保つ", () => {
  const wire = GetGoalDetailResponse.toJSON(createGoalDetailResponse());
  const restored = restoreGoalDetail(wire);
  assert.equal(restored.goalId, "goal-1");
  assert.equal(restored.status, 2);
  assert.equal(restored.executionStatus, 1);
  assert.equal(restored.objective, "採用ページのレイアウトを直す目的です。");
  assert.deepEqual(restored.outOfScope, ["管理画面の変更は対象外です。"]);
  assert.deepEqual(restored.acceptanceCriteria, [
    "採用ページのレイアウトが崩れないこと。",
    "スマートフォンで読めること。",
  ]);
  assert.deepEqual(restored.constraints, ["既存の配色を変えないこと。"]);
  assert.equal(restored.branch, "goal/45-frontend");
  assert.equal(restored.integrationBranch, "main");
  assert.ok(restored.workLog[0]!.recordedAt instanceof Date);
  assert.equal(restored.workLog[0]!.summary, "最初の作業記録です。");
  assert.equal(restored.questions[0]!.answerText, "変えないでください。");
  assert.equal(restored.questions[1]!.answerText, undefined);
  assert.equal(restored.tasks[0]!.status, 3);
  assert.equal(restored.tasks[1]!.runSlug, "run-bbb");
});

test("タスク詳細の JSON を内部型へ復元し、開始時刻と列挙型と任意項目を保つ", () => {
  const wire = GetTaskDetailResponse.toJSON(createTaskDetailResponse());
  const restored = restoreTaskDetail(wire);
  assert.equal(restored.project, "corporate-site");
  assert.equal(restored.taskName, "採用ページのレイアウトを直す");
  assert.equal(restored.goalId, "goal-1");
  assert.equal(restored.status, 3);
  assert.ok(restored.startedAt instanceof Date);
  assert.equal(restored.pullRequestNumber, 418);
  assert.equal(restored.branch, "goal/45-frontend");
  assert.equal(restored.pullRequestUrl, "https://example.test/pr/418");
  assert.deepEqual(restored.linkedRunSlugs, ["run-aaa", "run-bbb"]);
});

test("タスク詳細の任意項目が省略された JSON は undefined のまま復元し、undefined 文字列を作らない", () => {
  const wire = GetTaskDetailResponse.toJSON(
    createTaskDetailResponse({
      startedAt: undefined,
      branch: undefined,
      pullRequestNumber: undefined,
      pullRequestUrl: undefined,
      goalId: undefined,
      runSlug: undefined,
      linkedRunSlugs: [],
    }),
  );
  const restored = restoreTaskDetail(wire);
  assert.equal(restored.startedAt, undefined);
  assert.equal(restored.branch, undefined);
  assert.equal(restored.pullRequestNumber, undefined);
  assert.equal(restored.pullRequestUrl, undefined);
  assert.equal(restored.goalId, undefined);
  assert.equal(restored.runSlug, undefined);
  assert.deepEqual(restored.linkedRunSlugs, []);
});

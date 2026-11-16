// TASK_STATUS の数値が生成コードの TaskStatus と一致することを確かめる。
//
// サーバーカード下端の「進行中のゴール」は、仕上げタスクの最後の状態
// （mergeTaskLast）が未設定かどうかで人のマージ待ちを判定する。数値がずれると
// 進行中のゴール数が誤るため、生成コードの enum と突き合わせて検出する。
import test from "node:test";
import assert from "node:assert/strict";
import { TaskStatus } from "../../src/proto/taktdash.ts";
import { TASK_STATUS } from "../../src/state/proto-values.ts";

test("TASK_STATUS は生成コードの TaskStatus と一致する", () => {
  assert.equal(TASK_STATUS.PENDING, TaskStatus.TASK_STATUS_PENDING);
  assert.equal(TASK_STATUS.RUNNING, TaskStatus.TASK_STATUS_RUNNING);
  assert.equal(TASK_STATUS.COMPLETED, TaskStatus.TASK_STATUS_COMPLETED);
  assert.equal(TASK_STATUS.FAILED, TaskStatus.TASK_STATUS_FAILED);
  assert.equal(TASK_STATUS.EXCEEDED, TaskStatus.TASK_STATUS_EXCEEDED);
  assert.equal(TASK_STATUS.PR_FAILED, TaskStatus.TASK_STATUS_PR_FAILED);
});

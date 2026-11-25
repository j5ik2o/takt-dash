// 最後に読めた内容の保持（issue #309）。run やゴールは増え続けるため、上限を超えたら
// いちばん長く使っていないものから捨てる。
import test from "node:test";
import assert from "node:assert/strict";
import { createLastLoaded } from "../../src/state/last-loaded.ts";

test("上限を超えると、いちばん長く使っていない内容から捨てる", () => {
  const last = createLastLoaded<string>(2);
  last.remember("a", "A");
  last.remember("b", "B");
  assert.equal(last.recall("a"), "A");
  last.remember("c", "C");
  assert.equal(last.recall("b"), undefined, "読み返していない b を捨てる");
  assert.equal(last.recall("a"), "A");
  assert.equal(last.recall("c"), "C");
});

test("同じキーを覚え直すと、新しい内容に置き換える", () => {
  const last = createLastLoaded<string>(2);
  last.remember("a", "A1");
  last.remember("a", "A2");
  assert.equal(last.recall("a"), "A2");
});

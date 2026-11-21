import test from "node:test";
import assert from "node:assert/strict";
import { readCountText, readCoverage } from "../../src/state/read-coverage.ts";
import { NOW } from "../support/fixtures.ts";

test("取得実績のない空集合と初回成功前は未読とし、正常取得した空集合を区別する", () => {
  assert.equal(readCoverage([]), "unread");
  assert.equal(readCoverage([], true), "complete");
  assert.equal(readCoverage([{ lastReadAt: undefined }], true), "unread");
  assert.equal(readCoverage([{ lastReadAt: undefined }]), "unread");
});

test("成功時刻の有無が混在する範囲とすべて成功した範囲を区別する", () => {
  assert.equal(readCoverage([{ lastReadAt: NOW }, { lastReadAt: undefined }]), "partial");
  assert.equal(readCoverage([{ lastReadAt: NOW }, { lastReadAt: NOW }]), "complete");
});

test("未読ではゼロを確定せず、既読のゼロと部分的に読めた件数を区別する", () => {
  assert.equal(readCountText(0, "unread"), "不明");
  assert.equal(readCountText(0, "complete"), "0件");
  assert.equal(readCountText(0, "partial"), "読めた分 0件");
  assert.equal(readCountText(3, "partial"), "読めた分 3件");
  assert.equal(readCountText(3, "complete"), "3件");
});

// 画面側のドメインプリミティブ（C01〜C06）の契約。
//
// 型の生成は size → lexical content → syntax → semantics の順に行い、検証を通らない
// 値からは生成できない。ここでは各段の拒否と、通常の値の受理を観測する。
import test from "node:test";
import assert from "node:assert/strict";
import {
  DomainPrimitiveError,
  toGoalId,
  toInquiryId,
  toProjectName,
  toPullRequestNumber,
  toRegistrationRevision,
  toRunSlug,
  toServerId,
  toSilentMinutes,
  toStaleMinutes,
  toTaskCount,
  toTaskName,
} from "../../src/state/domain-primitives.ts";

type Stage = "size" | "lexical" | "syntax" | "semantics";

const stageOf = (build: () => unknown): Stage => {
  try {
    build();
  } catch (error) {
    assert.ok(
      error instanceof DomainPrimitiveError,
      "検証に失敗した値は DomainPrimitiveError で表す",
    );
    return error.stage;
  }
  assert.fail("検証を通らない値からは生成できません");
};

const TEXT_TYPES: Array<{ name: string; build: (value: string) => unknown }> = [
  { name: "ServerId", build: toServerId },
  { name: "ProjectName", build: toProjectName },
  { name: "GoalId", build: toGoalId },
  { name: "TaskName", build: toTaskName },
  { name: "InquiryId", build: toInquiryId },
];

test("テキストの値は空を size、空白のみと ASCII 制御文字を lexical で拒否する", () => {
  for (const { name, build } of TEXT_TYPES) {
    assert.equal(stageOf(() => build("")), "size", `${name} は空を拒否する`);
    assert.equal(stageOf(() => build("   ")), "lexical", `${name} は空白のみを拒否する`);
    assert.equal(stageOf(() => build("a\nb")), "lexical", `${name} は改行を拒否する`);
    assert.equal(stageOf(() => build("a\u007fb")), "lexical", `${name} は DEL を拒否する`);
    assert.equal(build("mac-mini"), "mac-mini", `${name} は通常の値を保持する`);
  }
});

test("run slug はテキスト検証の後にパス区切りと親ディレクトリを syntax で拒否する", () => {
  assert.equal(toRunSlug("20261116-run"), "20261116-run");
  assert.equal(stageOf(() => toRunSlug("")), "size");
  assert.equal(stageOf(() => toRunSlug("  ")), "lexical");
  assert.equal(stageOf(() => toRunSlug("a/b")), "syntax");
  assert.equal(stageOf(() => toRunSlug("a\\b")), "syntax");
  assert.equal(stageOf(() => toRunSlug(".")), "syntax");
  assert.equal(stageOf(() => toRunSlug("..")), "syntax");
  assert.equal(stageOf(() => toRunSlug("../x")), "syntax");
});

test("プルリクエスト番号は正の安全な整数だけを受理する", () => {
  assert.equal(toPullRequestNumber(418), 418);
  assert.equal(stageOf(() => toPullRequestNumber(0)), "size");
  assert.equal(stageOf(() => toPullRequestNumber(-1)), "size");
  assert.equal(stageOf(() => toPullRequestNumber(1.5)), "syntax");
  assert.equal(stageOf(() => toPullRequestNumber("418")), "lexical");
  assert.equal(stageOf(() => toPullRequestNumber(Number.NaN)), "lexical");
  assert.equal(stageOf(() => toPullRequestNumber(Number.POSITIVE_INFINITY)), "lexical");
});

test("承認待ちの revision は正の i32 だけを受理する", () => {
  assert.equal(toRegistrationRevision(1), 1);
  assert.equal(toRegistrationRevision(2_147_483_647), 2_147_483_647);
  assert.equal(stageOf(() => toRegistrationRevision(0)), "size");
  assert.equal(stageOf(() => toRegistrationRevision(-1)), "size");
  assert.equal(stageOf(() => toRegistrationRevision(2_147_483_648)), "size");
  assert.equal(stageOf(() => toRegistrationRevision(1.5)), "syntax");
  assert.equal(stageOf(() => toRegistrationRevision("1")), "lexical");
});

test("タスク件数は 0 以上 i32::MAX 以下だけを受理する", () => {
  assert.equal(toTaskCount(0), 0);
  assert.equal(toTaskCount(2_147_483_647), 2_147_483_647);
  assert.equal(stageOf(() => toTaskCount(-1)), "size");
  assert.equal(stageOf(() => toTaskCount(2_147_483_648)), "size");
  assert.equal(stageOf(() => toTaskCount(1.5)), "syntax");
  assert.equal(stageOf(() => toTaskCount("1")), "lexical");
});

test("滞留と無音の閾値は 0 以上 i32::MAX 以下だけを受理する", () => {
  assert.equal(toStaleMinutes(240), 240);
  assert.equal(toSilentMinutes(10), 10);
  assert.equal(toStaleMinutes(0), 0);
  assert.equal(toSilentMinutes(0), 0);
  for (const [name, build] of [
    ["StaleMinutes", toStaleMinutes],
    ["SilentMinutes", toSilentMinutes],
  ] as const) {
    assert.equal(stageOf(() => build(-1)), "size", `${name} は負数を拒否する`);
    assert.equal(stageOf(() => build(2_147_483_648)), "size", `${name} は i32 の上限超を拒否する`);
    assert.equal(stageOf(() => build(1.5)), "syntax", `${name} は小数を拒否する`);
    assert.equal(stageOf(() => build("10")), "lexical", `${name} は数値以外を拒否する`);
  }
});

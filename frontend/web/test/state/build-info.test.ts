// frontend のビルド情報の契約（issue #73 / AC1）。
//
// ビルド時に git から短縮コミット SHA とブランチ名を取得して埋め込み、取得できない
// 環境でも "unknown" に正規化してビルドを壊さない。frontend と backend のコミットが
// 異なる場合だけ不一致として扱う。
//
// `readGitBuildInfo` は git の実行関数を注入して受け取る。next.config.ts は
// execFileSync を注入し、テストは決定的な runner を注入する。
import test from "node:test";
import assert from "node:assert/strict";
import {
  commitsDiffer,
  normalizeBuildInfo,
  readGitBuildInfo,
  type BuildInfo,
  type GitRunner,
} from "../../src/state/build-info.ts";
import nextConfig from "../../next.config.ts";

const UNKNOWN = "unknown";

// 短縮 SHA は `--short`、ブランチ名は `--abbrev-ref` の問い合わせで得る。
// それ以外の引数で git を呼んだら、契約違反としてテストを失敗させる。
const runnerReturning = (commitOutput: string, branchOutput: string): GitRunner => (args) => {
  if (args.includes("--abbrev-ref")) {
    return branchOutput;
  }
  if (args.includes("--short")) {
    return commitOutput;
  }
  throw new Error(`想定していない git の引数です: ${args.join(" ")}`);
};

test("ビルド時に git から取得した短縮 SHA とブランチ名を前後の空白を除いて返す", () => {
  const info = readGitBuildInfo(runnerReturning("a1b2c3d\n", "takt/20261116-build-info\n"));

  assert.deepEqual(info, { commit: "a1b2c3d", branch: "takt/20261116-build-info" });
});

test("git が失敗しても例外にせず、コミットとブランチの両方を unknown にする", () => {
  const failingRunner: GitRunner = () => {
    throw new Error("git が使えない環境です");
  };

  const info = readGitBuildInfo(failingRunner);

  assert.deepEqual(info, { commit: UNKNOWN, branch: UNKNOWN });
});

test("git の出力が空でもコミットとブランチの両方を unknown にする", () => {
  const info = readGitBuildInfo(runnerReturning("", ""));

  assert.deepEqual(info, { commit: UNKNOWN, branch: UNKNOWN });
});

test("空文字・空白・HEAD・不正なコミットは unknown に正規化する", () => {
  assert.deepEqual(normalizeBuildInfo({ commit: "", branch: "" }), { commit: UNKNOWN, branch: UNKNOWN });
  assert.deepEqual(normalizeBuildInfo({ commit: "   ", branch: "   " }), { commit: UNKNOWN, branch: UNKNOWN });
  assert.deepEqual(normalizeBuildInfo({ commit: "HEAD", branch: "HEAD" }), { commit: UNKNOWN, branch: UNKNOWN });
  assert.deepEqual(normalizeBuildInfo({ commit: "not-a-sha", branch: "main" }), {
    commit: UNKNOWN,
    branch: "main",
  });
});

test("取得できた短縮 SHA とブランチ名はそのまま表示に使う", () => {
  assert.deepEqual(normalizeBuildInfo({ commit: "a1b2c3d", branch: "takt/20261116-build-info" }), {
    commit: "a1b2c3d",
    branch: "takt/20261116-build-info",
  });
});

test("ビルド情報が無い場合はコミットとブランチの両方を unknown にする", () => {
  assert.deepEqual(normalizeBuildInfo(undefined), { commit: UNKNOWN, branch: UNKNOWN });
});

test("両方のコミットが取得できていて異なるときだけ不一致とする", () => {
  const frontend: BuildInfo = { commit: "a1b2c3d", branch: "takt/frontend" };
  const same: BuildInfo = { commit: "a1b2c3d", branch: "takt/backend" };
  const different: BuildInfo = { commit: "d4e5f6a", branch: "takt/backend" };
  const unknown: BuildInfo = { commit: UNKNOWN, branch: "takt/backend" };

  assert.equal(commitsDiffer(frontend, different), true, "両方取得できていて異なれば不一致");
  assert.equal(commitsDiffer(frontend, same), false, "同じコミットは不一致ではない");
  assert.equal(commitsDiffer(frontend, unknown), false, "backend が unknown なら判定しない");
  assert.equal(commitsDiffer(unknown, different), false, "frontend が unknown なら判定しない");
});

test("next.config はビルド時の git の値を env へ埋め込む", () => {
  const env = (nextConfig as { env?: Record<string, string | undefined> }).env;

  assert.ok(env, "next.config はビルド時の値を env に持つ必要があります");
  assert.match(
    env.TAKTDASH_FRONTEND_COMMIT ?? "",
    /^(unknown|[0-9a-f]{7,40})$/i,
    "コミットは短縮 SHA か unknown を埋め込む",
  );
  assert.ok(
    (env.TAKTDASH_FRONTEND_BRANCH ?? "").length > 0,
    "ブランチ名は空でない値（取得できない場合は unknown）を埋め込む",
  );
});


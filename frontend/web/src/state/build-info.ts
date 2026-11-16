// frontend のビルド元の情報（issue #73 / AC1）。
//
// ビルド時に git から短縮コミット SHA とブランチ名を取得して埋め込み、取得できない
// 環境でも "unknown" に正規化してビルドを壊さない。取得はビルド設定から注入した
// 実行関数（runner）に任せ、テストでは決定的な runner を渡す。

export type BuildInfo = {
  commit: string;
  branch: string;
};

/** git を引数つきで実行し、標準出力を返す関数。 */
export type GitRunner = (args: string[]) => string;

const UNKNOWN = "unknown";

const isCommit = (value: string): boolean => /^[0-9a-f]{7,40}$/i.test(value);

/** git の値を表示用に正規化する。不正な値と欠落は `unknown` にする。 */
export function normalizeBuildInfo(raw: { commit?: string; branch?: string } | undefined): BuildInfo {
  const commit = raw?.commit?.trim() ?? "";
  const branch = raw?.branch?.trim() ?? "";
  return {
    commit: isCommit(commit) ? commit : UNKNOWN,
    branch: branch !== "" && branch.toUpperCase() !== "HEAD" ? branch : UNKNOWN,
  };
}

/** ビルド時に git から短縮 SHA とブランチ名を取得する。失敗しても例外にしない。 */
export function readGitBuildInfo(runner: GitRunner): BuildInfo {
  const read = (args: string[]): string => {
    try {
      return runner(args);
    } catch {
      return "";
    }
  };
  return normalizeBuildInfo({
    commit: read(["rev-parse", "--short", "HEAD"]),
    branch: read(["rev-parse", "--abbrev-ref", "HEAD"]),
  });
}

const isKnown = (commit: string): boolean => commit !== UNKNOWN && commit.trim() !== "";

/** 両方のコミットが取得できていて異なるときだけ不一致とみなす。 */
export function commitsDiffer(a: BuildInfo, b: BuildInfo): boolean {
  return isKnown(a.commit) && isKnown(b.commit) && a.commit !== b.commit;
}

import { execFileSync } from "node:child_process";
import type { NextConfig } from "next";
import { readGitBuildInfo } from "./src/state/build-info";

// ビルド元の短縮 SHA とブランチ名をビルド時に一度だけ git から取得し、env に埋め込む。
// 取得できない環境では "unknown" になり、ビルドを壊さない。
const buildInfo = readGitBuildInfo((args) =>
  execFileSync("git", args, { cwd: process.cwd(), encoding: "utf8" }),
);

const nextConfig: NextConfig = {
  // gRPC はネイティブ実装をサーバー側だけで動かす。サーバーのバンドルからは外す。
  serverExternalPackages: ["@grpc/grpc-js"],
  env: {
    TAKTDASH_FRONTEND_COMMIT: buildInfo.commit,
    TAKTDASH_FRONTEND_BRANCH: buildInfo.branch,
  },
};

export default nextConfig;

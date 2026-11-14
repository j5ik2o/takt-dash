import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  // gRPC はネイティブ実装をサーバー側だけで動かす。サーバーのバンドルからは外す。
  serverExternalPackages: ["@grpc/grpc-js"],
};

export default nextConfig;

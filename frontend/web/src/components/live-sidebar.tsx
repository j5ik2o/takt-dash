"use client";

import type { ReactElement } from "react";
import { usePathname } from "next/navigation";
import { Sidebar } from "./sidebar";
import { useLiveState } from "./state-provider";
import { normalizeBuildInfo, type BuildInfo } from "../state/build-info";

type LiveSidebarProps = {
  frontendBuild: BuildInfo;
};

// 自動更新の状態からサイドバーを描画する。初回は RSC が渡した状態を使い、
// SSE を受信するたびにサーバー一覧と判断待ち件数を最新の状態へ追従させます。
// backend のビルド情報は状態から、frontend のビルド情報はビルド時に埋め込んだ値を
// prop で受け取り、サイドバー下端に表示します。
export function LiveSidebar({ frontendBuild }: LiveSidebarProps): ReactElement {
  const { state, snapshotLoaded } = useLiveState();
  const pathname = usePathname();
  return (
    <Sidebar
      snapshotLoaded={snapshotLoaded}
      servers={state.servers}
      pendingCount={state.pendingDecisions.length}
      currentPath={pathname ?? undefined}
      readAt={state.readAt}
      runs={state.runs}
      projects={state.projects}
      frontendBuild={frontendBuild}
      backendBuild={normalizeBuildInfo(state.buildInfo)}
    />
  );
}

"use client";

import type { ReactElement } from "react";
import { usePathname } from "next/navigation";
import { Sidebar } from "./sidebar";
import { useLiveState } from "./state-provider";

// 自動更新の状態からサイドバーを描画する。初回は RSC が渡した状態を使い、
// SSE を受信するたびにサーバー一覧と判断待ち件数を最新の状態へ追従させます。
// 現在のパスは usePathname で受け取り、遷移のたびに現在ページの強調を更新します。
export function LiveSidebar(): ReactElement {
  const { state } = useLiveState();
  const pathname = usePathname();
  return (
    <Sidebar
      servers={state.servers}
      pendingCount={state.pendingDecisions.length}
      currentPath={pathname ?? undefined}
    />
  );
}

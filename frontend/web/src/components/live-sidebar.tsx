"use client";

import type { ReactElement } from "react";
import { usePathname } from "next/navigation";
import type { Server } from "../proto/taktdash";
import { Sidebar } from "./sidebar";

type LiveSidebarProps = {
  servers: Server[];
  pendingCount: number;
};

// サイドバーは layout が保持し、ページ遷移でも同じ実体のまま残る。現在のパスを
// usePathname で受け取り、遷移のたびに現在ページの強調を更新する。
export function LiveSidebar({ servers, pendingCount }: LiveSidebarProps): ReactElement {
  const pathname = usePathname();
  return <Sidebar servers={servers} pendingCount={pendingCount} currentPath={pathname} />;
}

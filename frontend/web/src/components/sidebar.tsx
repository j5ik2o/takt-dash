import type { ReactElement } from "react";
import Link from "next/link";
import type { Server } from "../proto/taktdash";

type SidebarProps = {
  servers: Server[];
  pendingCount: number;
};

const PAGES = [
  { href: "/", label: "概要" },
  { href: "/decisions", label: "判断待ち" },
  { href: "/projects", label: "プロジェクト" },
  { href: "/goals", label: "ゴール" },
  { href: "/activity", label: "アクティビティ" },
  { href: "/conversations", label: "会話" },
] as const;

export function Sidebar({ servers, pendingCount }: SidebarProps): ReactElement {
  return (
    <nav className="sidebar" aria-label="画面と集約対象のサーバー">
      <p className="brand">taktDash</p>
      <div className="side-block">
        <p className="side-title">サーバー</p>
        <ul className="srv-list">
          {servers.map((server) => (
            <li key={server.serverId}>
              <span className={`dot ${server.connected ? "live" : "down"}`} aria-hidden="true" />
              <span className="nm">{server.name}</span>
              <span className="srv-state">{server.connected ? "動作中" : "応答なし"}</span>
            </li>
          ))}
        </ul>
      </div>
      <div className="side-block">
        <p className="side-title">ページ</p>
        <ul className="nav">
          {PAGES.map((page) => (
            <li key={page.href}>
              <Link href={page.href}>{page.label}</Link>
              {page.href === "/decisions" && pendingCount > 0 ? <span className="badge">{pendingCount}件</span> : null}
            </li>
          ))}
        </ul>
      </div>
    </nav>
  );
}

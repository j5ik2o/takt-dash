import type { ReactElement } from "react";
import Link from "next/link";
import type { Server } from "../proto/taktdash";
import { serverHref } from "../state/anchors";

type SidebarProps = {
  servers: Server[];
  pendingCount: number;
  currentPath?: string;
};

const PAGES = [
  { href: "/", label: "概要" },
  { href: "/decisions", label: "判断待ち" },
  { href: "/projects", label: "プロジェクト" },
  { href: "/activity", label: "アクティビティ" },
] as const;

// 現在のパスと一致するナビだけを現在ページにする。前方一致にすると "/" が
// すべてのページで一致してしまうため、完全一致で判定する。
export const isCurrentPath = (currentPath: string | undefined, href: string): boolean =>
  currentPath !== undefined && currentPath === href;

export function Sidebar({ servers, pendingCount, currentPath }: SidebarProps): ReactElement {
  return (
    <nav className="sidebar" aria-label="画面と集約対象のサーバー">
      <Link className="brand" href="/">
        taktDash
      </Link>
      <div className="side-block">
        <p className="side-title">サーバー</p>
        <ul className="srv-list">
          {servers.map((server) => (
            <li key={server.serverId}>
              <span className={`dot ${server.connected ? "live" : "down"}`} aria-hidden="true" />
              <Link className="nm" href={serverHref(server.serverId)}>
                {server.name}
              </Link>
              <span className="srv-state">{server.connected ? "動作中" : "応答なし"}</span>
            </li>
          ))}
        </ul>
      </div>
      <div className="side-block">
        <p className="side-title">ページ</p>
        <ul className="nav">
          {PAGES.map((page) => {
            const current = isCurrentPath(currentPath, page.href);
            return (
              <li key={page.href}>
                <Link
                  href={page.href}
                  className={current ? "on" : undefined}
                  aria-current={current ? "page" : undefined}
                >
                  {page.label}
                </Link>
                {page.href === "/decisions" && pendingCount > 0 ? <span className="badge">{pendingCount}件</span> : null}
              </li>
            );
          })}
        </ul>
      </div>
    </nav>
  );
}

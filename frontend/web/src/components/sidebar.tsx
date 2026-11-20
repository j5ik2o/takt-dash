import type { ReactElement } from "react";
import Link from "next/link";
import type { Server } from "../proto/taktdash";
import type { DashboardRun } from "../state/restore-state";
import { serverHref } from "../state/anchors";
import { commitsDiffer, type BuildInfo } from "../state/build-info";
import { formatClock } from "../state/format";

type SidebarProps = {
  servers: Server[];
  pendingCount: number;
  currentPath?: string;
  readAt?: Date;
  runs?: readonly DashboardRun[];
  frontendBuild: BuildInfo;
  backendBuild: BuildInfo;
};

const PAGES = [
  { href: "/", label: "概要" },
  { href: "/decisions", label: "判断待ち" },
  { href: "/projects", label: "プロジェクト" },
  { href: "/goals", label: "ゴール" },
  { href: "/activity", label: "アクティビティ" },
  { href: "/conversations", label: "会話" },
] as const;

// 現在ページの判定に使う先頭セグメントを取り出す。クエリやハッシュは無視し、
// 先頭の空でないセグメントだけを見る。"goals" と "goalsx" は別セグメントになる。
const firstSegment = (path: string): string => path.split(/[/?#]/).filter((part) => part !== "")[0] ?? "";

// パスをナビの項目へ対応付ける。ゴール配下のタスク詳細（"/tasks/..."）は
// ゴールの項目（"/goals"）として示す。それ以外は先頭セグメントで決める。
const navSegmentOf = (path: string): string => {
  const segment = firstSegment(path);
  return segment === "tasks" ? "goals" : segment;
};

// 現在のパスがナビの項目に対応するかを判定する。完全一致に加え、先頭セグメントが
// 一致する詳細ページ（"/goals/goal-1" は "/goals"）も現在ページにする。"/" は
// 完全一致したときだけ対象にし、"/runs/..." のようにナビに対応が無いパスは
// どの項目にも一致させない。
export const isCurrentPath = (currentPath: string | undefined, href: string): boolean => {
  if (currentPath === undefined) {
    return false;
  }
  if (currentPath === href) {
    return true;
  }
  if (href === "/") {
    return false;
  }
  const currentSegment = navSegmentOf(currentPath);
  return currentSegment !== "" && currentSegment === navSegmentOf(href);
};

export function Sidebar({
  servers,
  pendingCount,
  currentPath,
  readAt,
  runs,
  frontendBuild,
  backendBuild,
}: SidebarProps): ReactElement {
  const connectedCount = servers.filter((server) => server.connected).length;
  return (
    <nav className="sidebar" aria-label="画面と集約対象のサーバー">
      <Link className="brand" href="/">
        taktDash
      </Link>
      <div className="side-block">
        <p className="side-title">サーバー</p>
        <ul className="srv-list">
          {servers.map((server) => {
            const runCount = runs?.filter((run) => run.serverId === server.serverId).length ?? 0;
            return (
              <li key={server.serverId}>
                <span className={`dot ${server.connected ? "live" : "down"}`} aria-hidden="true" />
                <Link className="nm" href={serverHref(server.serverId)}>
                  {server.name}
                </Link>
                <span className="cnt">run {runCount}件</span>
                <span className="srv-state">{server.connected ? "動作中" : "応答なし"}</span>
              </li>
            );
          })}
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
      <details className="side-foot">
        <summary>詳細</summary>
        <p>最終読み取り時刻 {readAt ? formatClock(readAt) : "不明"}</p>
        <p>
          {servers.length}台中{connectedCount}台から読めています
        </p>
        <p style={{ overflowWrap: "anywhere" }}>
          frontend {frontendBuild.commit} ({frontendBuild.branch})
        </p>
        <p style={{ overflowWrap: "anywhere" }}>
          backend {backendBuild.commit} ({backendBuild.branch})
        </p>
        {commitsDiffer(frontendBuild, backendBuild) ? <p>コミットが不一致です</p> : null}
      </details>
    </nav>
  );
}

import type { ReactElement } from "react";
import Link from "next/link";
import type { Project, Server } from "../proto/taktdash";
import type { DashboardRun } from "../state/restore-state";
import { serverHref } from "../state/anchors";
import { commitsDiffer, type BuildInfo } from "../state/build-info";
import { formatClock } from "../state/format";
import {
  notRespondingProjectText,
  notRespondingProjects,
  serverDisplayState,
  serverStateLabel,
} from "../state/dashboard";
import { toRunCount } from "../state/domain-primitives";
import { readCountText, readCoverage } from "../state/read-coverage";

type SidebarProps = {
  servers: Server[];
  pendingCount: number;
  currentPath?: string;
  readAt?: Date;
  runs?: readonly DashboardRun[];
  // サーバー配下のプロジェクト。サーバーが応答していても配下のプロジェクトが
  // 応答していないことがあるため、状態と下端の注記に使う（issue #264）。
  projects?: readonly Project[];
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

// 現在のパスと一致するナビだけを現在ページにする。前方一致にすると "/" が
// すべてのページで一致してしまうため、完全一致で判定する。
export const isCurrentPath = (currentPath: string | undefined, href: string): boolean =>
  currentPath !== undefined && currentPath === href;

export function Sidebar({
  servers,
  pendingCount,
  currentPath,
  readAt,
  runs,
  projects,
  frontendBuild,
  backendBuild,
}: SidebarProps): ReactElement {
  const connectedCount = servers.filter((server) => server.connected).length;
  const notResponding = notRespondingProjects(projects ?? []);
  return (
    <nav className="sidebar" aria-label="画面と集約対象のサーバー">
      <Link className="brand" href="/">
        taktDash
      </Link>
      <div className="side-block">
        <p className="side-title">サーバー</p>
        <ul className="srv-list">
          {servers.map((server) => {
            const runCount = toRunCount(
              runs?.filter((run) => run.serverId === server.serverId).length ?? 0,
            );
            // 状態の決め方と文言は概要のサーバーカードと共通の関数を使う。
            // 接続だけで決めると、run が無いサーバーがカードの表示と食い違う（issue #188）。
            // 配下のプロジェクトが応答していないサーバーは、接続できていても正常に
            // 見えないようにカードと同じく warn の側へ倒す（issue #264）。
            const state = serverDisplayState(server, runCount, projects ?? []);
            const ownProjects = projects?.filter((project) => project.serverId === server.serverId);
            const countText = ownProjects?.length === 0
              ? "対象なし"
              : readCountText(runCount, readCoverage(ownProjects ?? [server]));
            return (
              <li key={server.serverId}>
                <span className={`dot ${state}`} aria-hidden="true" />
                <Link className="nm" href={serverHref(server.serverId)}>
                  {server.name}
                </Link>
                <span className="cnt">run {countText}</span>
                <span className="srv-state">{serverStateLabel(state)}</span>
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
      <div className="side-foot">
        <p>最終読み取り時刻 {readAt ? formatClock(readAt) : "不明"}</p>
        <p>
          {servers.length}台中{connectedCount}台から読めています
        </p>
        {notResponding.map((project) => (
          <p className="stale-note" key={`${project.serverId}:${project.name}`}>
            {notRespondingProjectText(project)}
          </p>
        ))}
        <p style={{ overflowWrap: "anywhere" }}>
          frontend {frontendBuild.commit} ({frontendBuild.branch})
        </p>
        <p style={{ overflowWrap: "anywhere" }}>
          backend {backendBuild.commit} ({backendBuild.branch})
        </p>
        {commitsDiffer(frontendBuild, backendBuild) ? <p>コミットが不一致です</p> : null}
      </div>
    </nav>
  );
}

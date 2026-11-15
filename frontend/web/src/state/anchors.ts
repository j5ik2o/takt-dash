// 概要・プロジェクト・サイドバーの間で共有するアンカー id と href の規則。
//
// リンクを生成する側と、リンク先の id を付ける側が同じ規則を使うように集約する。
// Next.js は hash を decode して getElementById で要素を探すため、id は生の名前から作り、
// href でも同じ値を使う。
const PROJECT_PREFIX = "project-";
const SERVER_PREFIX = "server-";

export function projectAnchorId(name: string): string {
  return `${PROJECT_PREFIX}${name}`;
}

export function projectHref(name: string): string {
  return `/projects#${projectAnchorId(name)}`;
}

export function serverAnchorId(serverId: string): string {
  return `${SERVER_PREFIX}${serverId}`;
}

export function serverHref(serverId: string): string {
  return `/#${serverAnchorId(serverId)}`;
}

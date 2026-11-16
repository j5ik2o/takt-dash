// 概要・プロジェクト・サイドバーの間で共有するアンカー id と href の規則。
//
// リンクを生成する側と、リンク先の id を付ける側が同じ規則を使うように集約する。
// Next.js は hash を decode して getElementById で要素を探すため、id は生の名前から作り、
// href でも同じ値を使う。
const PROJECT_PREFIX = "project-";
const SERVER_PREFIX = "server-";
const DECISION_PREFIX = "decision-";

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

// 判断待ちの1項目を指す id。key は同じ項目を再描画後も識別する鍵（decision-actions の
// decisionKey）で、リンクと一覧の id の両方をこの規則から作る。この共有層は判断待ち機能の
// 型・キー実装へ依存しないよう、key を文字列として受け取る。decision- 接頭辞で既存の
// 見出し id と分ける。
export function decisionAnchorId(key: string): string {
  return `${DECISION_PREFIX}${key}`;
}

export function decisionHref(key: string): string {
  return `/decisions#${decisionAnchorId(key)}`;
}

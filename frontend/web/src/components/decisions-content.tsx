import type { ReactElement } from "react";
import type { DashboardState, InquiryKind, PendingDecision } from "../proto/taktdash";
import { INQUIRY_KIND, PENDING_DECISION_TYPE } from "../state/proto-values";
import { sortPendingDecisions, resolveThresholds, type PendingDecisionView } from "../state/dashboard";
import { formatDuration } from "../state/format";

type DecisionsContentProps = {
  state: DashboardState;
  now: Date;
};

const inquiryKindLabel = (kind: InquiryKind): string => {
  switch (kind) {
    case INQUIRY_KIND.OPERATION_REQUEST:
      return "作業の依頼";
    case INQUIRY_KIND.QUESTION:
    default:
      return "質問";
  }
};

const decisionKindLabel = (decision: PendingDecision): string =>
  decision.type === PENDING_DECISION_TYPE.AWAITING_MERGE
    ? "人のマージ待ち"
    : `問い合わせ（${inquiryKindLabel(decision.inquiryKind)}）`;

const targetLabel = (decision: PendingDecision): string | undefined => {
  if (decision.pullRequest !== undefined) {
    return `プルリクエスト #${decision.pullRequest.number}`;
  }
  if (decision.goalId !== undefined) {
    return `ゴール ${decision.goalId}`;
  }
  return undefined;
};

const waitSeconds = (decision: PendingDecisionView): number => Math.round(decision.waitMinutes * 60);

// 本文は takt-mcp の原文をそのまま運ぶ。空行を段落の区切りとして扱い、段落内の改行は
// 空白へ畳まずに残す（issue #37 受け入れ条件1）。
const splitBodyParagraphs = (body: string): string[] =>
  body
    .replace(/\r\n?/g, "\n")
    .split(/\n[ \t]*\n+/)
    .map((paragraph) => paragraph.trim())
    .filter((paragraph) => paragraph !== "");

// 判断待ちは表示のみとする（order.md:25, :51）。回答・承認・却下の操作要素を持たない。
export function DecisionsContent({ state, now }: DecisionsContentProps): ReactElement {
  const thresholds = resolveThresholds(state.thresholds);
  const decisions = sortPendingDecisions(state.pendingDecisions, now, {
    staleThresholdMinutes: thresholds.pendingDecisionStaleMinutes,
  });
  const serverNames = new Map(state.servers.map((server) => [server.serverId, server.name]));

  return (
    <main className="page">
      <header className="page-head">
        <div>
          <h1>判断待ち</h1>
          <p>
            全サーバーで、人の判断を待っている項目が {decisions.length}件あります。待ち時間の長い順に並べています。
          </p>
        </div>
      </header>

      <section className="tray" aria-label="判断待ちの一覧">
        {decisions.length === 0 ? (
          <p className="note">判断待ちはありません。</p>
        ) : (
          <ul className="tray-list">
            {decisions.map((decision, index) => {
              // サーバー一覧に表示名が無い応答でも、項目を識別できるよう ID を使う。
              const serverName = serverNames.get(decision.serverId) ?? decision.serverId;
              const target = targetLabel(decision);
              return (
                <li className={`tray-item ${decision.stale ? "stale" : ""}`} key={`${decision.serverId}-${decision.project}-${index}`}>
                  <div className="proj">
                    <span>プロジェクト: {decision.project}</span>
                    <span className="srv">サーバー: {serverName}</span>
                    <span className="kind">種別: {decisionKindLabel(decision)}</span>
                    {target !== undefined ? <span className="target">対象: {target}</span> : null}
                  </div>
                  <div className="desc">
                    {splitBodyParagraphs(decision.body).map((paragraph, paragraphIndex) => (
                      <p key={paragraphIndex}>{paragraph}</p>
                    ))}
                  </div>
                  {decision.options.length > 0 ? (
                    <div className="opts">
                      <span className="opts-label">選択肢</span>
                      <ul>
                        {decision.options.map((option, optionIndex) => (
                          <li key={optionIndex}>{option}</li>
                        ))}
                      </ul>
                    </div>
                  ) : null}
                  <div className="wait">
                    <b>{formatDuration(waitSeconds(decision))} 待ち</b>
                    {decision.stale ? <span className="stale-label">滞留しています。</span> : null}
                  </div>
                </li>
              );
            })}
          </ul>
        )}
        <div className="foot-notes">
          <p>
            判断待ちは、未回答の問い合わせと、人のマージ待ちのゴールの合計です。滞留は
            {thresholds.pendingDecisionStaleMinutes}分を超えたものを示します。
          </p>
        </div>
      </section>
    </main>
  );
}

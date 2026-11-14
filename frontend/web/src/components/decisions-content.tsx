import type { ReactElement } from "react";
import type { DashboardState, InquiryKind } from "../proto/taktdash";
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

const waitSeconds = (decision: PendingDecisionView): number => Math.round(decision.waitMinutes * 60);

// 判断待ちは表示のみとする（order.md:25, :51）。回答・承認・却下の操作要素を持たない。
export function DecisionsContent({ state, now }: DecisionsContentProps): ReactElement {
  const thresholds = resolveThresholds(state.thresholds);
  const decisions = sortPendingDecisions(state.pendingDecisions, now, {
    staleThresholdMinutes: thresholds.pendingDecisionStaleMinutes,
  });

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
            {decisions.map((decision, index) => (
              <li className={`tray-item ${decision.stale ? "stale" : ""}`} key={`${decision.serverId}-${decision.project}-${index}`}>
                <div className="proj">
                  <span>{decision.project}</span>
                  <span className="srv">{decision.serverId}</span>
                  <span className="kind">
                    {decision.type === PENDING_DECISION_TYPE.AWAITING_MERGE
                      ? "人のマージ待ち"
                      : `問い合わせ（${inquiryKindLabel(decision.inquiryKind)}）`}
                  </span>
                </div>
                <p className="desc">{decision.body}</p>
                {decision.options.length > 0 ? <p className="opts">選択肢は {decision.options.join(" と ")} です。</p> : null}
                <div className="wait">
                  <b>{formatDuration(waitSeconds(decision))} 待ち</b>
                  {decision.stale ? <span className="stale-label">滞留しています。</span> : null}
                </div>
              </li>
            ))}
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

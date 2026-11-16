"use client";

// 判断待ちページの一覧。各項目に種別ごとの操作 UI を埋め込み、操作結果は一覧の上に出す。
// 結果は decisionKey で項目に紐づけて保持するため、SSE の再描画で並び順が変わっても
// 同じ項目の結果として残る。
import { useCallback, useState, type ReactElement } from "react";
import {
  decisionKey,
  decisionTypeLabel,
  upsertDecisionResult,
  type DecisionResult,
} from "../state/decision-actions";
import { resolveThresholds, sortPendingDecisions, type DecisionsStateInput } from "../state/dashboard";
import { formatDuration } from "../state/format";
import { BackendUnavailableNotice } from "./backend-unavailable-notice";
import { DecisionActions } from "./decision-actions";
import { DecisionResults } from "./decision-results";

type DecisionsContentProps = {
  state: DecisionsStateInput;
  now: Date;
  disconnected: boolean;
};

const waitSeconds = (waitMinutes: number): number => Math.round(waitMinutes * 60);

export function DecisionsContent({ state, now, disconnected }: DecisionsContentProps): ReactElement {
  const thresholds = resolveThresholds(state.thresholds);
  const decisions = sortPendingDecisions(state.pendingDecisions, now, {
    staleThresholdMinutes: thresholds.pendingDecisionStaleMinutes,
  });
  const [results, setResults] = useState<DecisionResult[]>([]);

  const handleResult = useCallback((next: DecisionResult) => {
    setResults((previous) => upsertDecisionResult(previous, next));
  }, []);

  return (
    <main className="page">
      {disconnected ? <BackendUnavailableNotice /> : null}
      <header className="page-head">
        <div>
          <h1>判断待ち</h1>
          <p>
            全サーバーで、人の判断を待っている項目が {decisions.length}件あります。待ち時間の長い順に並べています。
          </p>
        </div>
      </header>

      <section className="tray" aria-label="判断待ちの一覧">
        <DecisionResults results={results} />
        {decisions.length === 0 ? (
          <p className="note">判断待ちはありません。</p>
        ) : (
          <ul className="tray-list">
            {decisions.map((decision) => (
              <li className={`tray-item${decision.stale ? " stale" : ""}`} key={decisionKey(decision)}>
                <div className="proj">
                  <span>{decision.project}</span>
                  <span className="srv">{decision.serverId}</span>
                  <span className="kind">{decisionTypeLabel(decision)}</span>
                </div>
                <p className="desc">{decision.body}</p>
                {decision.options.length > 0 ? <p className="opts">選択肢は {decision.options.join(" と ")} です。</p> : null}
                <div className="wait">
                  <b>{formatDuration(waitSeconds(decision.waitMinutes))} 待ち</b>
                  {decision.stale ? <span className="stale-label">滞留しています。</span> : null}
                </div>
                <DecisionActions decision={decision} onResult={handleResult} />
              </li>
            ))}
          </ul>
        )}
        <div className="foot-notes">
          <p>
            判断待ちは、未回答の問い合わせ、人のマージ待ちのゴール、ゴール登録の承認待ちの合計です。滞留は
            {thresholds.pendingDecisionStaleMinutes}分を超えたものを示します。
          </p>
        </div>
      </section>
    </main>
  );
}

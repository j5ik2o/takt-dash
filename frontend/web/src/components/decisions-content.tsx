"use client";

// 判断待ちページの一覧。各項目に種別ごとの操作 UI を埋め込み、操作結果は一覧の上に出す。
// 結果は decisionKey で項目に紐づけて保持するため、SSE の再描画で並び順が変わっても
// 同じ項目の結果として残る。
import { useCallback, useState, type ReactElement } from "react";
import type { PendingDecision } from "../proto/taktdash";
import {
  decisionActionState,
  decisionKey,
  decisionTypeLabel,
  pullRequestLinkUrl,
  upsertDecisionResult,
  type DecisionResult,
} from "../state/decision-actions";
import {
  pendingDecisionMergeFailureNote,
  resolveThresholds,
  sortPendingDecisions,
  type DecisionsStateInput,
  type PendingDecisionView,
} from "../state/dashboard";
import { decisionAnchorId } from "../state/anchors";
import { readCoverage } from "../state/read-coverage";
import { formatClock, formatDateTime, formatDuration } from "../state/format";
import type { BackendUnavailableSituation } from "../grpc/client";
import { BackendUnavailableNotice } from "./backend-unavailable-notice";
import { DecisionActions } from "./decision-actions";
import { DecisionResults } from "./decision-results";

type DecisionsContentProps = {
  state: DecisionsStateInput;
  snapshotLoaded: boolean;
  now: Date;
  disconnected: boolean;
  /** 切断時に残す最後の内容の取得時刻。ライブ表示だけが渡す。 */
  readAt?: Date;
  /** 切断時の通知の文面を決める状況。ライブ表示だけが渡す。 */
  unavailableSituation?: BackendUnavailableSituation;
};

// 対象がプルリクエストなら、中身を確かめられるよう、新しいタブで開くリンクにする（issue #304）。
const targetLabel = (decision: PendingDecision): ReactElement | string | undefined => {
  if (decision.pullRequest !== undefined) {
    const label = `プルリクエスト #${decision.pullRequest.number}`;
    const url = pullRequestLinkUrl(decision.pullRequest.url);
    return url === undefined ? (
      label
    ) : (
      <a href={url} target="_blank" rel="noopener noreferrer">
        {label}
      </a>
    );
  }
  if (decision.goalId !== undefined) {
    return `ゴール ${decision.goalId}`;
  }
  return undefined;
};

const waitSeconds = (decision: PendingDecisionView): number => Math.round(decision.waitMinutes * 60);

// 本文は takt-mcp の原文をそのまま運ぶ。空行を段落の区切りとして扱い、段落内の改行は
// 空白へ畳まずに残す（issue #37 受け入れ条件1）。段落の先頭の字下げも残すため、trim は
// 空の段落を取り除く判定にだけ使い、表示する文字列自体は切り取らない（issue #48）。
const splitBodyParagraphs = (body: string): string[] =>
  body
    .replace(/\r\n?/g, "\n")
    .split(/\n[ \t]*\n+/)
    .filter((paragraph) => paragraph.trim() !== "");

export function DecisionsContent({ state, snapshotLoaded, now, disconnected, readAt, unavailableSituation }: DecisionsContentProps): ReactElement {
  const thresholds = resolveThresholds(state.thresholds);
  const decisions = sortPendingDecisions(state.pendingDecisions, now, {
    staleThresholdMinutes: thresholds.pendingDecisionStaleMinutes,
  });
  // 判断待ちの供給元はプロジェクトであり、配下のないサーバー登録は読み取り対象ではない。
  // 0件でも「判断待ちはありません」とは断定できないため、未読を注記する（issue #259）。
  const coverage = readCoverage(state.projects, snapshotLoaded);
  const hasUnread = coverage !== "complete";
  const [results, setResults] = useState<DecisionResult[]>([]);

  const handleResult = useCallback((next: DecisionResult) => {
    setResults((previous) => upsertDecisionResult(previous, next));
  }, []);

  return (
    <main className="page">
      {disconnected ? <BackendUnavailableNotice situation={unavailableSituation} /> : null}
      <header className="page-head">
        <div>
          <h1>判断待ち</h1>
          {decisions.length > 0 && coverage !== "unread" ? <p>
            {coverage === "partial" ? "読めた範囲で、" : null}
            全サーバーで、人の判断を待っている項目が {decisions.length}件あります。待ち時間の長い順に並べています。
          </p> : null}
          {hasUnread ? (
            <p className="stale-note">
              まだ読めていないサーバー・プロジェクトがあります。表示している件数には、まだ読めていない分は含まれていません。
            </p>
          ) : null}
          {disconnected && readAt !== undefined ? (
            <p className="stale-note">
              最終読み取り時刻 {formatClock(readAt)}（{formatDateTime(readAt)}）時点の内容を表示しています。
            </p>
          ) : null}
        </div>
      </header>

      {/* 0件は警告ではないため、橙の容器ではなく通常の線の容器にする（issue #121）。 */}
      <section className={decisions.length === 0 ? "tray tray-empty" : "tray"} aria-label="判断待ちの一覧">
        <DecisionResults results={results} />
        {decisions.length === 0 ? (
          hasUnread ? (
            <p className="note">サーバーからまだ読めていないため、判断待ちがあるかどうかは分かりません。</p>
          ) : (
            <p className="note">判断待ちはありません。</p>
          )
        ) : (
          <ul className="tray-list">
            {decisions.map((decision) => {
              const target = targetLabel(decision);
              const mergeFailureNote = pendingDecisionMergeFailureNote(decision);
              // 選択肢の回答ボタンを出す項目では、同じ文を一覧でも出さず、ボタンの1か所だけにする。
              // ボタンを出せない項目は、一覧が選択肢を知る唯一の場所なので残す（issue #200）。
              const showOptionList =
                decision.options.length > 0 && decisionActionState(decision).kind !== "question-options";
              // 骨格は見出し行・本文・回答ボタンの3行に固定し、内容の長さで位置をずらさない（issue #121）。
              return (
                <li
                  id={decisionAnchorId(decisionKey(decision))}
                  className={`tray-item l-stack${decision.stale ? " stale" : ""}`}
                  key={decisionKey(decision)}
                >
                  <div className="decision-head l-cluster">
                    <div className="proj l-cluster">
                      <span>プロジェクト: {decision.project}</span>
                      <span className="kind">種別: {decisionTypeLabel(decision)}</span>
                      {target !== undefined ? <span className="target">対象: {target}</span> : null}
                    </div>
                    <div className="wait l-cluster">
                      <b>{formatDuration(waitSeconds(decision))} 待ち</b>
                      {decision.stale ? <span className="stale-label">滞留しています。</span> : null}
                    </div>
                  </div>
                  <div className="decision-body">
                    <div className="desc">
                      {splitBodyParagraphs(decision.body).map((paragraph, paragraphIndex) => (
                        <p key={paragraphIndex}>{paragraph}</p>
                      ))}
                    </div>
                    {mergeFailureNote !== undefined ? <p>{mergeFailureNote}</p> : null}
                    {showOptionList ? (
                      <div className="opts">
                        <span className="opts-label">選択肢</span>
                        <ul>
                          {decision.options.map((option, optionIndex) => (
                            <li key={optionIndex}>{option}</li>
                          ))}
                        </ul>
                      </div>
                    ) : null}
                  </div>
                  <DecisionActions decision={decision} disconnected={disconnected} onResult={handleResult} />
                </li>
              );
            })}
          </ul>
        )}
        <div className="foot-notes">
          <p>
            判断待ちは、未回答の問い合わせ、人のマージ待ちのゴール、ゴール登録の承認待ちの合計です。滞留は
            {thresholds.pendingDecisionStaleMinutes}分、ログの無音は{thresholds.runLogSilentMinutes}
            分を超えたものを示します。
          </p>
        </div>
      </section>
    </main>
  );
}

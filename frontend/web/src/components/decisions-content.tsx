"use client";

// 判断待ちページの一覧。各項目に種別ごとの操作 UI を埋め込み、操作結果は一覧の上に出す。
// 結果は decisionKey で項目に紐づけて保持するため、SSE の再描画で並び順が変わっても
// 同じ項目の結果として残る。
import { useCallback, useState, type ReactElement } from "react";
import type { PendingDecision } from "../proto/taktdash";
import {
  decisionKey,
  decisionTypeLabel,
  upsertDecisionResult,
  type DecisionResult,
} from "../state/decision-actions";
import {
  neverReadProjects,
  neverReadServers,
  pendingDecisionMergeFailureNote,
  resolveThresholds,
  sortPendingDecisions,
  type DecisionsStateInput,
  type PendingDecisionView,
} from "../state/dashboard";
import { decisionAnchorId } from "../state/anchors";
import { formatClock, formatDateTime, formatDuration } from "../state/format";
import type { BackendUnavailableSituation } from "../grpc/client";
import { BackendUnavailableNotice } from "./backend-unavailable-notice";
import { DecisionActions } from "./decision-actions";
import { DecisionResults } from "./decision-results";

type DecisionsContentProps = {
  state: DecisionsStateInput;
  now: Date;
  disconnected: boolean;
  /** 切断時に残す最後の内容の取得時刻。ライブ表示だけが渡す。 */
  readAt?: Date;
  /** 切断時の通知の文面を決める状況。ライブ表示だけが渡す。 */
  unavailableSituation?: BackendUnavailableSituation;
};

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
// 空白へ畳まずに残す（issue #37 受け入れ条件1）。段落の先頭の字下げも残すため、trim は
// 空の段落を取り除く判定にだけ使い、表示する文字列自体は切り取らない（issue #48）。
const splitBodyParagraphs = (body: string): string[] =>
  body
    .replace(/\r\n?/g, "\n")
    .split(/\n[ \t]*\n+/)
    .filter((paragraph) => paragraph.trim() !== "");

export function DecisionsContent({ state, now, disconnected, readAt, unavailableSituation }: DecisionsContentProps): ReactElement {
  const thresholds = resolveThresholds(state.thresholds);
  const decisions = sortPendingDecisions(state.pendingDecisions, now, {
    staleThresholdMinutes: thresholds.pendingDecisionStaleMinutes,
  });
  // サーバー一覧に表示名が無い応答でも、項目を識別できるよう ID を使う。
  const serverNames = new Map(state.servers.map((server) => [server.serverId, server.name]));
  // まだ読めていないサーバー・プロジェクトがあると、その配下に判断待ちがあっても
  // 一覧へ出てこない。サーバーの最終読み取り時刻は配下プロジェクトの最大値のため、
  // サーバーが既読でもプロジェクト単位では未読のことがある。ゴールページと同じく
  // サーバーとプロジェクトの両方で判定する。
  // 0件でも「判断待ちはありません」とは断定できないため、未読を注記する（issue #259）。
  const hasUnread =
    neverReadServers(state.servers).length > 0 || neverReadProjects(state.projects).length > 0;
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
          <p>
            全サーバーで、人の判断を待っている項目が {decisions.length}件あります。待ち時間の長い順に並べています。
          </p>
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

      <section className="tray" aria-label="判断待ちの一覧">
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
              const serverName = serverNames.get(decision.serverId) ?? decision.serverId;
              const target = targetLabel(decision);
              const mergeFailureNote = pendingDecisionMergeFailureNote(decision);
              return (
                <li
                  id={decisionAnchorId(decisionKey(decision))}
                  className={`tray-item${decision.stale ? " stale" : ""}`}
                  key={decisionKey(decision)}
                >
                  <div className="proj">
                    <span>プロジェクト: {decision.project}</span>
                    <span className="srv">サーバー: {serverName}</span>
                    <span className="kind">種別: {decisionTypeLabel(decision)}</span>
                    {target !== undefined ? <span className="target">対象: {target}</span> : null}
                  </div>
                  <div className="desc">
                    {splitBodyParagraphs(decision.body).map((paragraph, paragraphIndex) => (
                      <p key={paragraphIndex}>{paragraph}</p>
                    ))}
                  </div>
                  {mergeFailureNote !== undefined ? <p>{mergeFailureNote}</p> : null}
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
                  <DecisionActions decision={decision} onResult={handleResult} />
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

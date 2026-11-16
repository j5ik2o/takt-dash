import type { ReactElement } from "react";
import type { OverviewView } from "../state/dashboard";
import { formatClock, formatDateTime } from "../state/format";
import { BackendUnavailableNotice } from "./backend-unavailable-notice";

type OverviewContentProps = {
  view: OverviewView;
  disconnected: boolean;
};

// 概要は要件 order.md:23 とモック docs/taktdash.html:663-699 に沿う。「24時間のトークン消費」は
// 対象外のため、タイルもその旨の表示も持たない。
export function OverviewContent({ view, disconnected }: OverviewContentProps): ReactElement {
  const { thresholds, pendingCounts } = view;
  return (
    <main className="page">
      {disconnected ? <BackendUnavailableNotice /> : null}
      <header className="page-head">
        <div>
          <h1>概要</h1>
          <p>
            {view.servers.length}台のサーバーを集約し、{view.runningRuns}件の run が動いています。
          </p>
        </div>
      </header>

      <section className="tiles" aria-label="概要の集計">
        <div className="tile">
          <div className="label">実行中の run（全サーバー）</div>
          <div className="num">
            {view.runningRuns}
            <small>件</small>
          </div>
        </div>
        <div className="tile">
          <div className="label">判断待ち（全サーバー）</div>
          <div className="num">
            {pendingCounts.total}
            <small>件</small>
          </div>
          <ul className="kv">
            <li>
              <span>未回答の問い合わせ</span>
              <b>{pendingCounts.inquiries}件</b>
            </li>
            <li>
              <span>人のマージ待ち</span>
              <b>{pendingCounts.awaitingMerge}件</b>
            </li>
            <li>
              <span>ゴール登録の承認待ち</span>
              <b>{pendingCounts.goalRegistration}件</b>
            </li>
          </ul>
        </div>
        <div className="tile">
          <div className="label">異常判定の閾値</div>
          <ul className="kv">
            <li>
              <span>判断待ちの滞留</span>
              <b>{thresholds.pendingDecisionStaleMinutes}分</b>
            </li>
            <li>
              <span>ログの無音</span>
              <b>{thresholds.runLogSilentMinutes}分</b>
            </li>
          </ul>
        </div>
        <div className="tile">
          <div className="label">最終読み取り時刻</div>
          <div className="num word">{formatClock(view.readAt)}</div>
          <div className="t-note">takt-mcp から最後に読めた時刻です（{formatDateTime(view.readAt)}）。</div>
        </div>
      </section>

      <section aria-labelledby="h-projects">
        <h2 className="sec" id="h-projects">
          プロジェクト
        </h2>
        <ul className="cards">
          {view.projects.map((project) => (
            <li className="card" key={project.name}>
              <h3>{project.name}</h3>
              <p className="about">{project.about}</p>
              <p>直近24時間に完了したタスクは {project.completedTasks24h}件です。</p>
              <a className="view" href={`/goals?project=${encodeURIComponent(project.name)}`}>
                ゴール一覧を見る
              </a>
            </li>
          ))}
        </ul>
      </section>

      <section aria-labelledby="h-servers">
        <h2 className="sec" id="h-servers">
          サーバー
        </h2>
        <div className="scards">
          {view.servers.map(({ server, state, stateLabel, runs }) => (
            <article className={`scard ${state === "down" ? "down" : ""}`} key={server.serverId}>
              <div className="sc-head">
                <h3>
                  <span className={`dot ${state}`} aria-hidden="true" />
                  <span>
                    {server.name}
                    <span className="kind">{server.kind}</span>
                  </span>
                </h3>
                <span className={`big-badge ${state}`}>{stateLabel}</span>
              </div>
              <div className="read-at">
                {server.lastReadAt
                  ? `takt-mcp から ${formatClock(server.lastReadAt)} に読みました。`
                  : "takt-mcp からまだ読めていません。"}
              </div>
              <ul className="runs">
                {runs.length > 0 ? (
                  runs.map((runView) => (
                    <li className="run-row" key={runView.run.runSlug}>
                      <span className={`st ${runView.connected ? "live" : "idle"}`}>{runView.label}</span>
                      <div className="run-main">
                        <span className="p">{runView.run.project}</span>
                        <div className="task">タスク「{runView.run.taskName}」</div>
                      </div>
                      <a className="view" href={`/runs/${encodeURIComponent(runView.run.runSlug)}`}>
                        run の詳細を見る
                      </a>
                    </li>
                  ))
                ) : (
                  <li className="empty">走っている run はありません。</li>
                )}
              </ul>
            </article>
          ))}
        </div>
      </section>
    </main>
  );
}

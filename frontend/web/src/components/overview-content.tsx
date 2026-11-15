import type { ReactElement } from "react";
import Link from "next/link";
import type { OverviewView } from "../state/dashboard";
import { formatClock, formatDateTime, formatDuration, formatModel } from "../state/format";
import { projectHref, serverAnchorId } from "../state/anchors";

type OverviewContentProps = {
  view: OverviewView;
  flashingRunSlugs?: ReadonlySet<string>;
};

const positionText = (stepNumber?: number, totalSteps?: number, iteration?: number): string => {
  if (stepNumber === undefined || totalSteps === undefined) {
    return "";
  }
  const iterationText = iteration !== undefined ? `、反復 ${iteration} 回目` : "";
  return `（${totalSteps} ステップ中 ${stepNumber} つ目${iterationText}）`;
};

// 概要は要件 order.md:23 とモック docs/taktdash.html:663-699 に沿う。「24時間のトークン消費」は
// 対象外のため、タイルもその旨の表示も持たない。
export function OverviewContent({ view, flashingRunSlugs }: OverviewContentProps): ReactElement {
  const { thresholds, pendingCounts } = view;
  return (
    <main className="page">
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
            <li key={project.name}>
              <Link className="card card-link" href={projectHref(project.name)}>
                <h3>{project.name}</h3>
                <p className="about">{project.about}</p>
                <p>直近24時間に完了したタスクは {project.completedTasks24h}件です。</p>
              </Link>
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
            <article
              id={serverAnchorId(server.serverId)}
              className={`scard ${state === "down" ? "down" : ""}`}
              key={server.serverId}
            >
              <div className="sc-head">
                <h3>
                  <span className={`dot ${state}`} aria-hidden="true" />
                  <span>
                    表示名: {server.name}
                    <span className="srv-id">サーバー ID: {server.serverId}</span>
                    <span className="kind">機種: {server.kind}</span>
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
                  runs.map((runView) => {
                    const flashing = flashingRunSlugs?.has(runView.run.runSlug) ?? false;
                    const position = positionText(
                      runView.stepNumber,
                      runView.totalSteps,
                      runView.iteration,
                    );
                    const lastLogText =
                      runView.elapsedSeconds !== undefined
                        ? `最後のログは ${formatDuration(runView.elapsedSeconds)}前`
                        : "最後のログはまだありません";
                    return (
                      <li
                        className={`run-row${flashing ? " flash" : ""}`}
                        key={runView.run.runSlug}
                      >
                        <span className={`st ${runView.state}`}>
                          <span className={`dot ${runView.state}`} aria-hidden="true" />
                          {runView.label}
                        </span>
                        <div className="run-main">
                          <span className="p">{runView.run.project}</span>
                          {runView.goalName ? <span className="g">{runView.goalName}</span> : null}
                          <div className="task">
                            タスク「{runView.run.summary || runView.run.taskName}」
                          </div>
                          {runView.model ? (
                            <div className="model">モデルは {formatModel(runView.model)} です。</div>
                          ) : null}
                        </div>
                        <div className="run-prog">
                          {runView.currentStepName ? (
                            <div className="stp">
                              ステップ <b>{runView.currentStepName}</b>
                              {position}
                            </div>
                          ) : null}
                          {runView.stepNumber !== undefined && runView.totalSteps ? (
                            <div
                              className={`bar thick ${runView.state}`}
                              role="img"
                              aria-label={`${runView.totalSteps} ステップ中 ${runView.stepNumber} つ目`}
                            >
                              <i
                                style={{
                                  width: `${Math.round(
                                    (runView.stepNumber / runView.totalSteps) * 100,
                                  )}%`,
                                }}
                              />
                            </div>
                          ) : null}
                          <div className="meta">
                            <span className={runView.state === "warn" ? "warn" : ""}>
                              {lastLogText}
                            </span>
                            <a
                              className="view"
                              href={`/runs/${encodeURIComponent(runView.run.runSlug)}`}
                            >
                              run の詳細を見る
                            </a>
                          </div>
                          {runView.state === "warn" && runView.note ? (
                            <div className="run-note warn">{runView.note}</div>
                          ) : null}
                          {runView.state === "delayed" && runView.note ? (
                            <div className="run-note delayed">{runView.note}</div>
                          ) : null}
                        </div>
                      </li>
                    );
                  })
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

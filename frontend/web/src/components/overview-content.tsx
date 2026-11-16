import type { ReactElement } from "react";
import Link from "next/link";
import type { OverviewView } from "../state/dashboard";
import { decisionKey, decisionTypeLabel } from "../state/decision-actions";
import { formatClock, formatDateTime, formatDuration, formatModel } from "../state/format";
import { decisionHref, projectHref, serverAnchorId } from "../state/anchors";
import { BackendUnavailableNotice } from "./backend-unavailable-notice";

type OverviewContentProps = {
  view: OverviewView;
  flashingRunSlugs?: ReadonlySet<string>;
  disconnected: boolean;
};

const positionText = (stepNumber?: number, totalSteps?: number, iteration?: number): string => {
  if (stepNumber === undefined || totalSteps === undefined) {
    return "";
  }
  const iterationText = iteration !== undefined ? `、反復 ${iteration} 回目` : "";
  return `（${totalSteps} ステップ中 ${stepNumber} つ目${iterationText}）`;
};

const waitText = (waitMinutes: number): string => formatDuration(Math.round(waitMinutes * 60));

// 上限が0でも 0 を返し、0除算の NaN/Infinity を描画しない。
const runUsagePercent = (runningRuns: number, runLimit: number): number =>
  runLimit > 0 ? Math.min(100, Math.round((runningRuns / runLimit) * 100)) : 0;

// 概要は issue #75 の方針に沿う。上部は実行中 run・判断待ち・24時間完了タスクの3タイルだけにし、
// 異常があるときだけ警告バナーと判断待ちパネルを出す。閾値と最終読み取り時刻のタイルは持たない。
// 「24時間のトークン消費」は API に値が無いため、タイルもプレースホルダーも描画しない。
export function OverviewContent({
  view,
  flashingRunSlugs,
  disconnected,
}: OverviewContentProps): ReactElement {
  const { pendingCounts } = view;
  const hasUnresponsiveServer = view.unresponsiveServers.length > 0;
  const hasUnresponsiveProject = view.unresponsiveProjects.length > 0;
  const partialRead = hasUnresponsiveServer || hasUnresponsiveProject;
  // 応答なしの主語をサーバーとプロジェクトで分ける。サーバーが connected のまま配下の
  // プロジェクトだけが応答なしのときに「サーバー」と主張しない。
  let partialReadSubject = "プロジェクト";
  if (hasUnresponsiveServer) {
    partialReadSubject = hasUnresponsiveProject ? "サーバーとプロジェクト" : "サーバー";
  }
  const partialReadTime = view.partialReadAt
    ? `${formatClock(view.partialReadAt)} 時点`
    : "最後に読めた時刻は不明";
  const longestWait = view.pendingDecisions[0];
  return (
    <main className="page overview">
      {disconnected ? <BackendUnavailableNotice /> : null}
      <header className="page-head">
        <div>
          <h1>概要</h1>
          <p>
            {view.servers.length}台のサーバーを集約し、{view.runningRuns}件の run が動いています。
          </p>
          {partialRead ? (
            <p className="stale-note">
              応答のない{partialReadSubject}があります。実行中の run は、最後に読めた時点で {view.runningRuns} 件です（
              {partialReadTime}）。
            </p>
          ) : null}
          {disconnected ? (
            <p className="stale-note">
              最終読み取り時刻 {formatClock(view.readAt)}（{formatDateTime(view.readAt)}）時点の内容を表示しています。
            </p>
          ) : null}
        </div>
        {view.hasAnomalies ? (
          <div className="health attention" role="status">
            {pendingCounts.total > 0 ? <p>判断待ちが {pendingCounts.total}件あります。</p> : null}
            {view.unresponsiveServers.map((server) => (
              <p key={`server-${server.serverId}`}>{server.name} から応答がありません。</p>
            ))}
            {view.unresponsiveProjects.map((project) => (
              <p key={`project-${project.name}`}>{project.name} から応答がありません。</p>
            ))}
            {view.silentRunCount > 0 ? (
              <p>ログが止まっている run が {view.silentRunCount}件あります。</p>
            ) : null}
            {view.failedTaskCount > 0 ? (
              <p>失敗したタスクが {view.failedTaskCount}件あります。</p>
            ) : null}
            {view.abortedGoalCount > 0 ? (
              <p>中止したゴールが {view.abortedGoalCount}件あります。</p>
            ) : null}
          </div>
        ) : null}
      </header>

      <section className="tiles" aria-label="概要の集計">
        <div className="tile">
          <div className="label">実行中の run（全サーバー）</div>
          <div className="num">
            {view.runningRuns}
            <small>/ {view.runLimit} 件</small>
          </div>
          <div
            className="bar live"
            role="img"
            aria-label={`実行上限 ${view.runLimit} 件に対して ${view.runningRuns} 件`}
          >
            <i style={{ width: `${runUsagePercent(view.runningRuns, view.runLimit)}%` }} />
          </div>
          {partialRead ? (
            <div className="t-note">
              応答のない{partialReadSubject}があるため、最後に読めた時点で {view.runningRuns} 件です（
              {partialReadTime}）。
            </div>
          ) : null}
        </div>
        <div className={`tile${pendingCounts.total > 0 ? " attention" : ""}`}>
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
          {longestWait !== undefined ? (
            <div className="t-note">
              最も長いものは {waitText(longestWait.waitMinutes)} 待っています。
            </div>
          ) : null}
        </div>
        <div className="tile">
          <div className="label">24時間に完了したタスク</div>
          <div className="num">
            {view.completedTasks24h}
            <small>件</small>
          </div>
          {view.failedTaskCount > 0 ? (
            <div className="t-note">失敗したタスクが {view.failedTaskCount}件残っています。</div>
          ) : null}
        </div>
      </section>

      {pendingCounts.total > 0 ? (
        <section className="callout" aria-label="判断待ちの項目">
          <ul className="callout-list">
            {view.pendingDecisions.map((decision) => (
              <li className={decision.stale ? "stale" : undefined} key={decisionKey(decision)}>
                <span className="proj">{decision.project}</span>
                <span className="kind">{decisionTypeLabel(decision)}</span>
                <span className="wait">
                  {waitText(decision.waitMinutes)} 待ち
                  {decision.stale ? "（滞留）" : ""}
                </span>
                <Link className="view" href={decisionHref(decisionKey(decision))}>
                  内容を見る
                </Link>
              </li>
            ))}
          </ul>
        </section>
      ) : null}

      <section aria-labelledby="h-servers">
        <h2 className="sec" id="h-servers">
          サーバー
        </h2>
        <div className="scards">
          {view.servers.map(
            ({ server, state, stateLabel, runs, runLimit, runningRuns, inProgressGoals, pendingCount, waitingTasks }) => (
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
                {state === "down" ? (
                  <div className="stale-note">
                    {server.lastReadAt
                      ? `${formatClock(server.lastReadAt)} 時点の内容です。最後に読めた時点の状態を表示しています。`
                      : "まだ一度も読めていません。"}
                  </div>
                ) : null}
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
                <div className="sc-foot">
                  <span className="cnt">
                    run {runningRuns} / {runLimit}
                  </span>
                  <span>進行中のゴール {inProgressGoals}件</span>
                  <span>判断待ち {pendingCount}件</span>
                  <span>待機中のタスク {waitingTasks}件</span>
                </div>
              </article>
            ),
          )}
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
              <a className="view" href={`/goals?project=${encodeURIComponent(project.name)}`}>
                ゴール一覧を見る
              </a>
            </li>
          ))}
        </ul>
      </section>
    </main>
  );
}

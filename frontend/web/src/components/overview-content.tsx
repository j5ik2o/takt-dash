import type { ReactElement } from "react";
import Link from "next/link";
import type { OverviewView } from "../state/dashboard";
import { notRespondingProjectText } from "../state/dashboard";
import { CONNECTION_STATE } from "../state/proto-values";
import { decisionKey, decisionTypeLabel } from "../state/decision-actions";
import { formatClock, formatDateTime, formatDuration, formatModel } from "../state/format";
import { decisionHref, projectHref, serverAnchorId } from "../state/anchors";
import { readCountText } from "../state/read-coverage";
import type { BackendUnavailableSituation } from "../grpc/client";
import { BackendUnavailableNotice } from "./backend-unavailable-notice";

type OverviewContentProps = {
  view: OverviewView;
  flashingRunSlugs?: ReadonlySet<string>;
  disconnected: boolean;
  /** 切断時の通知の文面を決める状況。ライブ表示だけが渡す。 */
  unavailableSituation?: BackendUnavailableSituation;
};

const positionText = (stepNumber?: number, totalSteps?: number, iteration?: number): string => {
  if (stepNumber === undefined || totalSteps === undefined) {
    return "";
  }
  const iterationText = iteration !== undefined ? `、反復 ${iteration} 回目` : "";
  return `（${totalSteps} ステップ中 ${stepNumber} つ目${iterationText}）`;
};

const waitText = (waitMinutes: number): string => formatDuration(Math.round(waitMinutes * 60));

// プロジェクトが増えても、概要の最初の画面がカードの一覧で埋まらないよう枚数を抑える。
// 超えた分は件数だけを示し、全件は /projects で見る。
const OVERVIEW_PROJECT_CARD_LIMIT = 6;

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
  unavailableSituation,
}: OverviewContentProps): ReactElement {
  const { pendingCounts } = view;
  const partialReadSubject = view.partialReadSubject;
  const partialRead = partialReadSubject !== undefined;
  const partialReadTime = view.partialReadAt
    ? `${formatClock(view.partialReadAt)} 時点`
    : "最後に読めた時刻は不明";
  const firstProjectReadAt = view.projects[0]?.lastReadAt;
  const commonProjectReadAt = firstProjectReadAt !== undefined && view.projects.every(
    (project) => project.lastReadAt?.getTime() === firstProjectReadAt.getTime(),
  ) ? firstProjectReadAt : undefined;
  const emptyRunText = view.coverage !== "complete"
    ? "まだ読めていないため、run があるかどうかは分かりません。"
    : (partialRead || disconnected) && view.projects.length > 0
      ? commonProjectReadAt !== undefined
        ? `最後に読めた時点（${formatClock(commonProjectReadAt)} 時点）では、走っている run はありませんでした。`
        : "各プロジェクトで最後に取得した結果には、走っている run はありませんでした。"
      : "走っている run はありません。";
  const lastReadText = view.readAt
    ? `最終読み取り時刻 ${formatClock(view.readAt)}（${formatDateTime(view.readAt)}）時点の内容を表示しています。`
    : view.snapshotLoaded && view.projects.length === 0
      ? "取得済みの概要を表示しています。"
      : "takt-mcp からまだ読めていません。";
  const longestWait = view.pendingDecisions[0];
  const shownProjects = view.projects.slice(0, OVERVIEW_PROJECT_CARD_LIMIT);
  const hiddenProjectCount = view.projects.length - shownProjects.length;
  const knownEmptyRuns = view.coverage === "complete" && view.runs.length === 0;
  const hasConfiguration = view.projects.length > 0 || view.snapshotLoaded;
  return (
    <main className="page overview">
      {disconnected ? <BackendUnavailableNotice situation={unavailableSituation} /> : null}
      <header className="page-head">
        <div>
          <h1>概要</h1>
          <p>
            {view.snapshotLoaded ? (
              <>
                {view.servers.length}台のサーバーを集約{knownEmptyRuns ? "しています。" : <>し、{view.coverage === "complete"
                  ? `${view.runningRuns}件の run が動いています。`
                  : `実行中の run は${readCountText(view.runningRuns, view.coverage)}です。`}</>}
              </>
            ) : "サーバーの台数は不明です。実行中の run は不明です。"}
          </p>
          {view.snapshotLoaded && view.coverage !== "complete" ? (
            <p className="stale-note">まだ読めていないプロジェクトがあります。run・判断待ち・ゴールの件数には未読の分を含めていません。</p>
          ) : null}
          {partialRead && (view.coverage !== "complete" || knownEmptyRuns) ? (
            <p className="stale-note">応答のない{partialReadSubject}があります（{partialReadTime}）。</p>
          ) : null}
          {partialRead && view.coverage === "complete" && !knownEmptyRuns ? (
            <p className="stale-note">
              応答のない{partialReadSubject}があります。実行中の run は、最後に読めた時点で {view.runningRuns} 件です（
              {partialReadTime}）。
            </p>
          ) : null}
          {disconnected ? <p className="stale-note">{lastReadText}</p> : null}
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
          </div>
        ) : null}
      </header>

      <section className="tiles" aria-label="概要の集計">
        <div className="tile">
          <div className="label">実行中の run（全サーバー）</div>
          <div className="num">
            {knownEmptyRuns && !partialRead && !disconnected ? <>{view.runningRuns}<small>/ {view.runLimit} 件</small></>
              : knownEmptyRuns ? <>上限 {view.runLimit}<small>件</small></>
                : view.coverage === "complete" ? <>{view.runningRuns}<small>/ {view.runLimit} 件</small></>
                  : <>{readCountText(view.runningRuns, view.coverage)}{hasConfiguration ? <small>上限 {view.runLimit} 件</small> : null}</>}
          </div>
          {view.coverage === "complete" && !knownEmptyRuns ? <div
            className="bar live"
            role="img"
            aria-label={`実行上限 ${view.runLimit} 件に対して ${view.runningRuns} 件`}
          >
            <i style={{ width: `${runUsagePercent(view.runningRuns, view.runLimit)}%` }} />
          </div> : null}
          {partialRead && view.coverage !== "complete" ? (
            <div className="t-note">応答のない{partialReadSubject}があります（{partialReadTime}）。</div>
          ) : null}
          {partialRead && view.coverage === "complete" && !knownEmptyRuns ? (
            <div className="t-note">
              応答のない{partialReadSubject}があるため、最後に読めた時点で {view.runningRuns} 件です（
              {partialReadTime}）。
            </div>
          ) : null}
        </div>
        <div className={`tile${pendingCounts.total > 0 ? " attention" : ""}`}>
          <div className="label">判断待ち（全サーバー）</div>
          <div className="num">
            {view.coverage === "complete" ? <>{pendingCounts.total}<small>件</small></> : readCountText(pendingCounts.total, view.coverage)}
          </div>
          <ul className="kv">
            <li>
              <span>未回答の問い合わせ</span>
              <b>{readCountText(pendingCounts.inquiries, view.coverage)}</b>
            </li>
            <li>
              <span>人のマージ待ち</span>
              <b>{readCountText(pendingCounts.awaitingMerge, view.coverage)}</b>
            </li>
            <li>
              <span>ゴール登録の承認待ち</span>
              <b>{readCountText(pendingCounts.goalRegistration, view.coverage)}</b>
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
            {view.coverage === "complete" || view.completedTasks24h > 0
              ? <>{view.completedTasks24h}<small>件</small></> : "不明"}
          </div>
          <div className="t-note">保存済みの履歴を全プロジェクト分集計しています。</div>
          {view.coverage !== "complete" ? (
            <div className="t-note">未読のプロジェクトの最新状態はまだ確認できていません。</div>
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

      <section className="l-section" aria-labelledby="h-servers">
        <div className="l-cluster server-strip">
          <h2 id="h-servers">サーバー</h2>
          {view.servers.map(({ server, state, stateLabel, notRespondingProjects }) => (
            <div id={serverAnchorId(server.serverId)} className="l-cluster server-line" key={server.serverId}>
              <span className={`dot ${state}`} aria-hidden="true" />
              <span className="server-name">
                表示名: {server.name}
                <span className="srv-id">サーバー ID: {server.serverId}</span>
                <span className="kind">機種: {server.kind}</span>
              </span>
              <span className="read-at">
                {server.writeOnly
                  ? "書き込み専用のため、takt-mcp からは読み取りません。"
                  : server.lastReadAt
                    ? `takt-mcp から ${formatClock(server.lastReadAt)} に読みました。`
                    : "takt-mcp からまだ読めていません。"}
              </span>
              <span className={`big-badge ${state}`}>{stateLabel}</span>
              {state === "down" ? (
                <p className="stale-note">
                  {server.lastReadAt
                    ? `${formatClock(server.lastReadAt)} 時点の内容です。最後に読めた時点の状態を表示しています。`
                    : "まだ一度も読めていません。"}
                </p>
              ) : null}
              {/* サーバーの読み取り時刻は配下プロジェクトの最新の時刻であり、応答していない
                  プロジェクトがあっても進んでしまう。プロジェクトごとの最後に読めた時刻を
                  サーバーの1行に並べることで、サーバー単位の時刻とプロジェクトごとの時刻の
                  関係が画面から読み取れるようにする（issue #264）。 */}
              {notRespondingProjects.map((project) => (
                <p className="stale-note" key={project.name}>
                  {notRespondingProjectText(project)}
                  {project.connectionState === CONNECTION_STATE.UNRESPONSIVE && project.lastReadAt
                    ? `最後に読めたのは ${formatClock(project.lastReadAt)} です。`
                    : null}
                </p>
              ))}
            </div>
          ))}
        </div>
      </section>

      <section className="l-section" aria-labelledby="h-runs">
        <h2 id="h-runs">実行中の run</h2>
        {view.runs.length > 0 ? (
          <div className="l-table runs-table">
            <table>
              <thead>
                <tr>
                  <th scope="col">プロジェクト</th>
                  <th scope="col">ゴール</th>
                  <th scope="col">ステップ</th>
                  <th scope="col">経過</th>
                  <th scope="col">状態</th>
                </tr>
              </thead>
              <tbody>
                {view.runs.map((runView) => {
                  const flashing = flashingRunSlugs?.has(runView.run.runSlug) ?? false;
                  const position = positionText(runView.stepNumber, runView.totalSteps, runView.iteration);
                  const lastLogText =
                    runView.elapsedSeconds !== undefined
                      ? `最後のログは ${formatDuration(runView.elapsedSeconds)}前`
                      : "最後のログはまだありません";
                  return (
                    <tr className={`run-row${flashing ? " flash" : ""}`} key={runView.run.runSlug}>
                      <td className="run-project" data-label="プロジェクト">
                        <span className="p">{runView.run.project}</span>
                        {/* 同名のプロジェクトが複数のサーバーにあると、プロジェクト名だけでは
                            どのサーバーの run か分からない。サーバーの表示名を併記し、サーバーが
                            引けないときはサーバー ID で代える（アクティビティと同じ代え方）。 */}
                        <span className="srv">{runView.server?.name ?? runView.run.serverId}</span>
                        <a className="view" href={`/runs/${encodeURIComponent(runView.run.runSlug)}`}>
                          run の詳細を見る
                        </a>
                      </td>
                      <td className="run-goal" data-label="ゴール">
                        {runView.goalName ? <span className="g">{runView.goalName}</span> : null}
                        <div className="task">タスク「{runView.run.summary || runView.run.taskName}」</div>
                      </td>
                      <td className="run-step" data-label="ステップ">
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
                            <i style={{ width: `${Math.round((runView.stepNumber / runView.totalSteps) * 100)}%` }} />
                          </div>
                        ) : null}
                        {runView.model ? <div className="model">モデルは {formatModel(runView.model)} です。</div> : null}
                      </td>
                      <td className="run-elapsed" data-label="経過">
                        <span className={runView.state === "warn" ? "warn" : undefined}>{lastLogText}</span>
                      </td>
                      <td className="run-status" data-label="状態">
                        <span className={`st ${runView.state}`}>
                          <span className={`dot ${runView.state}`} aria-hidden="true" />
                          {runView.label}
                        </span>
                        {runView.state === "warn" && runView.note ? (
                          <div className="run-note warn">{runView.note}</div>
                        ) : null}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        ) : (
          <p className="empty">{emptyRunText}</p>
        )}
      </section>

      <section aria-labelledby="h-projects">
        <h2 className="sec" id="h-projects">
          プロジェクト
        </h2>
        <p>進行中のゴールは {readCountText(view.inProgressGoalCount, view.coverage)}です。</p>
        <ul className="overview-cards">
          {shownProjects.map((project) => (
            <li key={project.name}>
              <Link className="card card-link" href={projectHref(project.name)}>
                <h3>{project.name}</h3>
                <p className="about">{project.about}</p>
                {project.lastReadAt !== undefined || project.completedTasks24h > 0 ? (
                  <>
                    <p>直近24時間に完了したタスクは {project.completedTasks24h}件です。</p>
                    <p>保存済みの履歴です。</p>
                  </>
                ) : null}
                {project.lastReadAt === undefined ? (
                  <>
                    <p>{project.completedTasks24h > 0 ? "まだ読めていないため、run の件数は不明です。" : "まだ読めていないため、完了タスクと run の件数は不明です。"}</p>
                    <p>同時実行の上限は {project.concurrency} 件です。</p>
                  </>
                ) : (
                  <>
                    <p>
                      実行中の run は {project.runningRuns} 件で、同時実行の上限は {project.concurrency} 件です。
                    </p>
                  </>
                )}
              </Link>
              <a className="view" href={`/goals?project=${encodeURIComponent(project.name)}`}>
                ゴール一覧を見る
              </a>
            </li>
          ))}
        </ul>
        {hiddenProjectCount > 0 ? (
          <p>
            <Link className="view" href="/projects">
              ほか {hiddenProjectCount} 件のプロジェクトを見る
            </Link>
          </p>
        ) : null}
      </section>
    </main>
  );
}

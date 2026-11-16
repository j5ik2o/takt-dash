import type { ReactElement } from "react";
import type { GetRunDetailResponse } from "../proto/taktdash";
import { formatClock, formatDateTime, formatDuration } from "../state/format";

type RunDetailContentProps = {
  detail: GetRunDetailResponse;
  now: Date;
};

// run 詳細は takt_get_run で取得できる範囲だけを表示する（order.md:27）。
// ログは末尾（logTail）だけを示し、JSONL の生ログは表示しない。
export function RunDetailContent({ detail, now }: RunDetailContentProps): ReactElement {
  const elapsed =
    detail.startedAt !== undefined ? formatDuration((now.getTime() - detail.startedAt.getTime()) / 1000) : undefined;
  return (
    <main className="page">
      <a className="back" href="/projects">
        プロジェクト一覧に戻る
      </a>
      <header className="page-head">
        <div>
          <h1>{detail.taskName}</h1>
          <p>
            {detail.project} の run です。ワークフローは {detail.workflow} です。
          </p>
        </div>
      </header>

      <section className="summary" aria-label="run の要約">
        <div className="sum-grid">
          <div className="f">
            <div className="k">サーバー</div>
            <div className="v mono">{detail.serverId}</div>
          </div>
          <div className="f">
            <div className="k">開始と経過</div>
            <div className="v">
              {detail.startedAt ? (
                <>
                  <span className="mono">{formatDateTime(detail.startedAt)}</span> に始まりました。
                </>
              ) : (
                "開始時刻は不明です。"
              )}
              {elapsed !== undefined ? `経過は ${elapsed} です。` : ""}
            </div>
          </div>
          <div className="f">
            <div className="k">今のステップ</div>
            <div className="v">
              {detail.currentStep !== undefined ? <span className="mono">{detail.currentStep}</span> : "なし"}
              {detail.currentIteration !== undefined ? `（反復 ${detail.currentIteration} 回目）` : ""}
            </div>
          </div>
          <div className="f">
            <div className="k">このステップの開始</div>
            <div className="v">
              {detail.currentStepStartedAt ? (
                <>
                  <span className="mono">{formatClock(detail.currentStepStartedAt)}</span> に始まりました。
                </>
              ) : (
                "開始時刻は不明です。"
              )}
            </div>
          </div>
          <div className="f">
            <div className="k">最後のログ</div>
            <div className="v mono">{detail.lastLogAt ? `${formatClock(detail.lastLogAt)}（${formatDateTime(detail.lastLogAt)}）` : "まだログはありません。"}</div>
          </div>
          <div className="f wide">
            <div className="k">要約</div>
            <div className="v">{detail.summary}</div>
          </div>
          <div className="f wide">
            <div className="k">タスク本文</div>
            <div className="v">{detail.taskText}</div>
          </div>
        </div>
      </section>

      <section aria-labelledby="h-steps">
        <h2 className="sec" id="h-steps">
          ステップ
        </h2>
        <ul className="steps">
          {detail.stepLogs.map((step, index) => (
            <li key={`${step.stepName}-${index}`}>
              <div className="step-head">
                <b>{step.stepName}</b>
                <span>{step.persona}</span>
                <span>{step.status}</span>
                <span>反復 {step.iteration} 回目</span>
              </div>
              <pre className="step-body">{step.content}</pre>
            </li>
          ))}
        </ul>
      </section>

      <section aria-labelledby="h-log">
        <h2 className="sec" id="h-log">
          ログの末尾
        </h2>
        <pre className="log-tail">{detail.logTail}</pre>
      </section>

      <section aria-labelledby="h-reports">
        <h2 className="sec" id="h-reports">
          レポート
        </h2>
        {detail.reports.length === 0 ? (
          <p className="note">レポートはまだありません。</p>
        ) : (
          <ul className="reports">
            {detail.reports.map((report) => (
              <li key={report.filename}>
                <p className="rname">
                  {report.filename}（{report.sizeBytes} バイト）
                </p>
                <pre className="rbody">{report.content}</pre>
              </li>
            ))}
          </ul>
        )}
      </section>

      <section aria-labelledby="h-instructions">
        <h2 className="sec" id="h-instructions">
          指示書
        </h2>
        {detail.instructions.length === 0 ? (
          <p className="note">指示書はありません。</p>
        ) : (
          <ul className="instructions">
            {detail.instructions.map((instruction) => (
              <li key={instruction.instructionId}>
                <div className="ins-head">
                  <b>#{instruction.instructionId}</b>
                  <span>{instruction.state}</span>
                  <span>{instruction.issuedAt ? formatDateTime(instruction.issuedAt) : "発行時刻は不明です。"}</span>
                </div>
                <p>{instruction.content}</p>
              </li>
            ))}
          </ul>
        )}
      </section>

      <a className="back" href="/projects">
        プロジェクト一覧に戻る
      </a>
    </main>
  );
}

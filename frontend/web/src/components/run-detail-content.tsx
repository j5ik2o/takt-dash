import type { ReactElement } from "react";
import type { GetRunDetailResponse } from "../proto/taktdash";
import { formatClock, formatDateTime, formatDuration } from "../state/format";
import { LiveRunLog } from "./live-run-log";
import { projectHref } from "../state/anchors";
import { RunContentToggle } from "./run-content-toggle";

type RunDetailContentProps = {
  detail: GetRunDetailResponse;
  now: Date;
  serverName?: string;
};

const STEP_STATUS_LABELS: Readonly<Record<string, string>> = {
  done: "完了",
  blocked: "止まった",
  error: "エラー",
  rate_limited: "利用上限",
  running: "実行中",
  pending: "未着手",
};
export const stepStatusLabel = (status: string): string => STEP_STATUS_LABELS[status] ?? status;

const INSTRUCTION_STATE_LABELS: Readonly<Record<string, string>> = {
  pending: "未配信",
  deliveredSameSession: "配信済み（同じセッション）",
  deliveredNextStep: "配信済み（次のステップ）",
  unconsumedWarned: "未消費（警告済み）",
};
export const instructionStateLabel = (state: string): string => INSTRUCTION_STATE_LABELS[state] ?? state;

// run 詳細は状態ストアと run の保存場所から backend が組み立てた範囲だけを表示する。
// ログは末尾（logTail）から示し、開いている間は LiveRunLog が追記分を足す。JSONL の生ログは表示しない。レポートと指示書は一覧だけを
// 描画し、本文は利用者が項目を開いたときに RunContentToggle が取得する。
export function RunDetailContent({ detail, now, serverName }: RunDetailContentProps): ReactElement {
  const back = detail.goalId
    ? { text: "ゴール詳細に戻る", href: `/goals/${encodeURIComponent(detail.goalId)}` }
    : { text: "プロジェクトに戻る", href: projectHref(detail.project) };
  const elapsed =
    detail.startedAt !== undefined ? formatDuration((now.getTime() - detail.startedAt.getTime()) / 1000) : undefined;
  return (
    <main className="page">
      <a className="back" href={back.href}>
        {back.text}
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
            <div className="v mono">{serverName ?? detail.serverId}</div>
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
        {detail.stepLogs.length === 0 ? (
          <p className="note">ステップはまだありません。</p>
        ) : (
        <ul className="steps">
          {detail.stepLogs.map((step, index) => (
            <li key={`${step.stepName}-${index}`}>
              <div className="step-head">
                <b>{step.stepName}</b>
                <span>{step.persona}</span>
                <span>{stepStatusLabel(step.status)}</span>
                <span>反復 {step.iteration} 回目</span>
              </div>
              <pre className="step-body">{step.content}</pre>
            </li>
          ))}
        </ul>
        )}
      </section>

      <section aria-labelledby="h-log">
        <h2 className="sec" id="h-log">
          ログの末尾
        </h2>
        <LiveRunLog
          key={detail.runSlug}
          runSlug={detail.runSlug}
          logTail={detail.logTail}
          logsRemoved={detail.logsRemoved}
        />
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
                <RunContentToggle runSlug={detail.runSlug} target={{ kind: "report", filename: report.filename }} />
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
                  <span>{instructionStateLabel(instruction.state)}</span>
                  <span>{instruction.issuedAt ? formatDateTime(instruction.issuedAt) : "発行時刻は不明です。"}</span>
                </div>
                <RunContentToggle
                  runSlug={detail.runSlug}
                  target={{ kind: "instruction", instructionId: instruction.instructionId }}
                />
              </li>
            ))}
          </ul>
        )}
      </section>

      <a className="back" href={back.href}>
        {back.text}
      </a>
    </main>
  );
}

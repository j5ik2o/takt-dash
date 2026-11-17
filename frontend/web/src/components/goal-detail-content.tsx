import type { ReactElement } from "react";
import Link from "next/link";
import type { GetGoalDetailResponse, GoalQuestion, GoalTask, TaskStatusCounts } from "../proto/taktdash";
import { taskStatusLabel } from "../state/dashboard";
import { goalStateSentence } from "../state/goal-badge";
import { TASK_STATUS } from "../state/proto-values";
import { formatDateTime } from "../state/format";

type GoalDetailContentProps = {
  detail: GetGoalDetailResponse;
};

const listOrEmpty = (items: readonly string[], emptyMessage: string): ReactElement =>
  items.length === 0 ? (
    <p className="note">{emptyMessage}</p>
  ) : (
    <ul>
      {items.map((item, index) => (
        <li key={`${index}-${item}`}>{item}</li>
      ))}
    </ul>
  );

// 質問への回答は本文・判断・作業報告のいずれかで返る。どれも無いときだけ未回答とする。
const answerLines = (question: GoalQuestion): string[] => {
  const lines: string[] = [];
  if (question.answerText !== undefined) {
    lines.push(question.answerText);
  }
  if (question.answerDecision !== undefined) {
    lines.push(`回答の判断は ${question.answerDecision} です。`);
  }
  if (question.answerReport !== undefined) {
    lines.push(`作業の報告は ${question.answerReport} です。`);
  }
  return lines;
};

// 詳細の状態の文面はゴール一覧と同じ対応表（goalStateSentence）で作る。詳細の
// 応答には件数の集計が無いため、タスク一覧から状態別の件数を数えて一覧と同じ
// 入力にそろえる（issue #198）。
const taskCountsOf = (tasks: readonly GoalTask[]): TaskStatusCounts => {
  const counts: TaskStatusCounts = { pending: 0, running: 0, completed: 0, failed: 0, exceeded: 0, prFailed: 0 };
  for (const task of tasks) {
    switch (task.status) {
      case TASK_STATUS.PENDING:
        counts.pending += 1;
        break;
      case TASK_STATUS.RUNNING:
        counts.running += 1;
        break;
      case TASK_STATUS.COMPLETED:
        counts.completed += 1;
        break;
      case TASK_STATUS.FAILED:
        counts.failed += 1;
        break;
      case TASK_STATUS.EXCEEDED:
        counts.exceeded += 1;
        break;
      case TASK_STATUS.PR_FAILED:
        counts.prFailed += 1;
        break;
      default:
        break;
    }
  }
  return counts;
};

// ゴール詳細は proto の GetGoalDetailResponse が返す範囲だけを表示する。
// 開始ブランチは proto にフィールドがないため、値ではなく欠落が分かる表示にする。
export function GoalDetailContent({ detail }: GoalDetailContentProps): ReactElement {
  const runSlugs = Array.from(
    new Set(detail.tasks.map((task) => task.runSlug).filter((slug): slug is string => slug !== undefined)),
  );
  return (
    <main className="page">
      <Link className="back" href="/goals">
        ゴール一覧に戻る
      </Link>
      <header className="page-head">
        <div>
          <h1>ゴール {detail.goalId}</h1>
          <p>
            {detail.project} のゴールです。
            {goalStateSentence({
              status: detail.status,
              executionStatus: detail.executionStatus,
              taskCounts: taskCountsOf(detail.tasks),
            })}
          </p>
        </div>
      </header>

      <section aria-labelledby="h-objective">
        <h2 className="sec" id="h-objective">
          目的
        </h2>
        <p>{detail.objective}</p>
      </section>

      <section aria-labelledby="h-out-of-scope">
        <h2 className="sec" id="h-out-of-scope">
          対象外
        </h2>
        {listOrEmpty(detail.outOfScope, "対象外はありません。")}
      </section>

      <section aria-labelledby="h-acceptance">
        <h2 className="sec" id="h-acceptance">
          受け入れ条件
        </h2>
        {listOrEmpty(detail.acceptanceCriteria, "受け入れ条件はありません。")}
      </section>

      <section aria-labelledby="h-constraints">
        <h2 className="sec" id="h-constraints">
          制約
        </h2>
        {listOrEmpty(detail.constraints, "制約はありません。")}
      </section>

      <section aria-labelledby="h-branches">
        <h2 className="sec" id="h-branches">
          ブランチ
        </h2>
        <ul className="kv">
          <li>
            <span>開始ブランチ</span>
            <b>未取得</b>
          </li>
          <li>
            <span>統合ブランチ</span>
            <b>{detail.integrationBranch}</b>
          </li>
          <li>
            <span>ゴールブランチ</span>
            <b>{detail.branch}</b>
          </li>
        </ul>
      </section>

      <section aria-labelledby="h-work-log">
        <h2 className="sec" id="h-work-log">
          作業記録
        </h2>
        {detail.workLog.length === 0 ? (
          <p className="note">作業記録はありません。</p>
        ) : (
          <ol className="steps">
            {detail.workLog.map((entry) => (
              <li key={entry.workLogId}>
                <div className="step-head">
                  <span>{entry.recordedAt === undefined ? "時刻は未取得" : formatDateTime(entry.recordedAt)}</span>
                </div>
                <p>{entry.summary}</p>
              </li>
            ))}
          </ol>
        )}
      </section>

      <section aria-labelledby="h-questions">
        <h2 className="sec" id="h-questions">
          質問と回答
        </h2>
        {detail.questions.length === 0 ? (
          <p className="note">質問はありません。</p>
        ) : (
          <ul className="instructions">
            {detail.questions.map((question) => {
              const answers = answerLines(question);
              return (
                <li key={question.questionId}>
                  <div className="ins-head">
                    <b>{question.kind}</b>
                    <span>{question.status}</span>
                    <span>{question.createdAt === undefined ? "時刻は未取得" : formatDateTime(question.createdAt)}</span>
                  </div>
                  <p>{question.body}</p>
                  {answers.length === 0 ? (
                    <p>未回答</p>
                  ) : (
                    answers.map((answer, index) => <p key={`${index}-${answer}`}>{answer}</p>)
                  )}
                </li>
              );
            })}
          </ul>
        )}
      </section>

      <section aria-labelledby="h-tasks">
        <h2 className="sec" id="h-tasks">
          タスク一覧
        </h2>
        {detail.tasks.length === 0 ? (
          <p className="note">タスクはありません。</p>
        ) : (
          <ul className="steps">
            {detail.tasks.map((task) => (
              <li key={task.taskName}>
                <div className="step-head">
                  <a
                    href={`/tasks/${encodeURIComponent(detail.project)}?task=${encodeURIComponent(
                      task.taskName,
                    )}&goal=${encodeURIComponent(detail.goalId)}`}
                  >
                    {task.taskName}
                  </a>
                  <span>{taskStatusLabel(task.status)}</span>
                </div>
              </li>
            ))}
          </ul>
        )}
      </section>

      <section aria-labelledby="h-runs">
        <h2 className="sec" id="h-runs">
          紐づく run
        </h2>
        {runSlugs.length === 0 ? (
          <p className="note">紐づく run はありません。</p>
        ) : (
          <ul className="mini-runs">
            {runSlugs.map((slug) => (
              <li key={slug}>
                <a className="view" href={`/runs/${encodeURIComponent(slug)}`}>
                  {slug}
                </a>
              </li>
            ))}
          </ul>
        )}
      </section>

      <Link className="back" href="/goals">
        ゴール一覧に戻る
      </Link>
    </main>
  );
}

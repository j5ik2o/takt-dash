import type { ReactElement } from "react";
import Link from "next/link";
import type { GetTaskDetailResponse } from "../proto/taktdash";
import { taskStatusLabel } from "../state/dashboard";
import { formatDateTime } from "../state/format";

type TaskDetailContentProps = {
  detail: GetTaskDetailResponse;
};

// タスク詳細は proto の GetTaskDetailResponse が返す範囲だけを表示する。
// 作成・開始・完了の時刻は takt_list_tasks の createdAt / startedAt / completedAt に対応し、
// 値が無い時刻は「未取得」と示す。
export function TaskDetailContent({ detail }: TaskDetailContentProps): ReactElement {
  const runSlugs = Array.from(
    new Set([
      ...detail.linkedRunSlugs,
      ...(detail.runSlug === undefined ? [] : [detail.runSlug]),
    ]),
  );
  return (
    <main className="page">
      <Link className="back" href="/goals">
        ゴール一覧に戻る
      </Link>
      <header className="page-head">
        <div>
          <h1>{detail.taskName}</h1>
          <p>
            {detail.project} のタスクです。状態は{taskStatusLabel(detail.status)}です。
          </p>
        </div>
      </header>

      <section className="summary" aria-label="タスクの要約">
        <div className="sum-grid">
          <div className="f">
            <div className="k">状態</div>
            <div className="v">{taskStatusLabel(detail.status)}</div>
          </div>
          <div className="f wide">
            <div className="k">目的</div>
            <div className="v">{detail.taskName}</div>
          </div>
          <div className="f wide">
            <div className="k">要約</div>
            <div className="v">{detail.summary}</div>
          </div>
        </div>
      </section>

      <section aria-labelledby="h-branch">
        <h2 className="sec" id="h-branch">
          ブランチとプルリクエスト
        </h2>
        <ul className="kv">
          <li>
            <span>ブランチ</span>
            <b>{detail.branch === undefined ? "未取得" : detail.branch}</b>
          </li>
          <li>
            <span>プルリクエスト</span>
            <b>
              {detail.pullRequestNumber === undefined ? (
                "未取得"
              ) : detail.pullRequestUrl === undefined ? (
                `#${detail.pullRequestNumber}`
              ) : (
                <a href={detail.pullRequestUrl}>#{detail.pullRequestNumber}</a>
              )}
            </b>
          </li>
        </ul>
      </section>

      <section aria-labelledby="h-times">
        <h2 className="sec" id="h-times">
          時刻
        </h2>
        <ul className="kv">
          <li>
            <span>作成時刻</span>
            <b>{detail.createdAt === undefined ? "未取得" : formatDateTime(detail.createdAt)}</b>
          </li>
          <li>
            <span>開始時刻</span>
            <b>{detail.startedAt === undefined ? "未取得" : formatDateTime(detail.startedAt)}</b>
          </li>
          <li>
            <span>完了時刻</span>
            <b>{detail.completedAt === undefined ? "未取得" : formatDateTime(detail.completedAt)}</b>
          </li>
        </ul>
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

"use client";

import { useEffect, useRef, useState, type ReactElement } from "react";
import { formatClock } from "../state/format";
import {
  initialRunLogView,
  restoreRunUpdates,
  runUpdatesUrl,
  type RunLogCursor,
  type RunSessionActivityView,
  type RunUpdates,
} from "../state/run-log-stream";
import { startRunLogUpdates } from "./run-log-updates";

type LiveRunLogProps = {
  runSlug: string;
  logTail: string;
  logsRemoved: boolean;
};

const requestUpdates = async (
  runSlug: string,
  cursor: RunLogCursor | undefined,
  signal: AbortSignal,
): Promise<RunUpdates> => {
  const response = await fetch(runUpdatesUrl(runSlug, cursor), { cache: "no-store", signal });
  if (!response.ok) {
    throw new Error(`ログの追記分を取得できませんでした（${response.status}）。`);
  }
  return restoreRunUpdates(await response.json());
};

const activityText = (activity: RunSessionActivityView | undefined): string => {
  if (activity === undefined || (activity.latestTool === undefined && activity.updatedAt === undefined)) {
    return "セッションログの活動はまだありません。";
  }
  const tool = activity.latestTool === undefined ? "直近の道具はまだありません。" : `直近の道具は ${activity.latestTool} です。`;
  const updated = activity.updatedAt === undefined ? "" : `セッションログの更新は ${formatClock(activity.updatedAt)} です。`;
  return `${tool}道具の呼び出しは ${activity.toolUseCount} 件です。${updated}`;
};

// run 詳細のログの末尾を、RSC が取得した log_tail から描画し、ページを開いている間は 1 秒ごとに
// BFF の Route Handler から追記分を取って同じ一覧へ足す。ページ全体の再読み込みはしない。
//
// 画面の自動更新（router.refresh）は run 詳細を取り直し、新しい logTail を渡してくる。積んだ行と
// 続きの位置はこのコンポーネントが持つ状態なので、props の logTail と logsRemoved は最初の描画に
// だけ使い、ポーリングは張り直さない。別の run は呼び出し側が key で作り直す。
export function LiveRunLog({ runSlug, logTail, logsRemoved }: LiveRunLogProps): ReactElement {
  const [view, setView] = useState(() => initialRunLogView(logTail, logsRemoved));
  const [disconnected, setDisconnected] = useState(false);
  const viewRef = useRef(view);

  useEffect(() => {
    const controller = new AbortController();
    const polling = startRunLogUpdates({
      signal: controller.signal,
      intervalMs: 1_000,
      initialView: viewRef.current,
      fetchUpdates: (cursor) => requestUpdates(runSlug, cursor, controller.signal),
      onView: (next) => {
        viewRef.current = next;
        setView(next);
        setDisconnected(false);
      },
      onError: () => {
        setDisconnected(true);
      },
    });
    return () => {
      controller.abort();
      polling.stop();
    };
  }, [runSlug]);

  if (view.logsRemoved) {
    return (
      <p className="note">ログは残っていません。run の保存場所が掃除されたため、画面はステップとログを表示できません。</p>
    );
  }
  return (
    <>
      {disconnected ? (
        <p className="banner warn" role="alert">
          ログの追記分を取得できません。前回取得したログを表示しています。
        </p>
      ) : null}
      <ol className="log-tail" aria-label="ログの行">
        {view.lines.map((line, index) => (
          <li key={line.sequence ?? `initial-${index}`}>{line.text}</li>
        ))}
      </ol>
      <p className="note">{activityText(view.activity)}</p>
    </>
  );
}

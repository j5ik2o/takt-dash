"use client";

import { useEffect, useState, type ReactElement } from "react";
import type { GetTaskDetailResponse } from "../proto/taktdash";
import { restoreTaskDetail } from "../state/restore-state";
import { TaskDetailContent } from "./task-detail-content";
import { startPolling } from "./polling";

type LiveTaskDetailProps = {
  initialDetail: GetTaskDetailResponse;
  project: string;
  taskName: string;
  goalId?: string;
};

const taskDetailUrl = (project: string, taskName: string, goalId?: string): string => {
  const base = `/api/tasks/${encodeURIComponent(project)}?task=${encodeURIComponent(taskName)}`;
  return goalId === undefined ? base : `${base}&goal=${encodeURIComponent(goalId)}`;
};

// 詳細は RSC が取得した初期表示をそのまま使い、1秒を基本に Route Handler から
// 取得した値だけを差し替える。ページ全体の再読み込みはしない。
export function LiveTaskDetail({
  initialDetail,
  project,
  taskName,
  goalId,
}: LiveTaskDetailProps): ReactElement {
  const [detail, setDetail] = useState(initialDetail);
  const [disconnected, setDisconnected] = useState(false);

  useEffect(() => {
    const controller = new AbortController();
    const polling = startPolling({
      intervalMs: 1_000,
      signal: controller.signal,
      load: async () => {
        const response = await fetch(taskDetailUrl(project, taskName, goalId), {
          signal: controller.signal,
        });
        if (!response.ok) {
          throw new Error(`タスク詳細を取得できませんでした（${response.status}）。`);
        }
        return restoreTaskDetail(await response.json());
      },
      onValue: (next) => {
        setDetail(next);
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
  }, [goalId, project, taskName]);

  return (
    <>
      {disconnected ? (
        <p className="banner warn" role="alert">
          最新の状態を取得できません。前回取得した内容を表示しています。
        </p>
      ) : null}
      <TaskDetailContent detail={detail} />
    </>
  );
}

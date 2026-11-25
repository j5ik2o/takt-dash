"use client";

import { useEffect, useState, type ReactElement } from "react";
import type { GetGoalDetailResponse } from "../proto/taktdash";
import { restoreGoalDetail } from "../state/restore-state";
import { GoalDetailContent } from "./goal-detail-content";
import { startPolling } from "./polling";

type LiveGoalDetailProps = {
  initialDetail: GetGoalDetailResponse;
  goalId: string;
  /** 前に読めた内容で描画し、切断の通知を出した状態で始めるか（issue #309）。 */
  initialDisconnected?: boolean;
};

// 詳細は RSC が取得した初期表示をそのまま使い、1秒を基本に Route Handler から
// 取得した値だけを差し替える。ページ全体の再読み込みはしない。
export function LiveGoalDetail({
  initialDetail,
  goalId,
  initialDisconnected = false,
}: LiveGoalDetailProps): ReactElement {
  const [detail, setDetail] = useState(initialDetail);
  const [disconnected, setDisconnected] = useState(initialDisconnected);

  useEffect(() => {
    const controller = new AbortController();
    const polling = startPolling({
      intervalMs: 1_000,
      signal: controller.signal,
      load: async () => {
        const response = await fetch(`/api/goals/${encodeURIComponent(goalId)}`, {
          signal: controller.signal,
        });
        if (!response.ok) {
          throw new Error(`ゴール詳細を取得できませんでした（${response.status}）。`);
        }
        return restoreGoalDetail(await response.json());
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
  }, [goalId]);

  return (
    <>
      {disconnected ? (
        <p className="banner warn" role="alert">
          最新の状態を取得できません。前回取得した内容を表示しています。
        </p>
      ) : null}
      <GoalDetailContent detail={detail} />
    </>
  );
}

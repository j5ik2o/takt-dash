"use client";

import type { ReactElement } from "react";
import type { BackendUnavailableSituation } from "../grpc/client";
import { useLiveState } from "./state-provider";
import { GoalsContent } from "./goals-content";

type LiveGoalsProps = {
  project?: string;
};

// ゴール一覧は既存の WatchState の SSE で更新される state.goals を使う。
// 一覧のために新しいポーリング経路は増やさない。
export function LiveGoals({ project }: LiveGoalsProps): ReactElement {
  const { state, disconnected, failure, disconnectedSince, respondedBefore } = useLiveState();
  const now = new Date();
  // 切断の文面は、失敗理由と接続履歴と切断が続いている時間から決める
  // （issue #152、#92）。失敗理由が無い切断は従来どおり接続不可として扱う。
  const unavailableSituation: BackendUnavailableSituation = {
    failure: failure ?? "unreachable",
    respondedBefore: respondedBefore ?? true,
    ...(disconnectedSince !== undefined && disconnectedSince !== null
      ? { durationMs: Math.max(0, now.getTime() - disconnectedSince.getTime()) }
      : {}),
  };
  return (
    <GoalsContent
      state={state}
      project={project}
      disconnected={disconnected}
      unavailableSituation={unavailableSituation}
    />
  );
}

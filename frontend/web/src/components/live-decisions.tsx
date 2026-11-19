"use client";

import { useSyncExternalStore, type ReactElement } from "react";
import type { BackendUnavailableSituation } from "../grpc/client";
import { useLiveState } from "./state-provider";
import { DecisionsContent } from "./decisions-content";

type LiveDecisionsProps = {
  initialNowMs: number;
};

// 初回描画だけはサーバーの時刻を使う。以後の再描画では現在時刻を評価する。
// ハイドレーション以外に購読する外部イベントはない。
const subscribe = () => () => {};

export function LiveDecisions({ initialNowMs }: LiveDecisionsProps): ReactElement {
  const { state, disconnected, failure, disconnectedSince, respondedBefore } = useLiveState();
  const hydrating = useSyncExternalStore(subscribe, () => false, () => true);
  const now = hydrating ? new Date(initialNowMs) : new Date();
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
    <DecisionsContent
      state={state}
      now={now}
      disconnected={disconnected}
      readAt={state.readAt}
      unavailableSituation={unavailableSituation}
    />
  );
}

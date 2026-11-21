"use client";

import type { ReactElement } from "react";
import type { BackendUnavailableSituation } from "../grpc/client";
import { useLiveState } from "./state-provider";
import { DecisionsContent } from "./decisions-content";

export function LiveDecisions(): ReactElement {
  const { state, snapshotLoaded, disconnected, failure, disconnectedSince, respondedBefore } = useLiveState();
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
    <DecisionsContent
      state={state}
      snapshotLoaded={snapshotLoaded}
      now={now}
      disconnected={disconnected}
      readAt={state.readAt}
      unavailableSituation={unavailableSituation}
    />
  );
}

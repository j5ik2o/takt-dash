"use client";

import type { ReactElement } from "react";
import type { BackendUnavailableSituation } from "../grpc/client";
import { useLiveState } from "./state-provider";
import { DecisionsContent } from "./decisions-content";

export function LiveDecisions(): ReactElement {
  const { state, disconnected, disconnectedSince, respondedBefore } = useLiveState();
  const now = new Date();
  // 切断の文面は、接続履歴と切断が続いている時間から決める（issue #152）。
  const unavailableSituation: BackendUnavailableSituation = {
    failure: "unreachable",
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

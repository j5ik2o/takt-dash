"use client";

import type { ReactElement } from "react";
import { useLiveState } from "./state-provider";
import { DecisionsContent } from "./decisions-content";

export function LiveDecisions(): ReactElement {
  const { state, disconnected } = useLiveState();
  return <DecisionsContent state={state} now={new Date()} disconnected={disconnected} />;
}

"use client";

import type { ReactElement } from "react";
import { useLiveState } from "./state-provider";
import { DecisionsContent } from "./decisions-content";

export function LiveDecisions(): ReactElement {
  const { state } = useLiveState();
  return <DecisionsContent state={state} now={new Date()} />;
}

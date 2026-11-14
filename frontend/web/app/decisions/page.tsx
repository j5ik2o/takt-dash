import type { ReactElement } from "react";
import type { DashboardState } from "@/src/proto/taktdash";
import { DecisionsContent } from "@/src/components/decisions-content";
import { LiveDecisions } from "@/src/components/live-decisions";

export type DecisionsPageProps = {
  state?: DashboardState;
  now?: Date;
};

export default function DecisionsPage({ state, now }: DecisionsPageProps): ReactElement {
  if (state !== undefined) {
    return <DecisionsContent state={state} now={now ?? new Date()} />;
  }
  return <LiveDecisions />;
}

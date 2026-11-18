import type { ReactElement } from "react";
import type { DashboardState, GetActivityResponse } from "@/src/proto/taktdash";
import { ActivityContent } from "@/src/components/activity-content";
import { ActivityLoader } from "@/src/components/activity-loader";

export type ActivityPageProps = {
  state?: DashboardState;
  response?: GetActivityResponse;
  now?: Date;
};

export default function ActivityPage({ state, response, now }: ActivityPageProps): ReactElement {
  if (state !== undefined && response !== undefined) {
    return <ActivityContent state={state} response={response} now={now ?? new Date()} />;
  }
  return <ActivityLoader />;
}

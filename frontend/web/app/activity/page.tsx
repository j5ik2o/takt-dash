import type { ReactElement } from "react";
import type { GetActivityResponse } from "@/src/proto/taktdash";
import { ActivityContent } from "@/src/components/activity-content";
import { ActivityLoader } from "@/src/components/activity-loader";

export type ActivityPageProps = {
  response?: GetActivityResponse;
  now?: Date;
};

export default function ActivityPage({ response, now }: ActivityPageProps): ReactElement {
  if (response !== undefined) {
    return <ActivityContent response={response} now={now ?? new Date()} />;
  }
  return <ActivityLoader />;
}

import type { ReactElement } from "react";
import type { OverviewView } from "@/src/state/dashboard";
import { OverviewContent } from "@/src/components/overview-content";
import { LiveOverview } from "@/src/components/live-overview";

export type OverviewPageProps = {
  view?: OverviewView;
};

export default function OverviewPage({ view }: OverviewPageProps): ReactElement {
  if (view !== undefined) {
    return <OverviewContent view={view} />;
  }
  return <LiveOverview />;
}

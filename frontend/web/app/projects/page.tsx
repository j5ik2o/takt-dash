import type { ReactElement } from "react";
import type { DashboardState } from "@/src/proto/taktdash";
import { ProjectsContent } from "@/src/components/projects-content";
import { ProjectsLoader } from "@/src/components/projects-loader";

export type ProjectsPageProps = {
  state?: DashboardState;
  now?: Date;
};

export default function ProjectsPage({ state, now }: ProjectsPageProps): ReactElement {
  if (state !== undefined) {
    return <ProjectsContent state={state} now={now ?? new Date()} />;
  }
  return <ProjectsLoader />;
}

import type { ReactElement } from "react";
import type { GetProjectsResponse } from "@/src/proto/taktdash";
import { ProjectsContent } from "@/src/components/projects-content";
import { ProjectsLoader } from "@/src/components/projects-loader";

export type ProjectsPageProps = {
  response?: GetProjectsResponse;
  now?: Date;
};

export default function ProjectsPage({ response, now }: ProjectsPageProps): ReactElement {
  if (response !== undefined) {
    return <ProjectsContent response={response} now={now ?? new Date()} />;
  }
  return <ProjectsLoader />;
}

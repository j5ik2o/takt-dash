import type { ReactElement } from "react";
import { LiveGoals } from "@/src/components/live-goals";

export type GoalsPageProps = {
  searchParams: Promise<{ project?: string | string[] }>;
};

export default async function GoalsPage({ searchParams }: GoalsPageProps): Promise<ReactElement> {
  const query = await searchParams;
  const project = Array.isArray(query.project) ? query.project[0] : query.project;
  return <LiveGoals project={project} />;
}

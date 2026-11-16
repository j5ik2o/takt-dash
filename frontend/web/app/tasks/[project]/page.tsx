import type { ReactElement } from "react";
import { TaskDetailLoader } from "@/src/components/task-detail-loader";

export type TaskDetailPageProps = {
  params: Promise<{ project: string }>;
  searchParams: Promise<{ task?: string | string[]; goal?: string | string[] }>;
};

export default function TaskDetailPage({ params, searchParams }: TaskDetailPageProps): ReactElement {
  return <TaskDetailLoader params={params} searchParams={searchParams} />;
}

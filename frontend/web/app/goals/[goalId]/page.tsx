import type { ReactElement } from "react";
import { GoalDetailLoader } from "@/src/components/goal-detail-loader";

export type GoalDetailPageProps = {
  params: Promise<{ goalId: string }>;
};

export default function GoalDetailPage({ params }: GoalDetailPageProps): ReactElement {
  return <GoalDetailLoader params={params} />;
}

import type { ReactElement } from "react";
import { RunDetailLoader } from "@/src/components/run-detail-loader";

export type RunDetailPageProps = {
  params: Promise<{ slug: string }>;
};

export default function RunDetailPage({ params }: RunDetailPageProps): ReactElement {
  return <RunDetailLoader params={params} />;
}

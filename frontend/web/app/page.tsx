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
  // SSR とハイドレーションで同じ初期時刻を渡すため、サーバー側で数値にしてから client へ渡す。
  // eslint-disable-next-line react-hooks/purity -- 表示の再計算ではなく、初回描画の基準時刻を1度だけ決める
  return <LiveOverview initialNowMs={Date.now()} />;
}

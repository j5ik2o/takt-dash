import type { ReactElement } from "react";
import type { DashboardState } from "@/src/proto/taktdash";
import { DecisionsContent } from "@/src/components/decisions-content";
import { LiveDecisions } from "@/src/components/live-decisions";

export type DecisionsPageProps = {
  state?: DashboardState;
  now?: Date;
};

export default function DecisionsPage({ state, now }: DecisionsPageProps): ReactElement {
  if (state !== undefined) {
    return <DecisionsContent state={state} snapshotLoaded={true} now={now ?? new Date()} disconnected={false} />;
  }
  // SSR とハイドレーションで待ち時間を一致させ、入力欄の再生成を防ぐ。
  // eslint-disable-next-line react-hooks/purity -- サーバーで初回描画の基準時刻を1度だけ決める
  return <LiveDecisions initialNowMs={Date.now()} />;
}

import type { Metadata } from "next";
import "./globals.css";
import {
  BackendUnavailableError,
  createBackendClient,
  hasBackendResponded,
} from "@/src/grpc/client";
import { DashboardState } from "@/src/proto/taktdash";
import { resolveThresholds } from "@/src/state/dashboard";
import { StateStreamProviderWithRefresh } from "@/src/components/state-provider";
import { LiveSidebar } from "@/src/components/live-sidebar";
import { normalizeBuildInfo } from "@/src/state/build-info";

export const metadata: Metadata = {
  title: "taktDash",
  description: "taktDash は、takt manager 群の状態を集約する読み取り用のダッシュボードです。",
};

export const dynamic = "force-dynamic";

const FRONTEND_BUILD = normalizeBuildInfo({
  commit: process.env.TAKTDASH_FRONTEND_COMMIT,
  branch: process.env.TAKTDASH_FRONTEND_BRANCH,
});

const EMPTY_STATE = DashboardState.create({
  thresholds: resolveThresholds(undefined),
});

// 最後に取得できた概要の状態。読み込みに失敗しても画面を空にせず、この内容を
// 取得時刻つきで残す（issue #152）。一度も取得できていない場合は空の状態を使う。
let lastLoadedState: DashboardState | undefined;

async function loadInitialState(): Promise<{
  state: DashboardState;
  unavailable: boolean;
  respondedBefore: boolean;
}> {
  const client = createBackendClient();
  try {
    const state = await client.getOverview();
    lastLoadedState = state;
    return { state, unavailable: false, respondedBefore: hasBackendResponded() };
  } catch (error) {
    if (error instanceof BackendUnavailableError) {
      // 今回の読み込みは失敗しても、この Web プロセスが過去に応答を受けて
      // いれば接続履歴は応答ありのままにする（issue #152）。キャッシュを返す
      // 経路でも履歴を落とさないよう、成否とは別に履歴を返す。
      return {
        state: lastLoadedState ?? EMPTY_STATE,
        unavailable: true,
        respondedBefore: hasBackendResponded(),
      };
    }
    throw error;
  } finally {
    await client.close();
  }
}

export default async function RootLayout({ children }: LayoutProps<"/">) {
  const { state, unavailable, respondedBefore } = await loadInitialState();
  return (
    <html lang="ja">
      <body>
        <StateStreamProviderWithRefresh
          initialState={state}
          initialUnavailable={unavailable}
          initialRespondedBefore={respondedBefore}
        >
          <div className="shell">
            <LiveSidebar frontendBuild={FRONTEND_BUILD} />
            <div className="content">
              {/* backend 不通の通知は、画面側の内容が切断の表示として描画する。
                  レイアウトでは描画しない（issue #92）。 */}
              {children}
            </div>
          </div>
        </StateStreamProviderWithRefresh>
      </body>
    </html>
  );
}

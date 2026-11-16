import type { Metadata } from "next";
import "./globals.css";
import { loadServerState } from "@/src/state/server-state";
import { measurePage } from "@/src/telemetry/timing";
import { StateStreamProviderWithRefresh } from "@/src/components/state-provider";
import { LiveSidebar } from "@/src/components/live-sidebar";
import { BackendUnavailableNotice } from "@/src/components/backend-unavailable-notice";
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

export default async function RootLayout({ children }: LayoutProps<"/">) {
  // layout は全ページで実行されるため、ここで各ページのサーバー側 RPC（GetOverview）と
  // ページ応答の内訳を page=layout の1行として記録する。状態は要求内で共有し、
  // ページ側はこれを再利用する。
  const { state, unavailable } = await measurePage("layout", async (timing) => {
    const loaded = await loadServerState();
    timing.recordRpc({ rpc: "GetOverview", durationMs: loaded.rpcMs });
    return loaded;
  });
  return (
    <html lang="ja">
      <body>
        <StateStreamProviderWithRefresh initialState={state}>
          <div className="shell">
            <LiveSidebar frontendBuild={FRONTEND_BUILD} />
            <div className="content">
              {unavailable ? <BackendUnavailableNotice /> : null}
              {children}
            </div>
          </div>
        </StateStreamProviderWithRefresh>
      </body>
    </html>
  );
}

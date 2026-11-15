import type { Metadata } from "next";
import "./globals.css";
import { loadServerState } from "@/src/state/server-state";
import { measurePage } from "@/src/telemetry/timing";
import { StateStreamProvider } from "@/src/components/state-provider";
import { Sidebar } from "@/src/components/sidebar";
import { BackendUnavailableNotice } from "@/src/components/backend-unavailable-notice";

export const metadata: Metadata = {
  title: "taktDash",
  description: "taktDash は、takt manager 群の状態を集約する読み取り用のダッシュボードです。",
};

export const dynamic = "force-dynamic";

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
        <div className="shell">
          <Sidebar servers={state.servers} pendingCount={state.pendingDecisions.length} />
          <div className="content">
            {unavailable ? <BackendUnavailableNotice /> : null}
            <StateStreamProvider initialState={state}>{children}</StateStreamProvider>
          </div>
        </div>
      </body>
    </html>
  );
}

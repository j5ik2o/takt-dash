import type { Metadata } from "next";
import "./globals.css";
import { BackendUnavailableError, createBackendClient } from "@/src/grpc/client";
import { DashboardState } from "@/src/proto/taktdash";
import { resolveThresholds } from "@/src/state/dashboard";
import { measurePage, type TimingSink } from "@/src/telemetry/timing";
import { StateStreamProvider } from "@/src/components/state-provider";
import { Sidebar } from "@/src/components/sidebar";
import { BackendUnavailableNotice } from "@/src/components/backend-unavailable-notice";

export const metadata: Metadata = {
  title: "taktDash",
  description: "taktDash は、takt manager 群の状態を集約する読み取り用のダッシュボードです。",
};

export const dynamic = "force-dynamic";

const EMPTY_STATE = DashboardState.create({
  thresholds: resolveThresholds(undefined),
});

async function loadInitialState(timing: TimingSink): Promise<{ state: DashboardState; unavailable: boolean }> {
  const client = createBackendClient({ timing });
  try {
    return { state: await client.getOverview(), unavailable: false };
  } catch (error) {
    if (error instanceof BackendUnavailableError) {
      return { state: EMPTY_STATE, unavailable: true };
    }
    throw error;
  } finally {
    await client.close();
  }
}

export default async function RootLayout({ children }: LayoutProps<"/">) {
  // layout は全ページで実行されるため、ここで各ページのサーバー側 RPC（GetOverview）と
  // ページ応答の内訳を page=layout の1行として記録する。
  const { state, unavailable } = await measurePage("layout", (timing) => loadInitialState(timing));
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

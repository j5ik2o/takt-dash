import type { Metadata } from "next";
import "./globals.css";
import { BackendUnavailableError, createBackendClient } from "@/src/grpc/client";
import { DashboardState } from "@/src/proto/taktdash";
import { resolveThresholds } from "@/src/state/dashboard";
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

const EMPTY_STATE = DashboardState.create({
  thresholds: resolveThresholds(undefined),
});

async function loadInitialState(): Promise<{ state: DashboardState; unavailable: boolean }> {
  const client = createBackendClient();
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
  const { state, unavailable } = await loadInitialState();
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

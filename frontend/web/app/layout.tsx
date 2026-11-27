import type { Metadata } from "next";
import "./globals.css";
import { hasBackendResponded } from "@/src/grpc/client";
import { loadServerState } from "@/src/state/server-state";
import { measurePage } from "@/src/telemetry/timing";
import { StateStreamProviderWithRefresh } from "@/src/components/state-provider";
import { LiveSidebar } from "@/src/components/live-sidebar";
import { normalizeBuildInfo } from "@/src/state/build-info";

export const metadata: Metadata = {
  title: "taktDash",
  description: "taktDash は、takt manager 群の状態を集約し、判断待ちを操作するダッシュボードです。",
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
  const { state, error, snapshotLoaded } = await measurePage("layout", async (timing) => {
    const loaded = await loadServerState();
    timing.recordRpc({ rpc: "GetOverview", durationMs: loaded.rpcMs });
    return loaded;
  });
  // 今回の読み込みは失敗しても、この Web プロセスが過去に応答を受けて
  // いれば接続履歴は応答ありのままにする（issue #152）。キャッシュを返す
  // 経路でも履歴を落とさないよう、成否とは別に履歴を渡す。失敗理由は
  // 画面側の通知の文面の振り分けに使うため、そのまま渡す（issue #92）。
  const failure = error?.failure;
  const respondedBefore = hasBackendResponded();
  return (
    <html lang="ja">
      <body>
        <StateStreamProviderWithRefresh
          initialState={state}
          initialSnapshotLoaded={snapshotLoaded}
          initialFailure={failure}
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

import { cache } from "react";
import { BackendUnavailableError, getBackendClient } from "../grpc/client";
import { DashboardState } from "../proto/taktdash";
import { resolveThresholds } from "./dashboard";

const EMPTY_STATE = DashboardState.create({
  thresholds: resolveThresholds(undefined),
});

export type ServerState = {
  state: DashboardState;
  unavailable: boolean;
  // GetOverview の往復時間。layout が page 行の rpc_ms に反映する。
  rpcMs: number;
};

// 要求内で1つの結果を共有する。引数を取らないのは、cache のキーを要求内で固定するため。
const load = cache(async (): Promise<ServerState> => {
  const startedAt = performance.now();
  try {
    return { state: await getBackendClient().getOverview(), unavailable: false, rpcMs: performance.now() - startedAt };
  } catch (error) {
    if (error instanceof BackendUnavailableError) {
      return { state: EMPTY_STATE, unavailable: true, rpcMs: performance.now() - startedAt };
    }
    throw error;
  }
});

/** 1回の要求で共有する状態を返す。最初の呼び出しだけが GetOverview を実行する。 */
export function loadServerState(): Promise<ServerState> {
  return load();
}

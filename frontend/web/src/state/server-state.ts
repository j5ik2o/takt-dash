import { cache } from "react";
import { BackendUnavailableError, getBackendClient } from "../grpc/client";
import { DashboardState } from "../proto/taktdash";
import { resolveThresholds } from "./dashboard";

const EMPTY_STATE = DashboardState.create({
  thresholds: resolveThresholds(undefined),
});

// 最後に取得できた概要の状態。読み込みに失敗しても画面を空にせず、この内容を
// 取得時刻つきで残す（issue #152）。一度も取得できていない場合は空の状態を使う。
// 要求をまたいで残すため、要求内の cache とは別にモジュールに持つ。
let lastLoadedState: DashboardState | undefined;

export type ServerState = {
  state: DashboardState;
  // 読み込みに失敗した理由。失敗していなければ undefined。画面側の通知の文面の
  // 振り分けに使う（issue #92）。
  error?: BackendUnavailableError;
  // GetOverview の往復時間。layout が page 行の rpc_ms に反映する。
  rpcMs: number;
};

// 要求内で1つの結果を共有する。引数を取らないのは、cache のキーを要求内で固定するため。
const load = cache(async (): Promise<ServerState> => {
  const startedAt = performance.now();
  try {
    const state = await getBackendClient().getOverview();
    lastLoadedState = state;
    return { state, rpcMs: performance.now() - startedAt };
  } catch (error) {
    if (error instanceof BackendUnavailableError) {
      return { state: lastLoadedState ?? EMPTY_STATE, error, rpcMs: performance.now() - startedAt };
    }
    throw error;
  }
});

/** 1回の要求で共有する状態を返す。最初の呼び出しだけが GetOverview を実行する。 */
export function loadServerState(): Promise<ServerState> {
  return load();
}

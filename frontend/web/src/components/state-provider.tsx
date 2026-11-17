"use client";

import { createContext, useCallback, useContext, useEffect, useState, type ReactNode } from "react";
import { useRouter } from "next/navigation";
import type { DashboardState as ProtoDashboardState } from "../proto/taktdash";
import { dashboardStateFromProto, type DashboardState } from "../state/restore-state";
import { createStateHandlers, openStateStream, type EventSourceLike } from "./state-stream";

export type StateStreamContextValue = {
  state: DashboardState;
  disconnected: boolean;
  /** 切断が始まった時刻。接続中は null。切断の継続時間で文面を変えるために使う。 */
  disconnectedSince?: Date | null;
  /**
   * 一度でも状態を受けたかどうか。受けたことがあれば backend への接続設定は
   * 正しいはずなので、切断時の文面から設定の確認を外す（issue #152）。
   */
  respondedBefore?: boolean;
};

const StateStreamContext = createContext<StateStreamContextValue | null>(null);

export function useLiveState(): StateStreamContextValue {
  const value = useContext(StateStreamContext);
  if (value === null) {
    throw new Error("useLiveState は StateStreamProvider の内側でのみ使えます。");
  }
  return value;
}

/** SSE の受信結果と切断を、Context 値の更新イベントへ写像する。 */
export type LiveStateEvent = { type: "state"; state: DashboardState } | { type: "error"; at?: Date };

/**
 * 状態ストリームの `onState`／`onError` を、Context 値の更新イベントへ結線する。
 *
 * `StateStreamProvider` の `useEffect` はこの関数で作ったハンドラを `openStateStream` へ
 * 渡します。本番とテストが同じ関数を通すため、`{type:"state"}`／`{type:"error"}` の
 * 取り違えや引数順の誤りはテストで検出できます。
 */
export function stateStreamHandlers(update: (event: LiveStateEvent) => void): {
  onState: (state: DashboardState) => void;
  onError: () => void;
} {
  return {
    onState: (state) => update({ type: "state", state }),
    onError: () => update({ type: "error", at: new Date() }),
  };
}

/**
 * SSE の受信結果から画面の Context 値を決める純粋関数。
 *
 * 受信で切断フラグを消し、エラーで立てます。
 */
export function reduceLiveState(current: StateStreamContextValue, event: LiveStateEvent): StateStreamContextValue {
  switch (event.type) {
    case "state":
      return { state: event.state, disconnected: false, disconnectedSince: null, respondedBefore: true };
    case "error":
      return {
        state: current.state,
        disconnected: true,
        disconnectedSince: current.disconnectedSince ?? event.at ?? null,
        respondedBefore: current.respondedBefore,
      };
    default:
      return current;
  }
}

type StateStreamProviderProps = {
  initialState: ProtoDashboardState;
  /** 初回の読み込みで backend に接続できなかったかどうか。接続履歴の初期値に使う。 */
  initialUnavailable?: boolean;
  revalidate?: () => void;
  children: ReactNode;
};

// 画面の状態と自動更新の所有者。初回は RSC が渡した状態を表示し、WatchState の
// SSE を受信するたびに同じ画面の状態を置き換えます。revalidate が渡された場合は、
// 状態の受信ごとに RSC で取得している画面の再取得を促します。
export function StateStreamProvider({ initialState, initialUnavailable, revalidate, children }: StateStreamProviderProps) {
  const [value, setValue] = useState<StateStreamContextValue>(() => ({
    state: dashboardStateFromProto(initialState),
    disconnected: false,
    disconnectedSince: null,
    respondedBefore: initialUnavailable !== true,
  }));

  useEffect(() => {
    const handlers = stateStreamHandlers((event) => {
      setValue((current) => reduceLiveState(current, event));
    });
    const stream = openStateStream({
      url: "/api/state",
      // ブラウザの EventSource は onmessage の引数が MessageEvent だが、この画面が
      // 使うのは data だけなので、必要な部分だけを持つ型として境界で受け取る。
      createEventSource: (url) => toEventSourceLike(new EventSource(url)),
      ...createStateHandlers({
        onState: handlers.onState,
        onError: handlers.onError,
        revalidate,
      }),
    });
    return () => stream.close();
  }, [revalidate]);

  return <StateStreamContext.Provider value={value}>{children}</StateStreamContext.Provider>;
}
// 二重の型アサーションを避けるため、EventSource をこの画面が使う EventSourceLike へ
// 明示的に変換する。必要なメンバだけを渡し、型検査を迂回しない。
function toEventSourceLike(source: EventSource): EventSourceLike {
  return {
    get onmessage() {
      return source.onmessage as EventSourceLike["onmessage"];
    },
    set onmessage(handler) {
      source.onmessage = handler as typeof source.onmessage;
    },
    get onerror() {
      return source.onerror as EventSourceLike["onerror"];
    },
    set onerror(handler) {
      source.onerror = handler as typeof source.onerror;
    },
    close: () => source.close(),
  };
}

// App Router の router.refresh を状態ストリームへ配線する。テストで Provider を
// 直接描画できるよう、useRouter の呼び出しはこのラッパーに閉じます。
export function StateStreamProviderWithRefresh({ initialState, initialUnavailable, children }: Omit<StateStreamProviderProps, "revalidate">) {
  const router = useRouter();
  const revalidate = useCallback(() => router.refresh(), [router]);
  return (
    <StateStreamProvider initialState={initialState} initialUnavailable={initialUnavailable} revalidate={revalidate}>
      {children}
    </StateStreamProvider>
  );
}

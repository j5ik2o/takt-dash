"use client";

import { createContext, useCallback, useContext, useEffect, useMemo, useState, type ReactNode } from "react";
import { useRouter } from "next/navigation";
import type { DashboardState } from "../proto/taktdash";
import { createStateHandlers, openStateStream, type EventSourceLike } from "./state-stream";

export type StateStreamContextValue = {
  state: DashboardState;
  disconnected: boolean;
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
export type LiveStateEvent = { type: "state"; state: DashboardState } | { type: "error" };

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
    onError: () => update({ type: "error" }),
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
      return { state: event.state, disconnected: false };
    case "error":
      return { state: current.state, disconnected: true };
    default:
      return current;
  }
}

type StateStreamProviderProps = {
  initialState: DashboardState;
  revalidate?: () => void;
  children: ReactNode;
};

// 画面の状態と自動更新の所有者。初回は RSC が渡した状態を表示し、WatchState の
// SSE を受信するたびに同じ画面の状態を置き換えます。revalidate が渡された場合は、
// 状態の受信ごとに RSC で取得している画面の再取得を促します。
export function StateStreamProvider({ initialState, revalidate, children }: StateStreamProviderProps) {
  const [value, setValue] = useState<StateStreamContextValue>({ state: initialState, disconnected: false });

  useEffect(() => {
    const handlers = stateStreamHandlers((event) => {
      setValue((current) => reduceLiveState(current, event));
    });
    const stream = openStateStream({
      url: "/api/state",
      // ブラウザの EventSource は onmessage の引数が MessageEvent だが、この画面が
      // 使うのは data だけなので、必要な部分だけを持つ型として境界で受け取る。
      createEventSource: (url) => new EventSource(url) as unknown as EventSourceLike,
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

// App Router の router.refresh を状態ストリームへ配線する。テストで Provider を
// 直接描画できるよう、useRouter の呼び出しはこのラッパーに閉じます。
export function StateStreamProviderWithRefresh({ initialState, children }: Omit<StateStreamProviderProps, "revalidate">) {
  const router = useRouter();
  const revalidate = useCallback(() => router.refresh(), [router]);
  return (
    <StateStreamProvider initialState={initialState} revalidate={revalidate}>
      {children}
    </StateStreamProvider>
  );
}

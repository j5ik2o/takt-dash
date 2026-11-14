"use client";

import { createContext, useContext, useEffect, useMemo, useState, type ReactNode } from "react";
import type { DashboardState } from "../proto/taktdash";
import { openStateStream, type EventSourceLike } from "./state-stream";

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

type StateStreamProviderProps = {
  initialState: DashboardState;
  children: ReactNode;
};

// 画面の状態と自動更新の所有者。初回は RSC が渡した状態を表示し、WatchState の
// SSE を受信するたびに同じ画面の状態を置き換える。
export function StateStreamProvider({ initialState, children }: StateStreamProviderProps) {
  const [state, setState] = useState(initialState);
  const [disconnected, setDisconnected] = useState(false);

  useEffect(() => {
    const stream = openStateStream({
      url: "/api/state",
      // ブラウザの EventSource は onmessage の引数が MessageEvent だが、この画面が
      // 使うのは data だけなので、必要な部分だけを持つ型として境界で受け取る。
      createEventSource: (url) => new EventSource(url) as unknown as EventSourceLike,
      onState: (next) => {
        setState(next);
        setDisconnected(false);
      },
      onError: () => {
        setDisconnected(true);
      },
    });
    return () => stream.close();
  }, []);

  const value = useMemo(() => ({ state, disconnected }), [state, disconnected]);
  return <StateStreamContext.Provider value={value}>{children}</StateStreamContext.Provider>;
}

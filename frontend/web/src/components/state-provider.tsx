"use client";

import { createContext, useCallback, useContext, useEffect, useState, type ReactNode } from "react";
import { useRouter } from "next/navigation";
import type { DashboardState as ProtoDashboardState } from "../proto/taktdash";
import type { BackendFailure } from "../grpc/client";
import { dashboardStateFromProto, type DashboardState } from "../state/restore-state";
import { createStateHandlers, openStateStream, type EventSourceLike } from "./state-stream";

export type StateStreamContextValue = {
  state: DashboardState;
  /** GetOverview または WatchState の概要を取得済みか。切断時も state とともに保持する。 */
  snapshotLoaded: boolean;
  disconnected: boolean;
  /**
   * 現在の切断の失敗理由。初期読み込みの失敗理由を引き継ぎ、状態を受けてから
   * SSE が切断した場合は unreachable になる。接続中は undefined。通知の文面は
   * この区別で変わる（issue #92、#152）。
   */
  failure?: BackendFailure;
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
      return { state: event.state, snapshotLoaded: true, disconnected: false, disconnectedSince: null, respondedBefore: true };
    case "error":
      return {
        state: current.state,
        snapshotLoaded: current.snapshotLoaded,
        disconnected: true,
        // 初期読み込みの失敗理由は、切断が続く間は引き継ぐ。状態を受けて
        // 失敗理由が消えてからの切断は、SSE の切断として unreachable にする。
        failure: current.failure ?? "unreachable",
        disconnectedSince: current.disconnectedSince ?? event.at ?? null,
        respondedBefore: current.respondedBefore,
      };
    default:
      return current;
  }
}

type StateStreamProviderProps = {
  initialState: ProtoDashboardState;
  /** 初回未取得の代替値と、正常取得した空の概要を区別する。 */
  initialSnapshotLoaded: boolean;
  /**
   * 初回の読み込みで backend を利用できなかった場合の失敗理由（今回の読み込みの
   * 成否と理由）。渡された場合は切断状態で始める。initialRespondedBefore が
   * 渡されない場合にだけ、接続履歴の初期値の代用にもする。通知の表示元は画面側の
   * 内容だけに一本化され、レイアウトは通知を描画しないため、起動時の不通は
   * ここで切断として拾い、失敗理由は通知の文面の振り分けに使う（issue #92）。
   */
  initialFailure?: BackendFailure;
  /**
   * サーバー側の接続履歴。この Web プロセスが一度でも backend の応答を受けた
   * ことがあるかどうか。今回の読み込みの成否とは別に受け取り、Context の
   * 接続履歴の初期値に使う（issue #152）。
   */
  initialRespondedBefore?: boolean;
  revalidate?: () => void;
  children: ReactNode;
};

// 画面の状態と自動更新の所有者。初回は RSC が渡した状態を表示し、WatchState の
// SSE を受信するたびに同じ画面の状態を置き換えます。revalidate が渡された場合は、
// 状態の受信ごとに RSC で取得している画面の再取得を促します。
export function StateStreamProvider({
  initialState,
  initialSnapshotLoaded,
  initialFailure,
  initialRespondedBefore,
  revalidate,
  children,
}: StateStreamProviderProps) {
  const [value, setValue] = useState<StateStreamContextValue>(() => ({
    state: dashboardStateFromProto(initialState),
    snapshotLoaded: initialSnapshotLoaded,
    // 初期読み込みが失敗した場合は、その失敗理由を持って切断状態で始める。
    // レイアウトは通知を描画せず、概要と判断待ちの内容が disconnected で通知を
    // 描画するため、起動時の不通でも通知は画面側の1回だけになる（issue #92）。
    disconnected: initialFailure !== undefined,
    failure: initialFailure,
    disconnectedSince: null,
    // 接続履歴の初期値はサーバー側の履歴を正とする。今回の読み込みが失敗して
    // キャッシュで描画した場合でも、過去の応答があれば履歴は消えない。
    respondedBefore: initialRespondedBefore ?? initialFailure === undefined,
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
export function StateStreamProviderWithRefresh({
  initialState,
  initialSnapshotLoaded,
  initialFailure,
  initialRespondedBefore,
  children,
}: Omit<StateStreamProviderProps, "revalidate">) {
  const router = useRouter();
  const revalidate = useCallback(() => router.refresh(), [router]);
  return (
    <StateStreamProvider
      initialState={initialState}
      initialSnapshotLoaded={initialSnapshotLoaded}
      initialFailure={initialFailure}
      initialRespondedBefore={initialRespondedBefore}
      revalidate={revalidate}
    >
      {children}
    </StateStreamProvider>
  );
}

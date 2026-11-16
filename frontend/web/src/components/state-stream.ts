import { restoreDashboardState, type DashboardState } from "../state/restore-state";

/** EventSource のうち、この画面が使う部分だけを表す。テストでは差し替える。 */
export type EventSourceLike = {
  onmessage: ((event: { data: string }) => void) | null;
  onerror: ((event: unknown) => void) | null;
  close(): void;
};

export type OpenStateStreamOptions = {
  url: string;
  createEventSource: (url: string) => EventSourceLike;
  onState: (state: DashboardState) => void;
  onError?: () => void;
  /**
   * 再接続の予約。`run` を `delayMs` 後に実行し、取消関数を返す。テストでは決定的な
   * スケジューラへ差し替える。
   */
  scheduleReconnect?: (run: () => void, delayMs: number) => () => void;
};

export type StateStream = {
  close: () => void;
};

export type StateStreamHandlerOptions = {
  onState: (state: DashboardState) => void;
  onError?: () => void;
  revalidate?: () => void;
};

export type StateStreamHandlers = {
  onState: (state: DashboardState) => void;
  onError?: () => void;
};

// 状態の受信と画面の再取得を一か所で組み立てる。状態を画面へ適用した直後に
// revalidate を呼び、RSC で取得している画面（プロジェクト・アクティビティ・
// run 詳細・サイドバー）をサーバー側の最新状態へ追従させます。切断時は状態を
// 置き換えないため、revalidate を呼びません。
export function createStateHandlers({
  onState,
  onError,
  revalidate,
}: StateStreamHandlerOptions): StateStreamHandlers {
  return {
    onState: (state) => {
      onState(state);
      revalidate?.();
    },
    onError,
  };
}

/** 最初の再接続を待つ時間。連続失敗のたびに倍にする。 */
const RECONNECT_BASE_DELAY_MS = 1000;
/** 再接続の待機の上限。失敗が続いてもこの時間より長く待たない。 */
const RECONNECT_MAX_DELAY_MS = 30_000;

const scheduleWithTimeout = (run: () => void, delayMs: number): (() => void) => {
  const timer = setTimeout(run, delayMs);
  return () => clearTimeout(timer);
};

export function openStateStream(options: OpenStateStreamOptions): StateStream {
  const scheduleReconnect = options.scheduleReconnect ?? scheduleWithTimeout;
  let closed = false;
  let source: EventSourceLike | undefined;
  // 連続失敗の回数。受信に成功したら 0 に戻し、次の失敗を再び 1 秒待ちから始める。
  let consecutiveFailures = 0;
  let cancelScheduledReconnect: (() => void) | undefined;

  const connect = () => {
    const current = options.createEventSource(options.url);
    current.onmessage = (event) => {
      // 送信側は DashboardState.toJSON の JSON（時刻は ISO 文字列）を送る。受信の境界で
      // 内部の型へ復元し、Date 型の各フィールドを正規化する。
      consecutiveFailures = 0;
      options.onState(restoreDashboardState(JSON.parse(event.data)));
    };
    current.onerror = () => {
      if (closed) {
        return;
      }
      current.close();
      options.onError?.();
      const delayMs = Math.min(RECONNECT_BASE_DELAY_MS * 2 ** consecutiveFailures, RECONNECT_MAX_DELAY_MS);
      consecutiveFailures += 1;
      cancelScheduledReconnect = scheduleReconnect(() => {
        cancelScheduledReconnect = undefined;
        if (!closed) {
          connect();
        }
      }, delayMs);
    };
    source = current;
  };

  connect();

  return {
    close: () => {
      closed = true;
      cancelScheduledReconnect?.();
      cancelScheduledReconnect = undefined;
      source?.close();
    },
  };
}

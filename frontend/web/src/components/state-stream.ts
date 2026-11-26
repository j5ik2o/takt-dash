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
  /**
   * 無音の見張り時間（ミリ秒）。最後の有効な受信（または接続の開始）から壁時計でこの時間
   * 何も届かなければ、onerror が来なくても切断として扱い、接続を開き直す（issue #383）。
   */
  staleTimeoutMs?: number;
  /** 見張りタイマーの予約。scheduleReconnect と同じ形で、テストでは別に差し替える。 */
  scheduleWatchdog?: (run: () => void, delayMs: number) => () => void;
  /** 壁時計。見張りの経過時間の判定に使う。 */
  now?: () => number;
};

export type StateStream = {
  close: () => void;
  /**
   * 最後の受信からの経過を壁時計で確かめ、見張り時間を過ぎていれば切断して開き直す。
   * スリープや裏のタブからの復帰で、タイマーが遅れている間を埋めるために呼ぶ。
   */
  checkStale: () => void;
};

/**
 * 無音の見張り時間。backend は状態が変わらなくても 15 秒ごとに送り直す
 * （WATCH_RESEND_INTERVAL）ため、受信の間隔は「15 秒＋状態の組み立て・送信の時間」に
 * 収まる。40 秒は組み立てや遅延が 25 秒遅れても誤検知せず、長時間切断の文面に切り替わる
 * 60 秒より前に開き直しへ入れる長さ（docs/design/issue-383.md）。
 */
export const STATE_STREAM_STALE_TIMEOUT_MS = 40_000;
/** setTimeout が受け付ける遅延の上限。これを超えると 1ms 扱いになり、再接続が回り続ける。 */
const MAX_TIMER_DELAY_MS = 2_147_483_647;

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

type Schedule = (run: () => void, delayMs: number) => () => void;

/**
 * 予約を常に 1 本だけ持つ枠。新しい予約の前に前の予約を解除する。発火済み・取消済みの
 * 予約は走らせず、スケジューラがその場で実行しても、終わった予約で次の予約を上書きしない。
 */
const createTimerSlot = (schedule: Schedule) => {
  let cancelCurrent: (() => void) | undefined;
  const clear = () => {
    cancelCurrent?.();
    cancelCurrent = undefined;
  };
  const set = (run: () => void, delayMs: number) => {
    clear();
    let live = true;
    const cancel = schedule(() => {
      if (!live) {
        return;
      }
      live = false;
      cancelCurrent = undefined;
      run();
    }, delayMs);
    if (live) {
      cancelCurrent = () => {
        live = false;
        cancel();
      };
    }
  };
  return { set, clear };
};

const assertStaleTimeout = (staleTimeoutMs: number): void => {
  if (!Number.isFinite(staleTimeoutMs) || staleTimeoutMs <= 0 || staleTimeoutMs > MAX_TIMER_DELAY_MS) {
    throw new RangeError(`staleTimeoutMs は 1 以上 ${MAX_TIMER_DELAY_MS} 以下の有限の数です: ${staleTimeoutMs}`);
  }
};

type Freshness = { stale: true } | { stale: false; lastActivityAt: number; rearmDelayMs: number };

/**
 * 最後の受信からの経過を壁時計で判定する。タイマーは早く発火することも、スリープで遅れる
 * こともあるため、期限前なら残りの時間で張り直す。壁時計が戻っていたら誤って切らず、
 * 基準を今へ詰めて見張り時間の全体で張り直す。
 */
const judgeFreshness = (nowMs: number, lastActivityAt: number, staleTimeoutMs: number): Freshness => {
  if (nowMs < lastActivityAt) {
    return { stale: false, lastActivityAt: nowMs, rearmDelayMs: staleTimeoutMs };
  }
  const elapsedMs = nowMs - lastActivityAt;
  if (elapsedMs < staleTimeoutMs) {
    return { stale: false, lastActivityAt, rearmDelayMs: staleTimeoutMs - elapsedMs };
  }
  return { stale: true };
};

// close の中で onerror が再入しないよう、ハンドラを外してから閉じる。
const detachSource = (target: EventSourceLike): void => {
  target.onmessage = null;
  target.onerror = null;
  target.close();
};

export function openStateStream(options: OpenStateStreamOptions): StateStream {
  const staleTimeoutMs = options.staleTimeoutMs ?? STATE_STREAM_STALE_TIMEOUT_MS;
  assertStaleTimeout(staleTimeoutMs);
  const now = options.now ?? (() => Date.now());
  const reconnect = createTimerSlot(options.scheduleReconnect ?? scheduleWithTimeout);
  const watchdog = createTimerSlot(options.scheduleWatchdog ?? scheduleWithTimeout);
  let closed = false;
  // 現在の接続。切断・close で undefined にし、古い接続のイベントはこれと比べて捨てる。
  let source: EventSourceLike | undefined;
  // 連続失敗の回数。受信に成功したら 0 に戻し、次の失敗を再び 1 秒待ちから始める。
  let consecutiveFailures = 0;
  let lastActivityAt = 0; // 最後の有効な受信（または接続の開始）の壁時計
  const isCurrent = (target: EventSourceLike) => !closed && source === target;

  const disconnect = (target: EventSourceLike) => {
    if (!isCurrent(target)) {
      return;
    }
    watchdog.clear();
    source = undefined;
    detachSource(target);
    options.onError?.();
    if (closed) {
      return;
    }
    const delayMs = Math.min(RECONNECT_BASE_DELAY_MS * 2 ** consecutiveFailures, RECONNECT_MAX_DELAY_MS);
    consecutiveFailures += 1;
    // close は予約を取り消し、取り消した予約は発火しても走らない（createTimerSlot）。
    reconnect.set(connect, delayMs);
  };

  // 見張りの発火と checkStale で共通の判定。
  const evaluateFreshness = (target: EventSourceLike) => {
    if (!isCurrent(target)) {
      return;
    }
    const judged = judgeFreshness(now(), lastActivityAt, staleTimeoutMs);
    if (judged.stale) {
      disconnect(target);
    } else {
      lastActivityAt = judged.lastActivityAt;
      watchdog.set(() => evaluateFreshness(target), judged.rearmDelayMs);
    }
  };

  const markActive = (target: EventSourceLike) => {
    lastActivityAt = now();
    watchdog.set(() => evaluateFreshness(target), staleTimeoutMs);
  };

  const connect = () => {
    const current = options.createEventSource(options.url);
    source = current;
    current.onmessage = (event) => {
      if (!isCurrent(current)) {
        return;
      }
      // DashboardState.toJSON の JSON を受信の境界で内部の型へ復元する。検証に失敗したら
      // 例外のまま抜け、見張りも連続失敗の回数も更新しない。
      const state = restoreDashboardState(JSON.parse(event.data));
      consecutiveFailures = 0;
      // onState の中で close されてもタイマーが残らないよう、見張りを先に張り直す。
      markActive(current);
      options.onState(state);
    };
    current.onerror = () => disconnect(current);
    // 1 件も届かないまま黙る接続も検知できるよう、開いた時点から見張る。
    markActive(current);
  };

  connect();

  return {
    checkStale: () => {
      if (source !== undefined) {
        evaluateFreshness(source);
      }
    },
    close: () => {
      closed = true;
      watchdog.clear();
      reconnect.clear();
      if (source !== undefined) detachSource(source);
      source = undefined;
    },
  };
}

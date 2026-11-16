// 要素単位の自動更新の間隔制御。
//
// 詳細画面はページ全体を再読み込みせず、一定間隔で取得した値だけを差し替える。
// 前回の取得が終わるまで次の取得を始めず、unmount（abort）後は取得しない。
export type StartPollingOptions<T> = {
  intervalMs: number;
  signal: AbortSignal;
  load: () => Promise<T>;
  onValue: (value: T) => void;
  onError: (error: unknown) => void;
};

export type PollingHandle = {
  stop: () => void;
};

export function startPolling<T>(options: StartPollingOptions<T>): PollingHandle {
  let stopped = false;
  let timer: ReturnType<typeof setTimeout> | undefined;

  const clearTimer = () => {
    if (timer !== undefined) {
      clearTimeout(timer);
      timer = undefined;
    }
  };

  const stop = () => {
    if (stopped) {
      return;
    }
    stopped = true;
    clearTimer();
    options.signal.removeEventListener("abort", stop);
  };

  const scheduleNext = () => {
    if (stopped) {
      return;
    }
    timer = setTimeout(() => {
      void tick();
    }, options.intervalMs);
  };

  const tick = async () => {
    timer = undefined;
    if (stopped) {
      return;
    }
    try {
      const value = await options.load();
      if (!stopped) {
        options.onValue(value);
      }
    } catch (error) {
      if (!stopped) {
        options.onError(error);
      }
    }
    // 取得の完了後にだけ次を予約するため、未完了の取得へ次の要求を重ねない。
    scheduleNext();
  };

  if (options.signal.aborted) {
    return { stop };
  }
  options.signal.addEventListener("abort", stop, { once: true });
  void tick();

  return { stop };
}

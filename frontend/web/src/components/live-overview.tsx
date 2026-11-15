"use client";

import { useEffect, useRef, useState, type ReactElement } from "react";
import { buildOverviewView } from "../state/dashboard";
import { FLASH_MS, initialLogSeen, nextLogSeen, type LogSeen } from "../state/run-log-flash";
import { useLiveState } from "./state-provider";
import { OverviewContent } from "./overview-content";

type LiveOverviewProps = {
  initialNowMs: number;
};

// 自動更新の状態を概要の表示用の形へ変換する。時刻は1秒ごとに進め、新しいログが届いた
// run 行だけを1秒間光らせる。タイマーはこのコンポーネントが所有し、unmount 時に破棄する。
export function LiveOverview({ initialNowMs }: LiveOverviewProps): ReactElement {
  const { state } = useLiveState();
  const [now, setNow] = useState(() => state.readAt ?? new Date(initialNowMs));
  const seenRef = useRef<LogSeen | null>(null);
  if (seenRef.current === null) {
    seenRef.current = initialLogSeen(state.runs);
  }
  const [flashing, setFlashing] = useState<ReadonlySet<string>>(() => new Set());
  const timers = useRef(new Map<string, ReturnType<typeof setTimeout>>());

  useEffect(() => {
    const id = setInterval(() => setNow(new Date()), 1_000);
    return () => clearInterval(id);
  }, []);

  useEffect(() => {
    const { seen, flashed } = nextLogSeen(seenRef.current ?? {}, state.runs);
    seenRef.current = seen;
    if (flashed.length === 0) {
      return;
    }
    setFlashing((prev) => {
      const next = new Set(prev);
      for (const slug of flashed) {
        next.add(slug);
      }
      return next;
    });
    for (const slug of flashed) {
      const existing = timers.current.get(slug);
      if (existing !== undefined) {
        clearTimeout(existing);
      }
      timers.current.set(
        slug,
        setTimeout(() => {
          timers.current.delete(slug);
          setFlashing((prev) => {
            const next = new Set(prev);
            next.delete(slug);
            return next;
          });
        }, FLASH_MS),
      );
    }
  }, [state.runs]);

  useEffect(
    () => () => {
      for (const timer of timers.current.values()) {
        clearTimeout(timer);
      }
      timers.current.clear();
    },
    [],
  );

  return <OverviewContent view={buildOverviewView(state, now)} flashingRunSlugs={flashing} />;
}

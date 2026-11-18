// run 詳細のログの追記分を取りに行くポーリングの制御。
//
// 開いている run 詳細の分だけ、一定間隔で前回の続きの位置を送って追記分を取り、画面の状態へ
// 適用する。間隔と停止（前回の取得が終わるまで次を始めない、abort 後は取得も適用もしない）は
// startPolling に従う。ログが残っていない run は取得を始めず、取得の途中でログが残っていないと
// 分かったら、その応答を適用して止まる。取得に失敗した間は直前の状態を保ち、次の成功で続きから
// 再開する。
import { applyRunUpdates, type RunLogCursor, type RunLogView, type RunUpdates } from "../state/run-log-stream";
import { startPolling, type PollingHandle } from "./polling";

export type StartRunLogUpdatesOptions = {
  signal: AbortSignal;
  intervalMs: number;
  initialView: RunLogView;
  fetchUpdates: (cursor: RunLogCursor | undefined) => Promise<RunUpdates>;
  onView: (view: RunLogView) => void;
  onError: (error: unknown) => void;
};

export function startRunLogUpdates(options: StartRunLogUpdatesOptions): PollingHandle {
  if (options.initialView.logsRemoved) {
    return { stop: () => {} };
  }
  let view = options.initialView;
  const polling: PollingHandle = startPolling({
    intervalMs: options.intervalMs,
    signal: options.signal,
    load: () => options.fetchUpdates(view.cursor),
    onValue: (updates) => {
      view = applyRunUpdates(view, updates);
      options.onView(view);
      if (view.logsRemoved) {
        polling.stop();
      }
    },
    onError: options.onError,
  });
  return polling;
}

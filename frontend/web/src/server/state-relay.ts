import type { DashboardState } from "../proto/taktdash";
import type { Admission } from "./screen-streams";

// backend の WatchState を、画面へ送る ReadableStream に中継する（issue #385）。
// 仕様は specs/quint/state-stream-relay/stateStreamRelay.qnt。
//
// - 後始末は finish の 1 本。abort、gRPC の error・end、ReadableStream の cancel、同じ画面の
//   新しい接続による付け替えのどれから来ても、2 回目以降は何もしない。
// - Next が読めない間（desiredSize <= 0）は列へ積まず、最新の状態 1 件だけを持つ。状態は毎回
//   すべてを送るので、途中の版を飛ばしても画面は最新になる。

/** gRPC の ClientReadableStream のうち、中継が使う部分。テストでは差し替える。 */
export type WatchStream = {
  on(event: "data", listener: (state: DashboardState) => void): unknown;
  on(event: "error", listener: (error: unknown) => void): unknown;
  on(event: "end", listener: () => void): unknown;
  cancel(): void;
  destroy(): void;
};

export type RelayStateStreamOptions = {
  openStream: () => WatchStream;
  signal: AbortSignal;
  /**
   * 画面の登録簿へ登録する。断られたら undefined を返す。省略したとき（識別子の無い接続）は
   * 登録しない。`close` は同じ画面の新しい接続が届いたときに呼ばれる。
   */
  admit?: (close: () => void) => Admission | undefined;
  encode: (state: DashboardState) => Uint8Array;
  /** backend から状態を受けるたびに呼ぶ。 */
  onState?: () => void;
};

/** ReadableStream の列の上限。不変条件 queueBounded はこの値を前提にする。 */
export const RELAY_HIGH_WATER_MARK = 1;

/** 中継の ReadableStream を作る。登録を断られたら undefined を返し、gRPC を開かない。 */
export function relayStateStream(options: RelayStateStreamOptions): ReadableStream<Uint8Array> | undefined {
  const { signal } = options;
  let controller: ReadableStreamDefaultController<Uint8Array> | undefined;
  let stream: WatchStream | undefined;
  let finished = false;
  let pending: DashboardState | undefined;
  let admission: Admission | undefined;

  const finish = ({ cancelStream }: { cancelStream: boolean }) => {
    if (finished) {
      return;
    }
    finished = true;
    pending = undefined;
    admission?.release();
    signal.removeEventListener("abort", onAbort);
    if (cancelStream) {
      stream?.cancel();
      stream?.destroy();
    }
    try {
      controller?.close();
    } catch {
      // cancel の中や既に閉じている場合は何もしない。
    }
  };
  const onAbort = () => finish({ cancelStream: true });

  const enqueue = (state: DashboardState) => {
    try {
      controller?.enqueue(options.encode(state));
    } catch {
      onAbort();
    }
  };

  if (options.admit !== undefined) {
    admission = options.admit(onAbort);
    if (admission === undefined) {
      return undefined;
    }
  }

  return new ReadableStream<Uint8Array>(
    {
      start(started) {
        controller = started;
        if (finished || signal.aborted) {
          onAbort();
          return;
        }
        signal.addEventListener("abort", onAbort);
        const opened = options.openStream();
        stream = opened;
        opened.on("data", (state: DashboardState) => {
          options.onState?.();
          if (finished) {
            return;
          }
          if ((started.desiredSize ?? 0) > 0) {
            enqueue(state);
          } else {
            pending = state;
          }
        });
        opened.on("error", onAbort);
        // backend が終えたストリームは閉じ直さない。
        opened.on("end", () => finish({ cancelStream: false }));
      },
      pull() {
        if (finished || pending === undefined) {
          return;
        }
        const latest = pending;
        pending = undefined;
        enqueue(latest);
      },
      cancel() {
        onAbort();
      },
    },
    new CountQueuingStrategy({ highWaterMark: RELAY_HIGH_WATER_MARK }),
  );
}

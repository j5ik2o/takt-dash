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

/**
 * 1 本の接続の中継。後始末（finish）と間引き（offer・pull）を持つ。ReadableStream の
 * start・pull・cancel と、gRPC の data・error・end と、登録簿の付け替えから呼ばれる。
 */
class StateRelay {
  private controller: ReadableStreamDefaultController<Uint8Array> | undefined;
  private stream: WatchStream | undefined;
  private finished = false;
  private pending: DashboardState | undefined;
  admission: Admission | undefined;

  constructor(private readonly options: RelayStateStreamOptions) {}

  readonly abort = (): void => this.finish({ cancelStream: true });

  start(controller: ReadableStreamDefaultController<Uint8Array>): void {
    this.controller = controller;
    const { signal } = this.options;
    if (this.finished || signal.aborted) {
      this.abort();
      return;
    }
    signal.addEventListener("abort", this.abort);
    let opened: WatchStream;
    try {
      opened = this.options.openStream();
    } catch (error) {
      // 登録と abort の聞き手を残さずに、例外を呼び出し元へ返す。
      this.abort();
      throw error;
    }
    this.stream = opened;
    opened.on("data", (state: DashboardState) => this.offer(state));
    opened.on("error", this.abort);
    // backend が終えたストリームは閉じ直さない。
    opened.on("end", () => this.finish({ cancelStream: false }));
  }

  pull(): void {
    if (this.finished || this.pending === undefined) {
      return;
    }
    const latest = this.pending;
    this.pending = undefined;
    this.enqueue(latest);
  }

  private offer(state: DashboardState): void {
    this.options.onState?.();
    if (this.finished) {
      return;
    }
    if ((this.controller?.desiredSize ?? 0) > 0) {
      // 持ち分はこの状態より古い。列へ入れたあとに pull が古い版を送らないよう捨てる。
      this.pending = undefined;
      this.enqueue(state);
    } else {
      this.pending = state;
    }
  }

  private enqueue(state: DashboardState): void {
    try {
      this.controller?.enqueue(this.options.encode(state));
    } catch {
      this.abort();
    }
  }

  private finish({ cancelStream }: { cancelStream: boolean }): void {
    if (this.finished) {
      return;
    }
    this.finished = true;
    this.pending = undefined;
    this.admission?.release();
    this.options.signal.removeEventListener("abort", this.abort);
    if (cancelStream) {
      this.stream?.cancel();
      this.stream?.destroy();
    }
    try {
      this.controller?.close();
    } catch {
      // cancel の中や既に閉じている場合は何もしない。
    }
  }
}

/** 中継の ReadableStream を作る。登録を断られたら undefined を返し、gRPC を開かない。 */
export function relayStateStream(options: RelayStateStreamOptions): ReadableStream<Uint8Array> | undefined {
  const relay = new StateRelay(options);
  if (options.admit !== undefined) {
    relay.admission = options.admit(relay.abort);
    if (relay.admission === undefined) {
      return undefined;
    }
  }
  return new ReadableStream<Uint8Array>(
    {
      start: (controller) => relay.start(controller),
      pull: () => relay.pull(),
      cancel: () => relay.abort(),
    },
    new CountQueuingStrategy({ highWaterMark: RELAY_HIGH_WATER_MARK }),
  );
}

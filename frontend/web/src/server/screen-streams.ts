import type { StreamTicket } from "../state/stream-ticket";

// 画面ごとに、いまサーバーで開いている状態の配信の接続を 1 本だけ覚える登録簿（issue #385）。
// 仕様は specs/quint/state-stream-relay/stateStreamRelay.qnt の registry。
//
// 登録簿は Node のプロセスの中にだけある。配信を複数のプロセスに分けると、別のプロセスに
// 届いた古い接続は閉じられない（docs/design/issue-385.md）。

export type Admission = {
  /** 登録がまだこの接続を指しているときだけ外す。 */
  release: () => void;
};

export type ScreenStreams = {
  /**
   * 接続を登録する。同じ画面のもっと大きい番号が開いていれば断って undefined を返す。
   * それ以外は登録を付け替えてから、同じ画面の古い接続の close を呼ぶ。
   */
  admit: (ticket: StreamTicket, close: () => void) => Admission | undefined;
  /** 登録している画面の数。テストで、閉じた接続の登録が残らないことを確かめる。 */
  size: () => number;
};

type Entry = { attempt: number; close: () => void };

export function createScreenStreams(): ScreenStreams {
  const entries = new Map<string, Entry>();
  return {
    admit: (ticket, close) => {
      const current = entries.get(ticket.screen);
      if (current !== undefined && current.attempt > ticket.attempt) {
        return undefined;
      }
      const entry: Entry = { attempt: ticket.attempt, close };
      entries.set(ticket.screen, entry);
      // 先に付け替えるので、古い接続の後始末の release は新しい登録を消さない。
      current?.close();
      return {
        release: () => {
          if (entries.get(ticket.screen) === entry) {
            entries.delete(ticket.screen);
          }
        },
      };
    },
    size: () => entries.size,
  };
}

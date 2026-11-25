// 最後に読めたページの内容を、要求をまたいでサーバー側に残す（issue #309）。
// backend に接続できないとき、各ページは前に読めた内容をバナーの下に残して描画する。
// 概要の状態を残す server-state.ts と同じ考え方を、キーごとの内容へ広げたもの。
// run やゴールは増え続けるため、最近使ったものから上限件数だけを残す。

export const LAST_LOADED_LIMIT = 100;

export type LastLoaded<T> = {
  remember: (key: string, value: T) => void;
  recall: (key: string) => T | undefined;
};

export function createLastLoaded<T>(limit = LAST_LOADED_LIMIT): LastLoaded<T> {
  const entries = new Map<string, T>();
  return {
    remember: (key, value) => {
      entries.delete(key);
      entries.set(key, value);
      while (entries.size > limit) {
        const oldest = entries.keys().next().value;
        if (oldest === undefined) {
          break;
        }
        entries.delete(oldest);
      }
    },
    recall: (key) => {
      const value = entries.get(key);
      if (value !== undefined) {
        entries.delete(key);
        entries.set(key, value);
      }
      return value;
    },
  };
}

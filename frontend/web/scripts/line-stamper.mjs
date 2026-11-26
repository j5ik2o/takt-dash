// Web のログの各行に時刻を付ける純粋な部分（issue #186、docs/design/issue-186.md）。
//
// 流れ 1 本ぶんの状態（いまが行の頭か）を持ち、受け取ったバイト列の行の頭に
// 「UTC の RFC 3339（ミリ秒まで）と空白 1 つ」を差し込んで返す。
// 文字の符号は解釈せず、行を溜めずに受け取ったぶんをすぐに返す。

/** @param {number} epochMs @returns {string} */
export function formatStamp(epochMs) {
  return new Date(epochMs).toISOString();
}

/**
 * @param {() => number} [now] 既定は Date.now
 * @returns {(chunk: Uint8Array) => Buffer}
 */
export function createLineStamper(now = Date.now) {
  let atLineStart = true;
  return (chunk) => {
    if (chunk.length === 0) {
      return Buffer.alloc(0);
    }
    const input = Buffer.isBuffer(chunk)
      ? chunk
      : Buffer.from(chunk.buffer, chunk.byteOffset, chunk.byteLength);
    const parts = [];
    let start = 0;
    while (start < input.length) {
      if (atLineStart) {
        parts.push(Buffer.from(`${formatStamp(now())} `, "utf8"));
        atLineStart = false;
      }
      const newline = input.indexOf(0x0a, start);
      if (newline === -1) {
        parts.push(input.subarray(start));
        start = input.length;
      } else {
        parts.push(input.subarray(start, newline + 1));
        start = newline + 1;
        atLineStart = true;
      }
    }
    return Buffer.concat(parts);
  };
}

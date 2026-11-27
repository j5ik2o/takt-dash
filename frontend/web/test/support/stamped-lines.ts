// 時刻を付けたログ（issue #186）を行の頭から読み、時刻と中身に分ける。
import assert from "node:assert/strict";

export const STAMP_RE = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/;
export const STAMP_LEN = 24;

// 各行の頭の「時刻と空白 1 つ」を外し、外した時刻と、残りのバイト列を返す。
// 行が時刻で始まらない、時刻のあとが空白でない、時刻だけで中身が無い行は、失敗にする。
export const unstamp = (output: Buffer, where: string): { body: Buffer; stamps: string[] } => {
  const body: Buffer[] = [];
  const stamps: string[] = [];
  let at = 0;
  while (at < output.length) {
    const stamp = output.subarray(at, at + STAMP_LEN).toString("latin1");
    assert.match(stamp, STAMP_RE, `${where}: 出力の ${at} バイト目の行が時刻で始まる`);
    assert.equal(output[at + STAMP_LEN], 0x20, `${where}: 時刻のあとは空白 1 つ`);
    stamps.push(stamp);
    at += STAMP_LEN + 1;
    const newline = output.indexOf(0x0a, at);
    const end = newline === -1 ? output.length : newline + 1;
    assert.ok(end > at, `${where}: 時刻だけで中身の無い行を作らない`);
    body.push(output.subarray(at, end));
    at = end;
  }
  return { body: Buffer.concat(body), stamps };
};

import { expect, test } from "../support/test";

// issue #385: 画面が開き直した接続（同じ画面の識別子で、番号の大きい試行）がサーバーに届いたら、
// サーバーは同じ画面の古い状態の配信を閉じる。経路が切れていて古い接続の FIN が届かなくても、
// OS が TCP をあきらめるのを待たない。遅れて届いた古い試行は、backend を購読せずに 204 で断る。
const SCREEN = "385385385385385385385385385385ab";

const readUntilDone = async (reader: ReadableStreamDefaultReader<Uint8Array>, timeoutMs: number): Promise<boolean> => {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const result = await Promise.race([
      reader.read(),
      new Promise<"timeout">((resolve) => setTimeout(() => resolve("timeout"), deadline - Date.now())),
    ]);
    if (result === "timeout") return false;
    if (result.done) return true;
  }
  return false;
};

test("同じ画面の新しい試行が届くと古い配信を閉じ、遅れて届いた古い試行は 204 で断る", async ({ isolatedStack: stack }) => {
  const url = (attempt: number) => `${stack.webUrl}/api/state?screen=${SCREEN}&attempt=${attempt}`;

  const older = await fetch(url(1));
  expect(older.status).toBe(200);
  const olderReader = older.body!.getReader();
  expect((await olderReader.read()).done).toBe(false);

  const newer = await fetch(url(2));
  expect(newer.status).toBe(200);
  const newerReader = newer.body!.getReader();
  expect((await newerReader.read()).done).toBe(false);

  expect(await readUntilDone(olderReader, 10_000)).toBe(true);

  const stale = await fetch(url(1));
  expect(stale.status).toBe(204);

  await newerReader.cancel();
});

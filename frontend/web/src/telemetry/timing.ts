// ページ応答と gRPC 往復の所要時間を1行のログに分解する計測基盤（AC72A-1）。
//
// ページ要求ごとに、応答全体（total_ms）を gRPC の往復（rpc_ms）と Next.js 側の
// 差分（next_ms）へ分ける。実際の時刻と出力先は注入でき、テストで固定できる。

/** 1回の RPC 呼び出しの計測値。 */
export type RpcTiming = {
  rpc: string;
  durationMs: number;
};

/** RPC の計測値を受け取る先。gRPC クライアントはこの型だけに依存する。 */
export type TimingSink = {
  recordRpc(timing: RpcTiming): void;
};

/** 計測の1行を書き出す関数。 */
export type TimingLogger = (line: string) => void;

/** RPC の集計値と出力をまとめた計測器。 */
export type TimingRecorder = TimingSink & {
  readonly durationMs: number;
  readonly rpcs: readonly string[];
};

export type CreateTimingRecorderOptions = {
  log?: TimingLogger;
};

export type MeasurePageOptions = {
  clock?: () => number;
  log?: TimingLogger;
};

const TIMING_LINE_PREFIX = "[taktDash][timing] ";

// 監視用の1行を標準出力へ出す。行が持つ接頭辞は二重にせず、ここで1回だけ付ける。
const defaultLog: TimingLogger = (line) => console.info(`[taktDash][timing] ${line.replace(TIMING_LINE_PREFIX, "")}`);

const defaultClock = (): number => performance.now();

const formatMs = (value: number): string => value.toFixed(2);

/** RPC の所要時間を1回ごとに記録し、合計と名前の並びを保持する計測器を作る。 */
export function createTimingRecorder(options: CreateTimingRecorderOptions = {}): TimingRecorder {
  const log = options.log ?? defaultLog;
  const rpcs: string[] = [];
  let durationMs = 0;
  return {
    recordRpc({ rpc, durationMs: elapsedMs }) {
      log(`[taktDash][timing] scope=rpc rpc=${rpc} duration_ms=${formatMs(elapsedMs)}`);
      rpcs.push(rpc);
      durationMs += elapsedMs;
    },
    get durationMs() {
      return durationMs;
    },
    get rpcs() {
      return rpcs;
    },
  };
}

/**
 * `run` を1回のページ要求として計測する。
 *
 * `run` には RPC の計測値を受け取る `TimingSink` を渡す。成功・失敗にかかわらず
 * `finally` で total/rpc/next の1行を出し、`run` の結果はそのまま返す（失敗は再送出する）。
 */
export async function measurePage<Result>(
  route: string,
  run: (timing: TimingSink) => Promise<Result>,
  options: MeasurePageOptions = {},
): Promise<Result> {
  const clock = options.clock ?? defaultClock;
  const log = options.log ?? defaultLog;
  const recorder = createTimingRecorder({ log });
  const startedAt = clock();
  try {
    return await run(recorder);
  } finally {
    const totalMs = clock() - startedAt;
    const rpcMs = recorder.durationMs;
    const nextMs = Math.max(0, totalMs - rpcMs);
    log(
      `[taktDash][timing] scope=page page=${route} total_ms=${formatMs(totalMs)} rpc_ms=${formatMs(rpcMs)} next_ms=${formatMs(nextMs)} rpcs=${recorder.rpcs.join(",")}`,
    );
  }
}

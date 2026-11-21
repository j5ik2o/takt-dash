/**
 * backend を利用できないことを表す。
 *
 * transport は backend へ接続できない状態、server は backend が応答の中で
 * UNAVAILABLE を返した状態を指す。server の場合は backend が返した details を保つ。
 */
export type BackendUnavailableKind = "transport" | "server";

/**
 * backend を利用できない理由。unreachable は接続自体が成立しない状態、
 * no-response は期限内に応答が返らない状態を指す。表示はこの区別で文面を
 * 変える（issue #152）。
 */
export type BackendFailure = "unreachable" | "no-response";

export class BackendUnavailableError extends Error {
  readonly kind: BackendUnavailableKind;
  readonly failure: BackendFailure;
  readonly details: string;

  constructor(
    cause: unknown,
    kind: BackendUnavailableKind = "transport",
    details = "",
    failure: BackendFailure = "unreachable",
  ) {
    super(failure === "no-response" ? "backend が応答しません。" : "backend に接続できません。");
    this.name = "BackendUnavailableError";
    this.cause = cause;
    this.kind = kind;
    this.failure = failure;
    this.details = details;
  }
}

// この Web プロセスが一度でも backend の応答を受けたかどうか。設定の確認を
// 促す文面は、応答を受けたことがある場合には的外れになるため、表示の判断に使う。
let backendResponded = false;

/** backend の応答を受けたことを記録する。unary の成功と状態配信の受信で呼ぶ。 */
export function markBackendResponded(): void {
  backendResponded = true;
}

/** この Web プロセスが一度でも backend の応答を受けたかどうかを返す。 */
export function hasBackendResponded(): boolean {
  return backendResponded;
}

/** テスト用に接続履歴を消す。本番の経路では呼ばない。 */
export function resetBackendResponded(): void {
  backendResponded = false;
}

/**
 * backend 不通の表示に使う状況。失敗の種類と、この Web プロセスが一度でも
 * 応答を受けたかどうかから組み立てる。接続できない状態が続いている時間を
 * 測れる呼び出し元（ブラウザ側の切断表示）は durationMs を渡す。
 */
export type BackendUnavailableSituation = {
  failure: BackendFailure;
  respondedBefore: boolean;
  durationMs?: number;
};

export function backendUnavailableSituation(
  error: BackendUnavailableError,
  durationMs?: number,
): BackendUnavailableSituation {
  return {
    failure: error.failure,
    respondedBefore: hasBackendResponded(),
    ...(durationMs !== undefined ? { durationMs } : {}),
  };
}

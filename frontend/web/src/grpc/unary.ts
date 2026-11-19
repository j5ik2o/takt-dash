// backend の unary RPC を呼ぶ共通処理。接続できない失敗の分類、応答の記録、計測、
// 1件を引く RPC の「見つからない」の判定を受け持ち、操作ごとの組み立ては client.ts が持つ。
import * as grpc from "@grpc/grpc-js";
import type { TimingSink } from "../telemetry/timing";
import { BackendUnavailableError, markBackendResponded } from "./backend-availability";

const isConnectionFailure = (error: grpc.ServiceError): boolean =>
  error.code === grpc.status.UNAVAILABLE || error.code === grpc.status.DEADLINE_EXCEEDED;

// server が応答として UNAVAILABLE を返した場合は応答ヘッダが metadata に載る。
// 接続自体が成立しない transport 失敗では metadata が空になる。この差で分類する。
const hasResponseMetadata = (error: grpc.ServiceError): boolean => {
  const metadata = error.metadata;
  if (metadata === undefined || metadata === null) {
    return false;
  }
  return Object.keys(metadata.getMap()).length > 0;
};

const toBackendUnavailable = (error: grpc.ServiceError): BackendUnavailableError =>
  new BackendUnavailableError(
    error,
    hasResponseMetadata(error) ? "server" : "transport",
    typeof error.details === "string" ? error.details : "",
    error.code === grpc.status.DEADLINE_EXCEEDED ? "no-response" : "unreachable",
  );

export const unary = <Request, Response>(
  invoke: (
    request: Request,
    metadata: grpc.Metadata,
    options: grpc.CallOptions,
    callback: (error: grpc.ServiceError | null, response: Response) => void,
  ) => unknown,
  request: Request,
  options: grpc.CallOptions,
): Promise<Response> =>
  new Promise<Response>((resolve, reject) => {
    invoke(request, new grpc.Metadata(), options, (error, response) => {
      if (error) {
        reject(isConnectionFailure(error) ? toBackendUnavailable(error) : error);
        return;
      }
      markBackendResponded();
      resolve(response);
    });
  });

export const timedUnary = async <Response>(
  timing: TimingSink | undefined,
  rpc: string,
  run: () => Promise<Response>,
): Promise<Response> => {
  if (timing === undefined) {
    return run();
  }
  const startedAt = performance.now();
  try {
    return await run();
  } finally {
    timing.recordRpc({ rpc, durationMs: performance.now() - startedAt });
  }
};

// 1件を識別子で引く RPC の結果から「見つからない」を作る。backend の NOT_FOUND と、要求と違う
// 対象の応答を notFound のエラーにし、backend を利用できない失敗はそのまま投げる。
export const expectFound = async <Response>(
  call: () => Promise<Response>,
  notFound: () => Error,
  isRequested: (response: Response) => boolean,
): Promise<Response> => {
  const response = await call().catch((error: unknown) => {
    if (!(error instanceof BackendUnavailableError) && (error as grpc.ServiceError).code === grpc.status.NOT_FOUND) {
      throw notFound();
    }
    throw error;
  });
  if (!isRequested(response)) {
    throw notFound();
  }
  return response;
};

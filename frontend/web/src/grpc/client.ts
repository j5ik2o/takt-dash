import * as grpc from "@grpc/grpc-js";
import type { ClientReadableStream } from "@grpc/grpc-js";
import {
  TaktDashServiceClient,
  type DashboardState,
  type GetActivityResponse,
  type GetProjectsResponse,
  type GetRunDetailResponse,
  type WatchStateRequest,
} from "../proto/taktdash";
import type { TimingSink } from "../telemetry/timing";

export const BACKEND_ADDR_ENV = "TAKTDASH_BACKEND_ADDR";

// backend の gRPC 既定ポート。backend 側の待受設定が確定するまで tonic の慣例値を置き、
// 実環境では TAKTDASH_BACKEND_ADDR で必ず上書きできるようにする。
export const DEFAULT_BACKEND_ADDR = "127.0.0.1:50051";

export type BackendEnv = Record<string, string | undefined>;

export function resolveBackendAddress(env: BackendEnv): string {
  const configured = env[BACKEND_ADDR_ENV];
  return configured !== undefined && configured !== "" ? configured : DEFAULT_BACKEND_ADDR;
}

/** backend 自体に接続できないことを表す。応答がない対象の扱いは backend が担う。 */
export class BackendUnavailableError extends Error {
  constructor(cause: unknown) {
    super("backend に接続できません。");
    this.name = "BackendUnavailableError";
    this.cause = cause;
  }
}

/** 指定した runSlug の run が backend に存在しないことを表す。ページは 404 を返す。 */
export class RunNotFoundError extends Error {
  constructor(runSlug: string) {
    super(`run「${runSlug}」は見つかりません。`);
    this.name = "RunNotFoundError";
  }
}

// 計測の有無は呼び出しごとに決める。共有クライアントでも要求をまたいで計測が混ざらない。
export type BackendClient = {
  getOverview(timing?: TimingSink): Promise<DashboardState>;
  getProjects(timing?: TimingSink): Promise<GetProjectsResponse>;
  // 判断待ちページは一覧だけを使うため、応答の state をそのまま返す。
  getPendingDecisions(timing?: TimingSink): Promise<DashboardState>;
  getActivity(timing?: TimingSink): Promise<GetActivityResponse>;
  getRunDetail(runSlug: string, timing?: TimingSink): Promise<GetRunDetailResponse>;
  watchState(): ClientReadableStream<DashboardState>;
  close(): Promise<void>;
};

export type CreateBackendClientOptions = {
  env?: BackendEnv;
};

const isConnectionFailure = (error: grpc.ServiceError): boolean =>
  error.code === grpc.status.UNAVAILABLE || error.code === grpc.status.DEADLINE_EXCEEDED;

const unary = <Request, Response>(
  invoke: (
    request: Request,
    callback: (error: grpc.ServiceError | null, response: Response) => void,
  ) => unknown,
  request: Request,
): Promise<Response> =>
  new Promise<Response>((resolve, reject) => {
    invoke(request, (error, response) => {
      if (error) {
        reject(isConnectionFailure(error) ? new BackendUnavailableError(error) : error);
        return;
      }
      resolve(response);
    });
  });

const timedUnary = async <Response>(
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

export function createBackendClient(options: CreateBackendClientOptions = {}): BackendClient {
  const env = options.env ?? process.env;
  const address = resolveBackendAddress(env);
  const client = new TaktDashServiceClient(address, grpc.credentials.createInsecure());

  return {
    getOverview: (timing) =>
      timedUnary(timing, "GetOverview", () =>
        unary<object, DashboardState>((request, callback) => client.getOverview(request, callback), {}),
      ),
    getProjects: (timing) =>
      timedUnary(timing, "GetProjects", () =>
        unary<object, GetProjectsResponse>((request, callback) => client.getProjects(request, callback), {}),
      ),
    getPendingDecisions: async (timing) => {
      const response = await timedUnary(timing, "GetPendingDecisions", () =>
        unary<object, { state: DashboardState | undefined }>(
          (request, callback) => client.getPendingDecisions(request, callback),
          {},
        ),
      );
      if (response.state === undefined) {
        throw new Error("GetPendingDecisions の応答に state がありません。");
      }
      return response.state;
    },
    getActivity: (timing) =>
      timedUnary(timing, "GetActivity", () =>
        unary<object, GetActivityResponse>((request, callback) => client.getActivity(request, callback), {}),
      ),
    getRunDetail: async (runSlug, timing) => {
      const detail = await timedUnary(timing, "GetRunDetail", () =>
        unary<{ runSlug: string }, GetRunDetailResponse>(
          (request, callback) => client.getRunDetail(request, callback),
          { runSlug },
        ),
      ).catch((error: unknown) => {
        if (error instanceof BackendUnavailableError) {
          throw error;
        }
        if ((error as grpc.ServiceError).code === grpc.status.NOT_FOUND) {
          throw new RunNotFoundError(runSlug);
        }
        throw error;
      });
      if (detail.runSlug !== runSlug) {
        throw new RunNotFoundError(runSlug);
      }
      return detail;
    },
    watchState: () => client.watchState({} satisfies WatchStateRequest),
    close: () =>
      new Promise<void>((resolve) => {
        client.close();
        resolve();
      }),
  };
}

// プロセスで1つのクライアントを使い回す。RSC と Route Handler は同じプロセスで動くため、
// 要求ごとに接続を作り直さず、gRPC の接続を再利用する。
let sharedClient: BackendClient | undefined;

/** プロセス共有のバックエンドクライアントを返す。無ければ生成する。 */
export function getBackendClient(): BackendClient {
  if (sharedClient === undefined) {
    sharedClient = createBackendClient();
  }
  return sharedClient;
}

/** プロセス共有のクライアントを破棄する。テストのリセットとプロセス終了だけが呼ぶ。 */
export async function closeBackendClient(): Promise<void> {
  const client = sharedClient;
  sharedClient = undefined;
  if (client !== undefined) {
    await client.close();
  }
}

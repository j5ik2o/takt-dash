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

export type BackendClient = {
  getOverview(): Promise<DashboardState>;
  getProjects(): Promise<GetProjectsResponse>;
  // 判断待ちページは一覧だけを使うため、応答の state をそのまま返す。
  getPendingDecisions(): Promise<DashboardState>;
  getActivity(): Promise<GetActivityResponse>;
  getRunDetail(runSlug: string): Promise<GetRunDetailResponse>;
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

export function createBackendClient(options: CreateBackendClientOptions = {}): BackendClient {
  const env = options.env ?? process.env;
  const address = resolveBackendAddress(env);
  const client = new TaktDashServiceClient(address, grpc.credentials.createInsecure());

  return {
    getOverview: () => unary<object, DashboardState>((request, callback) => client.getOverview(request, callback), {}),
    getProjects: () =>
      unary<object, GetProjectsResponse>((request, callback) => client.getProjects(request, callback), {}),
    getPendingDecisions: async () => {
      const response = await unary<object, { state: DashboardState | undefined }>(
        (request, callback) => client.getPendingDecisions(request, callback),
        {},
      );
      if (response.state === undefined) {
        throw new Error("GetPendingDecisions の応答に state がありません。");
      }
      return response.state;
    },
    getActivity: () =>
      unary<object, GetActivityResponse>((request, callback) => client.getActivity(request, callback), {}),
    getRunDetail: async (runSlug) => {
      const detail = await unary<{ runSlug: string }, GetRunDetailResponse>(
        (request, callback) => client.getRunDetail(request, callback),
        { runSlug },
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

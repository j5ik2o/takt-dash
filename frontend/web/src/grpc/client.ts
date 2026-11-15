import * as grpc from "@grpc/grpc-js";
import type { ClientReadableStream } from "@grpc/grpc-js";
import {
  TaktDashServiceClient,
  type DashboardState,
  type GetActivityResponse,
  type GetGoalDetailResponse,
  type GetProjectsResponse,
  type GetRunDetailResponse,
  type GetTaskDetailRequest,
  type GetTaskDetailResponse,
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

/** 指定した goalId のゴールが backend に存在しないことを表す。ページは 404 を返す。 */
export class GoalNotFoundError extends Error {
  constructor(goalId: string) {
    super(`ゴール「${goalId}」は見つかりません。`);
    this.name = "GoalNotFoundError";
  }
}

/** 指定したプロジェクトとタスク名のタスクが backend に存在しないことを表す。ページは 404 を返す。 */
export class TaskNotFoundError extends Error {
  constructor(project: string, taskName: string) {
    super(`プロジェクト「${project}」のタスク「${taskName}」は見つかりません。`);
    this.name = "TaskNotFoundError";
  }
}

export type BackendClient = {
  getOverview(): Promise<DashboardState>;
  getProjects(): Promise<GetProjectsResponse>;
  // 判断待ちページは一覧だけを使うため、応答の state をそのまま返す。
  getPendingDecisions(): Promise<DashboardState>;
  getActivity(): Promise<GetActivityResponse>;
  getRunDetail(runSlug: string): Promise<GetRunDetailResponse>;
  getGoalDetail(goalId: string): Promise<GetGoalDetailResponse>;
  getTaskDetail(project: string, taskName: string, goalId?: string): Promise<GetTaskDetailResponse>;
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
    getGoalDetail: async (goalId) => {
      const detail = await unary<{ goalId: string }, GetGoalDetailResponse>(
        (request, callback) => client.getGoalDetail(request, callback),
        { goalId },
      ).catch((error: unknown) => {
        if (error instanceof BackendUnavailableError) {
          throw error;
        }
        if ((error as grpc.ServiceError).code === grpc.status.NOT_FOUND) {
          throw new GoalNotFoundError(goalId);
        }
        throw error;
      });
      if (detail.goalId !== goalId) {
        throw new GoalNotFoundError(goalId);
      }
      return detail;
    },
    getTaskDetail: async (project, taskName, goalId) => {
      const request: GetTaskDetailRequest =
        goalId === undefined ? { project, taskName } : { project, taskName, goalId };
      const detail = await unary<GetTaskDetailRequest, GetTaskDetailResponse>(
        (unaryRequest, callback) => client.getTaskDetail(unaryRequest, callback),
        request,
      ).catch((error: unknown) => {
        if (error instanceof BackendUnavailableError) {
          throw error;
        }
        if ((error as grpc.ServiceError).code === grpc.status.NOT_FOUND) {
          throw new TaskNotFoundError(project, taskName);
        }
        throw error;
      });
      if (detail.project !== project || detail.taskName !== taskName) {
        throw new TaskNotFoundError(project, taskName);
      }
      if (goalId !== undefined && detail.goalId !== goalId) {
        throw new TaskNotFoundError(project, taskName);
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

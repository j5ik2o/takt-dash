import * as grpc from "@grpc/grpc-js";
import type { ClientReadableStream } from "@grpc/grpc-js";
import {
  TaktDashServiceClient,
  type AnswerInquiryRequest,
  type AnswerInquiryResponse,
  type ApproveGoalMergeRequest,
  type ApproveGoalMergeResponse,
  type ApproveGoalRegistrationRequest,
  type ApproveGoalRegistrationResponse,
  type DashboardState,
  type GetActivityResponse,
  type GetConversationRequest,
  type GetConversationResponse,
  type GetGoalDetailResponse,
  type GetProjectsResponse,
  type GetRunDetailResponse,
  type GetTaskDetailRequest,
  type GetTaskDetailResponse,
  type RejectGoalRegistrationRequest,
  type RejectGoalRegistrationResponse,
  type WatchStateRequest,
} from "../proto/taktdash";

export const BACKEND_ADDR_ENV = "TAKTDASH_BACKEND_ADDR";

// backend の gRPC 既定ポート。backend 側の待受設定が確定するまで tonic の慣例値を置き、
// 実環境では TAKTDASH_BACKEND_ADDR で必ず上書きできるようにする。
export const DEFAULT_BACKEND_ADDR = "127.0.0.1:50051";

// 書き込みの要求に設定する既定の期限。backend が固まっても無期限に待たないようにする。
export const DEFAULT_WRITE_DEADLINE_MS = 30_000;

export type BackendEnv = Record<string, string | undefined>;

export function resolveBackendAddress(env: BackendEnv): string {
  const configured = env[BACKEND_ADDR_ENV];
  return configured !== undefined && configured !== "" ? configured : DEFAULT_BACKEND_ADDR;
}

/**
 * backend を利用できないことを表す。
 *
 * transport は backend へ接続できない状態、server は backend が応答の中で
 * UNAVAILABLE を返した状態を指す。server の場合は backend が返した details を保つ。
 * 読み取りページはどちらも従来どおり「接続できない」表示として扱う。
 */
export type BackendUnavailableKind = "transport" | "server";

export class BackendUnavailableError extends Error {
  readonly kind: BackendUnavailableKind;
  readonly details: string;

  constructor(cause: unknown, kind: BackendUnavailableKind = "transport", details = "") {
    super("backend に接続できません。");
    this.name = "BackendUnavailableError";
    this.cause = cause;
    this.kind = kind;
    this.details = details;
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
  getConversation(request: GetConversationRequest): Promise<GetConversationResponse>;
  getGoalDetail(goalId: string): Promise<GetGoalDetailResponse>;
  getTaskDetail(project: string, taskName: string, goalId?: string): Promise<GetTaskDetailResponse>;
  watchState(): ClientReadableStream<DashboardState>;
  answerInquiry(request: AnswerInquiryRequest): Promise<AnswerInquiryResponse>;
  approveGoalMerge(request: ApproveGoalMergeRequest): Promise<ApproveGoalMergeResponse>;
  approveGoalRegistration(request: ApproveGoalRegistrationRequest): Promise<ApproveGoalRegistrationResponse>;
  rejectGoalRegistration(request: RejectGoalRegistrationRequest): Promise<RejectGoalRegistrationResponse>;
  close(): Promise<void>;
};

export type CreateBackendClientOptions = {
  env?: BackendEnv;
  // 書き込みの要求に設定する期限（ミリ秒）。読み取りには適用しない。
  writeDeadlineMs?: number;
  /**
   * unary RPC の deadline（ミリ秒）。省略時は既定の有限値を用いる。
   * 応答を返さない backend で無期限に待たないため、テストからは短い値を注入する。
   */
  deadlineMs?: number;
};

type CallOptionsFactory = () => grpc.CallOptions;

type ReadOperations = Pick<
  BackendClient,
  "getOverview" | "getProjects" | "getPendingDecisions" | "getActivity" | "getConversation"
>;

type DetailOperations = Pick<BackendClient, "getRunDetail" | "getGoalDetail" | "getTaskDetail">;

type WriteOperations = Pick<
  BackendClient,
  "answerInquiry" | "approveGoalMerge" | "approveGoalRegistration" | "rejectGoalRegistration"
>;

// unary RPC に付ける既定の deadline。backend 側の取得上限より長く、かつ有限にする。
const DEFAULT_UNARY_DEADLINE_MS = 30_000;

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
  );

const unary = <Request, Response>(
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
      resolve(response);
    });
  });

function createReadOperations(client: TaktDashServiceClient, readOptions: CallOptionsFactory): ReadOperations {
  return {
    getOverview: () =>
      unary<object, DashboardState>(
        (request, metadata, callOptions, callback) => client.getOverview(request, metadata, callOptions, callback),
        {},
        readOptions(),
      ),
    getProjects: () =>
      unary<object, GetProjectsResponse>(
        (request, metadata, callOptions, callback) => client.getProjects(request, metadata, callOptions, callback),
        {},
        readOptions(),
      ),
    getPendingDecisions: async () => {
      const response = await unary<object, { state: DashboardState | undefined }>(
        (request, metadata, callOptions, callback) =>
          client.getPendingDecisions(request, metadata, callOptions, callback),
        {},
        readOptions(),
      );
      if (response.state === undefined) {
        throw new Error("GetPendingDecisions の応答に state がありません。");
      }
      return response.state;
    },
    getActivity: () =>
      unary<object, GetActivityResponse>(
        (request, metadata, callOptions, callback) => client.getActivity(request, metadata, callOptions, callback),
        {},
        readOptions(),
      ),
    getConversation: (request) =>
      unary<GetConversationRequest, GetConversationResponse>(
        (value, metadata, callOptions, callback) =>
          client.getConversation(value, metadata, callOptions, callback),
        {
          project: request.project,
          sessionId: request.sessionId,
          afterSeq: request.afterSeq,
          limit: request.limit,
        },
        readOptions(),
      ),
  };
}

// 1件を識別子で引く読み取り。NOT_FOUND と識別子の不一致を、対象ごとの「見つからない」エラーへ写す。
function createDetailOperations(client: TaktDashServiceClient, readOptions: CallOptionsFactory): DetailOperations {
  return {
    getRunDetail: async (runSlug) => {
      const detail = await unary<{ runSlug: string }, GetRunDetailResponse>(
        (request, metadata, callOptions, callback) =>
          client.getRunDetail(request, metadata, callOptions, callback),
        { runSlug },
        readOptions(),
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
        (request, metadata, callOptions, callback) => client.getGoalDetail(request, metadata, callOptions, callback),
        { goalId },
        readOptions(),
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
        (unaryRequest, metadata, callOptions, callback) =>
          client.getTaskDetail(unaryRequest, metadata, callOptions, callback),
        request,
        readOptions(),
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
  };
}

function createWriteOperations(client: TaktDashServiceClient, writeOptions: CallOptionsFactory): WriteOperations {
  return {
    answerInquiry: (request) =>
      unary<AnswerInquiryRequest, AnswerInquiryResponse>(
        (req, metadata, callOptions, callback) => client.answerInquiry(req, metadata, callOptions, callback),
        request,
        writeOptions(),
      ),
    approveGoalMerge: (request) =>
      unary<ApproveGoalMergeRequest, ApproveGoalMergeResponse>(
        (req, metadata, callOptions, callback) => client.approveGoalMerge(req, metadata, callOptions, callback),
        request,
        writeOptions(),
      ),
    approveGoalRegistration: (request) =>
      unary<ApproveGoalRegistrationRequest, ApproveGoalRegistrationResponse>(
        (req, metadata, callOptions, callback) =>
          client.approveGoalRegistration(req, metadata, callOptions, callback),
        request,
        writeOptions(),
      ),
    rejectGoalRegistration: (request) =>
      unary<RejectGoalRegistrationRequest, RejectGoalRegistrationResponse>(
        (req, metadata, callOptions, callback) =>
          client.rejectGoalRegistration(req, metadata, callOptions, callback),
        request,
        writeOptions(),
      ),
  };
}

export function createBackendClient(options: CreateBackendClientOptions = {}): BackendClient {
  const env = options.env ?? process.env;
  const address = resolveBackendAddress(env);
  const deadlineMs = options.deadlineMs ?? DEFAULT_UNARY_DEADLINE_MS;
  const readOptions = (): grpc.CallOptions => ({ deadline: Date.now() + deadlineMs });
  const client = new TaktDashServiceClient(address, grpc.credentials.createInsecure());
  const writeDeadlineMs = options.writeDeadlineMs ?? DEFAULT_WRITE_DEADLINE_MS;
  const writeOptions = (): grpc.CallOptions => ({ deadline: Date.now() + writeDeadlineMs });

  return {
    ...createReadOperations(client, readOptions),
    ...createDetailOperations(client, readOptions),
    watchState: () => client.watchState({} satisfies WatchStateRequest),
    ...createWriteOperations(client, writeOptions),
    close: () =>
      new Promise<void>((resolve) => {
        client.close();
        resolve();
      }),
  };
}

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
  type GetRunContentRequest,
  type GetRunContentResponse,
  type GetRunDetailResponse,
  type GetRunUpdatesRequest,
  type GetRunUpdatesResponse,
  type GetTaskDetailRequest,
  type GetTaskDetailResponse,
  type RejectGoalRegistrationRequest,
  type RejectGoalRegistrationResponse,
  type WatchStateRequest,
} from "../proto/taktdash";
import type { TimingSink } from "../telemetry/timing";
import type { InstructionId, ReportFilename, RunSlug } from "../state/domain-primitives";
import type { RunLogCursor } from "../state/run-log-stream";
import { BackendUnavailableError, markBackendResponded } from "./backend-availability";

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

export {
  type BackendFailure,
  type BackendUnavailableKind,
  type BackendUnavailableSituation,
  BackendUnavailableError,
  backendUnavailableSituation,
  hasBackendResponded,
  markBackendResponded,
  resetBackendResponded,
} from "./backend-availability";

/** 指定した runSlug の run が backend に存在しないことを表す。ページは 404 を返す。 */
export class RunNotFoundError extends Error {
  constructor(runSlug: string) {
    super(`run「${runSlug}」は見つかりません。`);
    this.name = "RunNotFoundError";
  }
}

/** run のレポートか指示書の本文が backend に存在しないことを表す。Route Handler は 404 を返す。 */
export class RunContentNotFoundError extends Error {
  constructor(runSlug: string) {
    super(`run「${runSlug}」の本文は見つかりません。`);
    this.name = "RunContentNotFoundError";
  }
}

/** GetRunContent で本文を取得する対象。レポートか指示書のどちらか一方。 */
export type RunContentTarget = { report: ReportFilename } | { instruction: InstructionId };

// 計測の有無は呼び出しごとに決める。共有クライアントでも要求をまたいで計測が混ざらない。

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
  getOverview(timing?: TimingSink): Promise<DashboardState>;
  getProjects(timing?: TimingSink): Promise<GetProjectsResponse>;
  // 判断待ちページは一覧だけを使うため、応答の state をそのまま返す。
  getPendingDecisions(timing?: TimingSink): Promise<DashboardState>;
  getActivity(timing?: TimingSink): Promise<GetActivityResponse>;
  getRunDetail(runSlug: string, timing?: TimingSink): Promise<GetRunDetailResponse>;
  getRunContent(runSlug: RunSlug, target: RunContentTarget): Promise<GetRunContentResponse>;
  // 位置を省くと、backend は全体を置き換える行を返す。
  getRunUpdates(runSlug: RunSlug, cursor: RunLogCursor | undefined): Promise<GetRunUpdatesResponse>;
  getConversation(request: GetConversationRequest, timing?: TimingSink): Promise<GetConversationResponse>;
  getGoalDetail(goalId: string, timing?: TimingSink): Promise<GetGoalDetailResponse>;
  getTaskDetail(project: string, taskName: string, goalId?: string, timing?: TimingSink): Promise<GetTaskDetailResponse>;
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
    error.code === grpc.status.DEADLINE_EXCEEDED ? "no-response" : "unreachable",
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
      markBackendResponded();
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

// run を対象にする RPC の結果から「run が見つからない」を作る。backend の NOT_FOUND と、要求と違う
// runSlug の応答を RunNotFoundError にし、backend を利用できない失敗はそのまま投げる。
const expectRun = async <Response extends { runSlug: string }>(
  runSlug: string,
  call: () => Promise<Response>,
): Promise<Response> => {
  const response = await call().catch((error: unknown) => {
    if (error instanceof BackendUnavailableError) {
      throw error;
    }
    if ((error as grpc.ServiceError).code === grpc.status.NOT_FOUND) {
      throw new RunNotFoundError(runSlug);
    }
    throw error;
  });
  if (response.runSlug !== runSlug) {
    throw new RunNotFoundError(runSlug);
  }
  return response;
};

export function createBackendClient(options: CreateBackendClientOptions = {}): BackendClient {
  const env = options.env ?? process.env;
  const address = resolveBackendAddress(env);
  const deadlineMs = options.deadlineMs ?? DEFAULT_UNARY_DEADLINE_MS;
  const readOptions = (): grpc.CallOptions => ({ deadline: Date.now() + deadlineMs });
  const client = new TaktDashServiceClient(address, grpc.credentials.createInsecure());
  const writeDeadlineMs = options.writeDeadlineMs ?? DEFAULT_WRITE_DEADLINE_MS;
  const writeOptions = (): grpc.CallOptions => ({ deadline: Date.now() + writeDeadlineMs });

  return {
    getOverview: (timing) =>
      timedUnary(timing, "GetOverview", () =>
        unary<object, DashboardState>(
          (request, metadata, callOptions, callback) => client.getOverview(request, metadata, callOptions, callback),
          {},
          readOptions(),
        ),
      ),
    getProjects: (timing) =>
      timedUnary(timing, "GetProjects", () =>
        unary<object, GetProjectsResponse>(
          (request, metadata, callOptions, callback) => client.getProjects(request, metadata, callOptions, callback),
          {},
          readOptions(),
        ),
      ),
    getPendingDecisions: async (timing) => {
      const response = await timedUnary(timing, "GetPendingDecisions", () =>
        unary<object, { state: DashboardState | undefined }>(
          (request, metadata, callOptions, callback) =>
            client.getPendingDecisions(request, metadata, callOptions, callback),
          {},
          readOptions(),
        ),
      );
      if (response.state === undefined) {
        throw new Error("GetPendingDecisions の応答に state がありません。");
      }
      return response.state;
    },
    getActivity: (timing) =>
      timedUnary(timing, "GetActivity", () =>
        unary<object, GetActivityResponse>(
          (request, metadata, callOptions, callback) => client.getActivity(request, metadata, callOptions, callback),
          {},
          readOptions(),
        ),
      ),
    getRunDetail: (runSlug, timing) =>
      expectRun(runSlug, () =>
        timedUnary(timing, "GetRunDetail", () =>
          unary<{ runSlug: string }, GetRunDetailResponse>(
            (request, metadata, callOptions, callback) =>
              client.getRunDetail(request, metadata, callOptions, callback),
            { runSlug },
            readOptions(),
          ),
        ),
      ),
    getRunContent: async (runSlug, target) => {
      const request: GetRunContentRequest =
        "report" in target ? { runSlug, reportFilename: target.report } : { runSlug, instructionId: target.instruction };
      const content = await unary<GetRunContentRequest, GetRunContentResponse>(
        (unaryRequest, metadata, callOptions, callback) =>
          client.getRunContent(unaryRequest, metadata, callOptions, callback),
        request,
        readOptions(),
      ).catch((error: unknown) => {
        if (error instanceof BackendUnavailableError) {
          throw error;
        }
        if ((error as grpc.ServiceError).code === grpc.status.NOT_FOUND) {
          throw new RunContentNotFoundError(runSlug);
        }
        throw error;
      });
      if (content.runSlug !== runSlug) {
        throw new RunContentNotFoundError(runSlug);
      }
      return content;
    },
    getRunUpdates: (runSlug, cursor) => {
      const request: GetRunUpdatesRequest =
        cursor === undefined ? { runSlug } : { runSlug, epoch: cursor.epoch, afterSequence: cursor.sequence };
      return expectRun(runSlug, () =>
        unary<GetRunUpdatesRequest, GetRunUpdatesResponse>(
          (unaryRequest, metadata, callOptions, callback) =>
            client.getRunUpdates(unaryRequest, metadata, callOptions, callback),
          request,
          readOptions(),
        ),
      );
    },
    getConversation: (request, timing) =>
      timedUnary(timing, "GetConversation", () =>
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
      ),
    getGoalDetail: async (goalId, timing) => {
      const detail = await timedUnary(timing, "GetGoalDetail", () =>
        unary<{ goalId: string }, GetGoalDetailResponse>(
          (request, metadata, callOptions, callback) => client.getGoalDetail(request, metadata, callOptions, callback),
          { goalId },
          readOptions(),
        ),
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
    getTaskDetail: async (project, taskName, goalId, timing) => {
      const request: GetTaskDetailRequest =
        goalId === undefined ? { project, taskName } : { project, taskName, goalId };
      const detail = await timedUnary(timing, "GetTaskDetail", () =>
        unary<GetTaskDetailRequest, GetTaskDetailResponse>(
          (unaryRequest, metadata, callOptions, callback) =>
            client.getTaskDetail(unaryRequest, metadata, callOptions, callback),
          request,
          readOptions(),
        ),
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

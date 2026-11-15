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
  type GetProjectsResponse,
  type GetRunDetailResponse,
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

export type BackendClient = {
  getOverview(): Promise<DashboardState>;
  getProjects(): Promise<GetProjectsResponse>;
  // 判断待ちページは一覧だけを使うため、応答の state をそのまま返す。
  getPendingDecisions(): Promise<DashboardState>;
  getActivity(): Promise<GetActivityResponse>;
  getRunDetail(runSlug: string): Promise<GetRunDetailResponse>;
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
};

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

export function createBackendClient(options: CreateBackendClientOptions = {}): BackendClient {
  const env = options.env ?? process.env;
  const address = resolveBackendAddress(env);
  const client = new TaktDashServiceClient(address, grpc.credentials.createInsecure());
  const writeDeadlineMs = options.writeDeadlineMs ?? DEFAULT_WRITE_DEADLINE_MS;
  const writeOptions = (): grpc.CallOptions => ({ deadline: Date.now() + writeDeadlineMs });

  return {
    getOverview: () =>
      unary<object, DashboardState>(
        (request, metadata, callOptions, callback) => client.getOverview(request, metadata, callOptions, callback),
        {},
        {},
      ),
    getProjects: () =>
      unary<object, GetProjectsResponse>(
        (request, metadata, callOptions, callback) => client.getProjects(request, metadata, callOptions, callback),
        {},
        {},
      ),
    getPendingDecisions: async () => {
      const response = await unary<object, { state: DashboardState | undefined }>(
        (request, metadata, callOptions, callback) =>
          client.getPendingDecisions(request, metadata, callOptions, callback),
        {},
        {},
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
        {},
      ),
    getRunDetail: async (runSlug) => {
      const detail = await unary<{ runSlug: string }, GetRunDetailResponse>(
        (request, metadata, callOptions, callback) =>
          client.getRunDetail(request, metadata, callOptions, callback),
        { runSlug },
        {},
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

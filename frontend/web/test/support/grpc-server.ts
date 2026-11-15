// テスト用のダミー gRPC サーバー。
//
// 生成物の TaktDashServiceService（サービス定義）と TaktDashServiceClient を
// そのまま使うため、テストは本番と同じ proto の契約で通信する。
import * as grpc from "@grpc/grpc-js";
import {
  TaktDashServiceService,
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
} from "../../src/proto/taktdash.ts";

export type ServerError = { code: grpc.status; details: string };

// 書き込み系 RPC の呼び出しを、メソッド名と送られたリクエストで記録する。
export type WriteMethod =
  | "answerInquiry"
  | "approveGoalMerge"
  | "approveGoalRegistration"
  | "rejectGoalRegistration";

export type RecordedCall = {
  method: WriteMethod;
  request: unknown;
};

export type ServerBehavior = {
  overview?: DashboardState;
  projects?: GetProjectsResponse;
  pendingDecisions?: { state: DashboardState };
  activity?: GetActivityResponse;
  runDetail?: GetRunDetailResponse | ServerError;
  answerInquiry?: AnswerInquiryResponse | ServerError;
  approveGoalMerge?: ApproveGoalMergeResponse | ServerError;
  approveGoalRegistration?: ApproveGoalRegistrationResponse | ServerError;
  rejectGoalRegistration?: RejectGoalRegistrationResponse | ServerError;
  watch?: DashboardState[];
  // 読み取り系 RPC の応答を遅らせるミリ秒。読み取りに期限が付かないことの検証に使う。
  readDelayMs?: number;
  // 書き込み系 RPC の応答を遅らせるミリ秒。要求期限の検証に使う。
  writeDelayMs?: number;
  // ストリームを送り終えた後も接続を保ち、クライアントの切断を待つか。
  holdWatchOpen?: boolean;
  // WatchState を購読している呼び出しがキャンセルされたときに呼ばれる。
  onWatchCancelled?: () => void;
};

const notFound = (details: string): ServerError => ({ code: grpc.status.NOT_FOUND, details });

export type RunningServer = {
  address: string;
  port: number;
  // 書き込み系 RPC が受け取ったリクエスト（呼ばれた順）。
  requests: RecordedCall[];
  shutdown: () => Promise<void>;
};

const isServerError = (value: unknown): value is ServerError =>
  typeof value === "object" && value !== null && "code" in value;

// 応答を遅らせ、クライアントが要求を取り消したら保留中の処理を破棄する。
const respondAfterDelay = <Request, Response>(
  delayMs: number | undefined,
  call: grpc.ServerUnaryCall<Request, Response>,
  respond: () => void,
): void => {
  if (delayMs === undefined || delayMs <= 0) {
    respond();
    return;
  }
  const timer = setTimeout(respond, delayMs);
  call.once("cancelled", () => clearTimeout(timer));
};

const respondUnary = <Response>(
  result: Response | ServerError | undefined,
  callback: grpc.sendUnaryData<Response>,
): void => {
  if (result === undefined) {
    callback(notFound("未設定"));
    return;
  }
  if (isServerError(result)) {
    // server が応答としてエラーを返したことを、接続できない失敗と区別できるようにする。
    // trailer を渡すと実 backend と同じく応答ヘッダが metadata に載る。
    const trailer = new grpc.Metadata();
    trailer.set("x-taktdash-server-error", "1");
    callback(result, null, trailer);
    return;
  }
  callback(null, result);
};

export async function startTaktDashServer(behavior: ServerBehavior): Promise<RunningServer> {
  const server = new grpc.Server();
  const requests: RecordedCall[] = [];
  server.addService(TaktDashServiceService, {
    getOverview: (
      call: grpc.ServerUnaryCall<unknown, unknown>,
      callback: grpc.sendUnaryData<DashboardState>,
    ) => {
      respondAfterDelay(behavior.readDelayMs, call, () => {
        if (!behavior.overview) {
          callback(notFound("未設定"));
          return;
        }
        callback(null, behavior.overview);
      });
    },
    getProjects: (
      _call: grpc.ServerUnaryCall<unknown, unknown>,
      callback: grpc.sendUnaryData<GetProjectsResponse>,
    ) => {
      if (!behavior.projects) {
        callback(notFound("未設定"));
        return;
      }
      callback(null, behavior.projects);
    },
    getPendingDecisions: (
      _call: grpc.ServerUnaryCall<unknown, unknown>,
      callback: grpc.sendUnaryData<{ state: DashboardState }>,
    ) => {
      if (!behavior.pendingDecisions) {
        callback(notFound("未設定"));
        return;
      }
      callback(null, behavior.pendingDecisions);
    },
    getActivity: (
      _call: grpc.ServerUnaryCall<unknown, unknown>,
      callback: grpc.sendUnaryData<GetActivityResponse>,
    ) => {
      if (!behavior.activity) {
        callback(notFound("未設定"));
        return;
      }
      callback(null, behavior.activity);
    },
    getRunDetail: (
      _call: grpc.ServerUnaryCall<unknown, unknown>,
      callback: grpc.sendUnaryData<GetRunDetailResponse>,
    ) => {
      const result = behavior.runDetail;
      if (!result) {
        callback(notFound("run が見つかりません"));
        return;
      }
      if ("code" in result) {
        callback(result);
        return;
      }
      callback(null, result);
    },
    watchState: (call: grpc.ServerWritableStream<unknown, DashboardState>) => {
      let cancelled = false;
      call.on("cancelled", () => {
        cancelled = true;
        behavior.onWatchCancelled?.();
      });
      for (const state of behavior.watch ?? []) {
        call.write(state);
      }
      if (!behavior.holdWatchOpen) {
        call.end();
        return;
      }
      // クライアント切断まで接続を保つ。切断されれば cancelled が発火する。
      const timer = setInterval(() => {
        if (cancelled) {
          clearInterval(timer);
        }
      }, 10);
      void timer;
    },
    answerInquiry: (
      call: grpc.ServerUnaryCall<AnswerInquiryRequest, AnswerInquiryResponse>,
      callback: grpc.sendUnaryData<AnswerInquiryResponse>,
    ) => {
      requests.push({ method: "answerInquiry", request: call.request });
      respondAfterDelay(behavior.writeDelayMs, call, () => respondUnary(behavior.answerInquiry, callback));
    },
    approveGoalMerge: (
      call: grpc.ServerUnaryCall<ApproveGoalMergeRequest, ApproveGoalMergeResponse>,
      callback: grpc.sendUnaryData<ApproveGoalMergeResponse>,
    ) => {
      requests.push({ method: "approveGoalMerge", request: call.request });
      respondAfterDelay(behavior.writeDelayMs, call, () =>
        respondUnary(behavior.approveGoalMerge, callback),
      );
    },
    approveGoalRegistration: (
      call: grpc.ServerUnaryCall<ApproveGoalRegistrationRequest, ApproveGoalRegistrationResponse>,
      callback: grpc.sendUnaryData<ApproveGoalRegistrationResponse>,
    ) => {
      requests.push({ method: "approveGoalRegistration", request: call.request });
      respondAfterDelay(behavior.writeDelayMs, call, () =>
        respondUnary(behavior.approveGoalRegistration, callback),
      );
    },
    rejectGoalRegistration: (
      call: grpc.ServerUnaryCall<RejectGoalRegistrationRequest, RejectGoalRegistrationResponse>,
      callback: grpc.sendUnaryData<RejectGoalRegistrationResponse>,
    ) => {
      requests.push({ method: "rejectGoalRegistration", request: call.request });
      respondAfterDelay(behavior.writeDelayMs, call, () =>
        respondUnary(behavior.rejectGoalRegistration, callback),
      );
    },
  } as unknown as grpc.UntypedServiceImplementation);

  const port = await new Promise<number>((resolve, reject) => {
    server.bindAsync("127.0.0.1:0", grpc.ServerCredentials.createInsecure(), (error, boundPort) => {
      if (error) {
        reject(error);
        return;
      }
      resolve(boundPort);
    });
  });

  return {
    address: `127.0.0.1:${port}`,
    port,
    requests,
    shutdown: () =>
      new Promise<void>((resolve) => {
        server.tryShutdown(() => resolve());
      }),
  };
}

// 接続だけを拒否する状態（backend が落ちているとき）を表す。
// 空きポートを取ってすぐに閉じることで、接続できないアドレスを作る。
export async function reserveClosedAddress(): Promise<string> {
  const server = new grpc.Server();
  const port = await new Promise<number>((resolve, reject) => {
    server.bindAsync("127.0.0.1:0", grpc.ServerCredentials.createInsecure(), (error, boundPort) => {
      if (error) {
        reject(error);
        return;
      }
      resolve(boundPort);
    });
  });
  await new Promise<void>((resolve) => server.tryShutdown(() => resolve()));
  return `127.0.0.1:${port}`;
}

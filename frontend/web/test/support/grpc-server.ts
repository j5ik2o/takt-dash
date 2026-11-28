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
  type GetConversationRequest,
  type GetConversationResponse,
  type GetGoalDetailRequest,
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
  overview?: DashboardState | ServerError;
  projects?: GetProjectsResponse;
  pendingDecisions?: { state: DashboardState };
  activity?: GetActivityResponse;
  runDetail?: GetRunDetailResponse | ServerError;
  runContent?: GetRunContentResponse | ServerError;
  onGetRunContent?: (request: GetRunContentRequest) => void;
  runUpdates?: GetRunUpdatesResponse | ServerError;
  onGetRunUpdates?: (request: GetRunUpdatesRequest) => void;
  goalDetail?: GetGoalDetailResponse | ServerError;
  taskDetail?: GetTaskDetailResponse | ServerError;
  onGetGoalDetail?: (request: GetGoalDetailRequest) => void;
  onGetTaskDetail?: (request: GetTaskDetailRequest) => void;
  answerInquiry?: AnswerInquiryResponse | ServerError;
  approveGoalMerge?: ApproveGoalMergeResponse | ServerError;
  approveGoalRegistration?: ApproveGoalRegistrationResponse | ServerError;
  rejectGoalRegistration?: RejectGoalRegistrationResponse | ServerError;
  conversation?: GetConversationResponse | ServerError;
  watch?: DashboardState[];
  // 呼ばれた RPC の名前を、呼び出し順に記録する。
  calls?: string[];
  // 読み取り系 RPC の応答を遅らせるミリ秒。読み取りに期限が付かないことの検証に使う。
  readDelayMs?: number;
  // 書き込み系 RPC の応答を遅らせるミリ秒。要求期限の検証に使う。
  writeDelayMs?: number;
  // unary RPC の callback を呼ばず、クライアントの deadline による終了だけを観測させる。
  hangUnary?: boolean;
  // getOverview が受け取った deadline を観測する。既定値の生成そのものを確かめるテストで使う。
  onOverviewDeadline?: (deadline: grpc.Deadline) => void;
  // getRunDetail が受け取った deadline を観測する。
  onRunDetailDeadline?: (deadline: grpc.Deadline) => void;
  // ストリームを送り終えた後も接続を保ち、クライアントの切断を待つか。
  holdWatchOpen?: boolean;
  // WatchState を購読している呼び出しがキャンセルされたときに呼ばれる。
  onWatchCancelled?: () => void;
};

const notFound = (details: string): ServerError => ({ code: grpc.status.NOT_FOUND, details });

// 応答を返さず、クライアントのキャンセルだけで呼び出しを終わらせる。
// deadline による終了を観測するテストで、サーバー停止が未完了の呼び出しを待ち続けないようにする。
const hangUntilCancelled = <T>(call: grpc.ServerUnaryCall<unknown, unknown>, callback: grpc.sendUnaryData<T>): void => {
  call.on("cancelled", () => {
    callback({ code: grpc.status.CANCELLED, details: "クライアントがキャンセルしました" });
  });
};

export type RunningServer = {
  address: string;
  port: number;
  // 書き込み系 RPC が受け取ったリクエスト（呼ばれた順）。
  requests: RecordedCall[];
  // 受け取った GetConversation の要求を到着順に記録する。
  conversationRequests: GetConversationRequest[];
  // 受け取った WatchState の要求を到着順に記録する。
  watchRequests: WatchStateRequest[];
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
  const conversationRequests: GetConversationRequest[] = [];
  const watchRequests: WatchStateRequest[] = [];
  server.addService(TaktDashServiceService, {
    getOverview: (call: grpc.ServerUnaryCall<unknown, unknown>, callback: grpc.sendUnaryData<DashboardState>) => {
      behavior.calls?.push("GetOverview");
      behavior.onOverviewDeadline?.(call.getDeadline());
      if (behavior.hangUnary) {
        hangUntilCancelled(call, callback);
        return;
      }
      respondAfterDelay(behavior.readDelayMs, call, () => {
        const result = behavior.overview;
        if (!result) {
          callback(notFound("未設定"));
          return;
        }
        if ("code" in result) {
          callback(result);
          return;
        }
        callback(null, result);
      });
    },
    getProjects: (
      call: grpc.ServerUnaryCall<unknown, unknown>,
      callback: grpc.sendUnaryData<GetProjectsResponse>,
    ) => {
      behavior.calls?.push("GetProjects");
      if (behavior.hangUnary) {
        hangUntilCancelled(call, callback);
        return;
      }
      if (!behavior.projects) {
        callback(notFound("未設定"));
        return;
      }
      callback(null, behavior.projects);
    },
    getPendingDecisions: (
      call: grpc.ServerUnaryCall<unknown, unknown>,
      callback: grpc.sendUnaryData<{ state: DashboardState }>,
    ) => {
      behavior.calls?.push("GetPendingDecisions");
      if (behavior.hangUnary) {
        hangUntilCancelled(call, callback);
        return;
      }
      if (!behavior.pendingDecisions) {
        callback(notFound("未設定"));
        return;
      }
      callback(null, behavior.pendingDecisions);
    },
    getActivity: (
      call: grpc.ServerUnaryCall<unknown, unknown>,
      callback: grpc.sendUnaryData<GetActivityResponse>,
    ) => {
      behavior.calls?.push("GetActivity");
      if (behavior.hangUnary) {
        hangUntilCancelled(call, callback);
        return;
      }
      if (!behavior.activity) {
        callback(notFound("未設定"));
        return;
      }
      callback(null, behavior.activity);
    },
    getRunDetail: (
      call: grpc.ServerUnaryCall<unknown, unknown>,
      callback: grpc.sendUnaryData<GetRunDetailResponse>,
    ) => {
      behavior.calls?.push("GetRunDetail");
      behavior.onRunDetailDeadline?.(call.getDeadline());
      if (behavior.hangUnary) {
        hangUntilCancelled(call, callback);
        return;
      }
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
    getRunContent: (
      call: grpc.ServerUnaryCall<GetRunContentRequest, unknown>,
      callback: grpc.sendUnaryData<GetRunContentResponse>,
    ) => {
      behavior.calls?.push("GetRunContent");
      behavior.onGetRunContent?.(call.request);
      const result = behavior.runContent;
      if (!result) {
        callback(notFound("本文が見つかりません"));
        return;
      }
      if ("code" in result) {
        callback(result);
        return;
      }
      callback(null, result);
    },
    getRunUpdates: (
      call: grpc.ServerUnaryCall<GetRunUpdatesRequest, unknown>,
      callback: grpc.sendUnaryData<GetRunUpdatesResponse>,
    ) => {
      behavior.calls?.push("GetRunUpdates");
      behavior.onGetRunUpdates?.(call.request);
      const result = behavior.runUpdates;
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
    getGoalDetail: (
      call: grpc.ServerUnaryCall<GetGoalDetailRequest, unknown>,
      callback: grpc.sendUnaryData<GetGoalDetailResponse>,
    ) => {
      behavior.calls?.push("GetGoalDetail");
      behavior.onGetGoalDetail?.(call.request);
      const result = behavior.goalDetail;
      if (!result) {
        callback(notFound("ゴールが見つかりません"));
        return;
      }
      if ("code" in result) {
        callback(result);
        return;
      }
      callback(null, result);
    },
    getTaskDetail: (
      call: grpc.ServerUnaryCall<GetTaskDetailRequest, unknown>,
      callback: grpc.sendUnaryData<GetTaskDetailResponse>,
    ) => {
      behavior.calls?.push("GetTaskDetail");
      behavior.onGetTaskDetail?.(call.request);
      const result = behavior.taskDetail;
      if (!result) {
        callback(notFound("タスクが見つかりません"));
        return;
      }
      if ("code" in result) {
        callback(result);
        return;
      }
      callback(null, result);
    },
    getConversation: (
      call: grpc.ServerUnaryCall<GetConversationRequest, unknown>,
      callback: grpc.sendUnaryData<GetConversationResponse>,
    ) => {
      behavior.calls?.push("GetConversation");
      conversationRequests.push(call.request);
      const result = behavior.conversation;
      if (!result) {
        callback(notFound("会話がありません"));
        return;
      }
      if ("code" in result) {
        callback(result);
        return;
      }
      callback(null, result);
    },
    watchState: (call: grpc.ServerWritableStream<WatchStateRequest, DashboardState>) => {
      behavior.calls?.push("WatchState");
      watchRequests.push(call.request);
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
      behavior.calls?.push("AnswerInquiry");
      requests.push({ method: "answerInquiry", request: call.request });
      respondAfterDelay(behavior.writeDelayMs, call, () => respondUnary(behavior.answerInquiry, callback));
    },
    approveGoalMerge: (
      call: grpc.ServerUnaryCall<ApproveGoalMergeRequest, ApproveGoalMergeResponse>,
      callback: grpc.sendUnaryData<ApproveGoalMergeResponse>,
    ) => {
      behavior.calls?.push("ApproveGoalMerge");
      requests.push({ method: "approveGoalMerge", request: call.request });
      respondAfterDelay(behavior.writeDelayMs, call, () =>
        respondUnary(behavior.approveGoalMerge, callback),
      );
    },
    approveGoalRegistration: (
      call: grpc.ServerUnaryCall<ApproveGoalRegistrationRequest, ApproveGoalRegistrationResponse>,
      callback: grpc.sendUnaryData<ApproveGoalRegistrationResponse>,
    ) => {
      behavior.calls?.push("ApproveGoalRegistration");
      requests.push({ method: "approveGoalRegistration", request: call.request });
      respondAfterDelay(behavior.writeDelayMs, call, () =>
        respondUnary(behavior.approveGoalRegistration, callback),
      );
    },
    rejectGoalRegistration: (
      call: grpc.ServerUnaryCall<RejectGoalRegistrationRequest, RejectGoalRegistrationResponse>,
      callback: grpc.sendUnaryData<RejectGoalRegistrationResponse>,
    ) => {
      behavior.calls?.push("RejectGoalRegistration");
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
    conversationRequests,
    watchRequests,
    shutdown: () =>
      new Promise<void>((resolve) => {
        // 応答を返さない unary を残したままでもテストが終わるよう、猶予後に強制停止する。
        let settled = false;
        const finish = (): void => {
          if (settled) {
            return;
          }
          settled = true;
          clearTimeout(timer);
          resolve();
        };
        const timer = setTimeout(() => {
          server.forceShutdown();
          finish();
        }, 500);
        server.tryShutdown(finish);
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

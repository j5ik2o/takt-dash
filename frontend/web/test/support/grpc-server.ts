// テスト用のダミー gRPC サーバー。
//
// 生成物の TaktDashServiceService（サービス定義）と TaktDashServiceClient を
// そのまま使うため、テストは本番と同じ proto の契約で通信する。
import * as grpc from "@grpc/grpc-js";
import {
  TaktDashServiceService,
  type DashboardState,
  type GetActivityResponse,
  type GetConversationRequest,
  type GetConversationResponse,
  type GetProjectsResponse,
  type GetRunDetailResponse,
} from "../../src/proto/taktdash.ts";

export type ServerError = { code: grpc.status; details: string };

export type ServerBehavior = {
  overview?: DashboardState | ServerError;
  projects?: GetProjectsResponse;
  pendingDecisions?: { state: DashboardState };
  activity?: GetActivityResponse;
  runDetail?: GetRunDetailResponse | ServerError;
  conversation?: GetConversationResponse | ServerError;
  watch?: DashboardState[];
  // unary RPC の callback を呼ばず、クライアントの deadline による終了だけを観測させる。
  hangUnary?: boolean;
  // getOverview が受け取った deadline を観測する。既定値の生成そのものを確かめるテストで使う。
  onOverviewDeadline?: (deadline: grpc.Deadline) => void;
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
  // 受け取った GetConversation の要求を到着順に記録する。
  conversationRequests: GetConversationRequest[];
  shutdown: () => Promise<void>;
};

export async function startTaktDashServer(behavior: ServerBehavior): Promise<RunningServer> {
  const server = new grpc.Server();
  const conversationRequests: GetConversationRequest[] = [];
  server.addService(TaktDashServiceService, {
    getOverview: (call: grpc.ServerUnaryCall<unknown, unknown>, callback: grpc.sendUnaryData<DashboardState>) => {
      behavior.onOverviewDeadline?.(call.getDeadline());
      if (behavior.hangUnary) {
        hangUntilCancelled(call, callback);
        return;
      }
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
    },
    getProjects: (
      call: grpc.ServerUnaryCall<unknown, unknown>,
      callback: grpc.sendUnaryData<GetProjectsResponse>,
    ) => {
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
    getConversation: (
      call: grpc.ServerUnaryCall<GetConversationRequest, unknown>,
      callback: grpc.sendUnaryData<GetConversationResponse>,
    ) => {
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
    conversationRequests,
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

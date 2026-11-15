// テスト用のダミー gRPC サーバー。
//
// 生成物の TaktDashServiceService（サービス定義）と TaktDashServiceClient を
// そのまま使うため、テストは本番と同じ proto の契約で通信する。
import * as grpc from "@grpc/grpc-js";
import {
  TaktDashServiceService,
  type DashboardState,
  type GetActivityResponse,
  type GetGoalDetailRequest,
  type GetGoalDetailResponse,
  type GetProjectsResponse,
  type GetRunDetailResponse,
  type GetTaskDetailRequest,
  type GetTaskDetailResponse,
} from "../../src/proto/taktdash.ts";

export type ServerError = { code: grpc.status; details: string };

export type ServerBehavior = {
  overview?: DashboardState;
  projects?: GetProjectsResponse;
  pendingDecisions?: { state: DashboardState };
  activity?: GetActivityResponse;
  runDetail?: GetRunDetailResponse | ServerError;
  goalDetail?: GetGoalDetailResponse | ServerError;
  taskDetail?: GetTaskDetailResponse | ServerError;
  onGetGoalDetail?: (request: GetGoalDetailRequest) => void;
  onGetTaskDetail?: (request: GetTaskDetailRequest) => void;
  watch?: DashboardState[];
  // ストリームを送り終えた後も接続を保ち、クライアントの切断を待つか。
  holdWatchOpen?: boolean;
  // WatchState を購読している呼び出しがキャンセルされたときに呼ばれる。
  onWatchCancelled?: () => void;
};

const notFound = (details: string): ServerError => ({ code: grpc.status.NOT_FOUND, details });

export type RunningServer = {
  address: string;
  port: number;
  shutdown: () => Promise<void>;
};

export async function startTaktDashServer(behavior: ServerBehavior): Promise<RunningServer> {
  const server = new grpc.Server();
  server.addService(TaktDashServiceService, {
    getOverview: (_call: grpc.ServerUnaryCall<unknown, unknown>, callback: grpc.sendUnaryData<DashboardState>) => {
      if (!behavior.overview) {
        callback(notFound("未設定"));
        return;
      }
      callback(null, behavior.overview);
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
    getGoalDetail: (
      call: grpc.ServerUnaryCall<GetGoalDetailRequest, unknown>,
      callback: grpc.sendUnaryData<GetGoalDetailResponse>,
    ) => {
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

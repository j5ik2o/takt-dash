// 判断待ちの書き込み Route Handler の共通処理。
//
// 入力の形だけを検証し、値はそのまま gRPC の書き込み API へ渡す。業務的な検証
// （ゴールの所属、pull_request の有無など）は backend に任せ、ここでは重複しない。
import * as grpc from "@grpc/grpc-js";
import { BackendUnavailableError, getBackendClient, type BackendClient } from "../../../src/grpc/client";

const JSON_HEADERS = { "content-type": "application/json; charset=utf-8" } as const;

const jsonResponse = (status: number, body: unknown): Response =>
  new Response(JSON.stringify(body), { status, headers: JSON_HEADERS });

export const badRequest = (message: string): Response => jsonResponse(400, { ok: false, error: message });

// 本文を読む前の境界検証。ブラウザの別オリジンや cross-site からの書き込み、および
// JSON でない本文を受け付けない。ヘッダの無い CLI 等の既存利用は許可する。
const unsupportedMediaType = (): Response =>
  jsonResponse(415, { ok: false, error: "Content-Type は application/json で指定してください。" });

const forbiddenOrigin = (): Response =>
  jsonResponse(403, { ok: false, error: "このオリジンからは操作できません。" });

const contentTypeIsJson = (request: Request): boolean => {
  const header = request.headers.get("content-type");
  if (header === null) {
    return false;
  }
  const [mediaType] = header.split(";");
  return mediaType.trim().toLowerCase() === "application/json";
};

const ALLOWED_HOSTS_ENV = "TAKTDASH_ALLOWED_HOSTS";

// 環境変数が未設定のときに許可するホスト（LAN、Tailscale、ローカル）。
const DEFAULT_ALLOWED_HOSTS: readonly string[] = [
  "10.0.1.143:3000",
  "100.106.166.1:3000",
  "127.0.0.1:3000",
  "localhost:3000",
];

// host:port の形の値だけを小文字の host:port へ正規化する。パスや userinfo 等が付く値は無効とする。
const normalizeHost = (value: string): string | undefined => {
  try {
    const url = new URL(`http://${value}`);
    if (url.host !== value.toLowerCase() || url.username !== "" || url.password !== "") {
      return undefined;
    }
    if (url.pathname !== "/" || url.search !== "" || url.hash !== "") {
      return undefined;
    }
    return url.host;
  } catch {
    return undefined;
  }
};

// 起動設定で一覧を差し替えられるよう、リクエストのたびに環境変数から読む。
const resolveAllowedHosts = (env: Record<string, string | undefined>): readonly string[] => {
  const configured = env[ALLOWED_HOSTS_ENV];
  if (configured === undefined || configured.trim() === "") {
    return DEFAULT_ALLOWED_HOSTS;
  }
  return configured
    .split(",")
    .map((entry) => normalizeHost(entry.trim()))
    .filter((host): host is string => host !== undefined);
};

const requestOriginIsAllowed = (request: Request): boolean => {
  if (request.headers.get("sec-fetch-site") === "cross-site") {
    return false;
  }
  const origin = request.headers.get("origin");
  if (origin === null) {
    return true;
  }
  let originHost: string;
  try {
    originHost = new URL(origin).host;
  } catch {
    return false;
  }
  // Web は 0.0.0.0 で待ち受けるため request.url は利用者の開いた URL と一致しない。
  // ブラウザが開いた URL を表す Host ヘッダと比べる。x-forwarded-host は利用者が
  // 自由に付けられ、間に信頼できるプロキシも無いので判定に使わない。
  const hostHeader = request.headers.get("host");
  const host = hostHeader === null ? undefined : normalizeHost(hostHeader);
  if (host === undefined || originHost !== host) {
    return false;
  }
  // Host と Origin を同時に偽れる DNS リバインディングを防ぐため、既知のホストに限る。
  return resolveAllowedHosts(process.env).includes(host);
};

export type ReadJsonObjectResult =
  | { kind: "ok"; body: Record<string, unknown> }
  | { kind: "error"; response: Response };

export async function readJsonObject(request: Request): Promise<ReadJsonObjectResult> {
  if (!contentTypeIsJson(request)) {
    return { kind: "error", response: unsupportedMediaType() };
  }
  if (!requestOriginIsAllowed(request)) {
    return { kind: "error", response: forbiddenOrigin() };
  }
  try {
    const value: unknown = await request.json();
    if (typeof value !== "object" || value === null || Array.isArray(value)) {
      return {
        kind: "error",
        response: badRequest("リクエストの本文が JSON オブジェクトではありません。"),
      };
    }
    return { kind: "ok", body: value as Record<string, unknown> };
  } catch {
    return {
      kind: "error",
      response: badRequest("リクエストの本文が JSON オブジェクトではありません。"),
    };
  }
}

export const isNonEmptyString = (value: unknown): value is string =>
  typeof value === "string" && value.trim() !== "";

export const isPositiveInteger = (value: unknown): value is number =>
  typeof value === "number" && Number.isInteger(value) && value > 0;

// 書き込みを実行し、成功・入力不正・接続不能を HTTP の応答へ一度だけ変換する。
export async function runWrite(call: (client: BackendClient) => Promise<unknown>): Promise<Response> {
  const client = getBackendClient();
  try {
    await call(client);
    return jsonResponse(200, { ok: true });
  } catch (error) {
    if (error instanceof BackendUnavailableError) {
      // backend が応答の中で UNAVAILABLE を返した場合は、接続障害ではないため
      // 接続できない旨に置き換えず、backend が返した理由を伝える。
      if (error.kind === "server") {
        return jsonResponse(502, {
          ok: false,
          error: error.details !== "" ? error.details : "backend が操作を受け付けられませんでした。",
        });
      }
      return jsonResponse(503, { ok: false, error: "backend に接続できません。" });
    }
    const serviceError = error as grpc.ServiceError;
    if (serviceError.code === grpc.status.INVALID_ARGUMENT) {
      return jsonResponse(400, { ok: false, error: serviceError.details || "入力が正しくありません。" });
    }
    return jsonResponse(500, { ok: false, error: "操作に失敗しました。" });
  }
}

// 判断待ちの書き込み Route Handler の共通処理。
//
// 入力の形だけを検証し、値はそのまま gRPC の書き込み API へ渡す。業務的な検証
// （ゴールの所属、pull_request の有無など）は backend に任せ、ここでは重複しない。
import { isIP } from "node:net";
import * as grpc from "@grpc/grpc-js";
import { BackendUnavailableError, getBackendClient, type BackendClient } from "../../../src/grpc/client";

const JSON_HEADERS = { "content-type": "application/json; charset=utf-8" } as const;

const jsonResponse = (status: number, body: unknown): Response =>
  new Response(JSON.stringify(body), { status, headers: JSON_HEADERS });

export const badRequest = (message: string): Response => jsonResponse(400, { ok: false, error: message });

// 本文を読む前の境界検証。ブラウザの別オリジンや cross-site からの書き込み、
// Origin ヘッダの無い書き込み、および JSON でない本文を受け付けない。
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

// 公開の DNS では引けない名前の末尾。mDNS（.local）、家庭内のルータが配る名前（.lan、
// .home.arpa）、私設用に予約された名前（.internal）、ループバック（.localhost）。
const PRIVATE_NAME_SUFFIXES: readonly string[] = [".local", ".lan", ".home.arpa", ".internal", ".localhost"];

// 外部の者が DNS で向き先を決められないホスト名かどうか。
// DNS リバインディングは、攻撃者が管理する公開の名前を利用者の機械へ向け直して、
// Host と Origin をその名前でそろえる。IP アドレスそのもの、ドットの無い名前、私設の
// 名前はこの手口に使えないので、機械の IP が変わっても一覧の設定なしで許可できる。
const hostnameIsLocallyResolved = (hostname: string): boolean => {
  const unbracketed = hostname.startsWith("[") && hostname.endsWith("]") ? hostname.slice(1, -1) : hostname;
  // 末尾のドットは、名前を省略せずに書く表記（mac.local.）。同じ名前として判定する。
  const bare = unbracketed.endsWith(".") ? unbracketed.slice(0, -1) : unbracketed;
  if (isIP(bare) !== 0) {
    return true;
  }
  if (!bare.includes(".")) {
    return bare !== "";
  }
  return PRIVATE_NAME_SUFFIXES.some((suffix) => bare.endsWith(suffix));
};

// スキームの既定ポート。http と https 以外のスキームでは、ポートの省略と明示を
// 同一視しない。
const defaultPortForScheme = (protocol: string): string | undefined =>
  protocol === "http:" ? "80" : protocol === "https:" ? "443" : undefined;

// Origin のスキームの既定ポートだけは、明示しても省略しても同じホストとみなす。
// 別のスキームの既定ポートは通常のポートとして扱い、ポートの境界を保つ。
// 設定値・Host ヘッダ・Origin のすべてを、Origin のスキームを基準にこの形へ
// そろえてから比べる。
const canonicalHost = (hostname: string, port: string, defaultPort: string | undefined): string =>
  port === "" || port === defaultPort ? hostname : `${hostname}:${port}`;

// host:port の形の値から、書かれたとおりのホスト名とポートを取り出す。
// URL の解釈はそのスキームの既定ポートを落とすため、http で解釈してポートが
// 取れなければ https で解釈し直し、明示されたポートを失わないようにする。
// パスや userinfo 等が付く値は無効とする。
const parseHostPort = (value: string): { hostname: string; port: string } | undefined => {
  let url: URL;
  try {
    url = new URL(`http://${value}`);
  } catch {
    return undefined;
  }
  if (url.username !== "" || url.password !== "") {
    return undefined;
  }
  if (url.pathname !== "/" || url.search !== "" || url.hash !== "") {
    return undefined;
  }
  let port = url.port;
  if (port === "") {
    try {
      port = new URL(`https://${value}`).port;
    } catch {
      return undefined;
    }
  }
  return { hostname: url.hostname, port };
};

// host:port の形の値だけを、Origin のスキームで正規化した host:port へそろえる。
// URL への解釈で表記が変わる値（空白の除去や符号化が起きる値）は無効とする。
const normalizeHost = (value: string, defaultPort: string | undefined): string | undefined => {
  const parsed = parseHostPort(value);
  if (parsed === undefined) {
    return undefined;
  }
  // URL の hostname は、IPv6 のアドレスを角括弧つきで返す。
  const spelled = parsed.port === "" ? parsed.hostname : `${parsed.hostname}:${parsed.port}`;
  if (value.toLowerCase() !== spelled) {
    return undefined;
  }
  return canonicalHost(parsed.hostname, parsed.port, defaultPort);
};

// 上の判定に当たらない名前（公開の DNS で引ける名前）で開くときに、追加で許可する
// ホストの一覧。起動設定で差し替えられるよう、リクエストのたびに環境変数から読む。
// 設定値は、Origin のスキームを基準に正規化してから比べる。
const resolveAllowedHosts = (
  env: Record<string, string | undefined>,
  defaultPort: string | undefined,
): readonly string[] => {
  const configured = env[ALLOWED_HOSTS_ENV];
  if (configured === undefined) {
    return [];
  }
  return configured
    .split(",")
    .map((entry) => normalizeHost(entry.trim(), defaultPort))
    .filter((host): host is string => host !== undefined);
};

const requestOriginIsAllowed = (request: Request): boolean => {
  if (request.headers.get("sec-fetch-site") === "cross-site") {
    return false;
  }
  // ブラウザは POST に必ず Origin を付ける。Origin の無い書き込みは画面からの操作ではなく、
  // Host や許可ホストと照合する材料も無いので、宛先に関係なく拒否する。
  const origin = request.headers.get("origin");
  if (origin === null) {
    return false;
  }
  let originHostname: string;
  let originHost: string;
  let defaultPort: string | undefined;
  try {
    const originUrl = new URL(origin);
    defaultPort = defaultPortForScheme(originUrl.protocol);
    originHostname = originUrl.hostname;
    originHost = canonicalHost(originHostname, originUrl.port, defaultPort);
  } catch {
    return false;
  }
  // Web の待ち受けアドレスは利用者の開いた URL と一致するとは限らず、request.url は使えない。
  // ブラウザが開いた URL を表す Host ヘッダと比べる。x-forwarded-host は利用者が
  // 自由に付けられ、間に信頼できるプロキシも無いので判定に使わない。
  const hostHeader = request.headers.get("host");
  const host = hostHeader === null ? undefined : normalizeHost(hostHeader, defaultPort);
  if (host === undefined || originHost !== host) {
    return false;
  }
  // Host と Origin が同じ名前でそろう DNS リバインディングを防ぐため、外部の者が向き先を
  // 決められない名前か、設定で許可した名前に限る。
  return hostnameIsLocallyResolved(originHostname) || resolveAllowedHosts(process.env, defaultPort).includes(host);
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

// 鍵の形は backend の検証と同じ（1〜128 文字の ASCII 表示文字）。
const IDEMPOTENCY_KEY_PATTERN = /^[\x21-\x7e]{1,128}$/;

export type ReadIdempotencyKeyResult =
  | { kind: "ok"; idempotencyKey: string | undefined }
  | { kind: "error"; response: Response };

// やり直しで書き込みを重ねないための鍵を読む。鍵は省略でき、鍵の無い既存の利用は変わらない。
export function readIdempotencyKey(body: Record<string, unknown>): ReadIdempotencyKeyResult {
  const value = body.idempotencyKey;
  if (value === undefined) {
    return { kind: "ok", idempotencyKey: undefined };
  }
  if (typeof value !== "string" || !IDEMPOTENCY_KEY_PATTERN.test(value)) {
    return {
      kind: "error",
      response: badRequest("idempotencyKey は 1 から 128 文字の ASCII 表示文字で指定してください。"),
    };
  }
  return { kind: "ok", idempotencyKey: value };
}

const UNKNOWN_OUTCOME_MESSAGE =
  "操作の結果は不明です。期限内に応答がありませんでした。状態を確かめてから、必要ならやり直してください。";

// 書き込みを実行し、成功・入力不正・結果不明・接続不能を HTTP の応答へ一度だけ変換する。
export async function runWrite(call: (client: BackendClient) => Promise<unknown>): Promise<Response> {
  const client = getBackendClient();
  try {
    await call(client);
    return jsonResponse(200, { ok: true });
  } catch (error) {
    if (error instanceof BackendUnavailableError) {
      // 期限内に応答が無い書き込みは、backend（takt 側）では済んでいることがある。
      // 接続できない旨に置き換えず、結果が不明であることを伝える（issue #305）。
      if (error.failure === "no-response") {
        return jsonResponse(504, {
          ok: false,
          error: error.kind === "server" && error.details !== "" ? error.details : UNKNOWN_OUTCOME_MESSAGE,
        });
      }
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

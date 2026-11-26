#!/usr/bin/env node
// taktDash のバックエンドテスト用の、takt-mcp の HTTP transport（`takt-mcp --http`）を模す mock サーバー。
//
// 実機 dist の `infra/mcp/serverHttpTransport.js` と同じ契約を再現する:
//   - 127.0.0.1 のみで待ち受ける
//   - `Authorization` ヘッダの完全一致（不一致は 403、欠落は 401）
//   - path は `/` のみ（他は 404）
//   - `POST /` で `Mcp-Session-Id` を採番し、既知の ID の要求は同一セッションへ再利用する
//   - 未知の `Mcp-Session-Id`（サーバー再起動で消えた場合）は 404
//   - `DELETE /` と既知の `Mcp-Session-Id` でセッションを破棄する（以後その ID は 404）
//   - body は JSON-RPC（initialize・notifications/initialized・tools/call）
//
// 状態ファイル（--state）は mock_mcp_server.mjs と同じ形式を使い、読み取り道具の応答を返す。
// --port を省略した場合は OS が空きポートを採番する。採番した実際のポートと URL は
// --ready-file に JSON で書き出す（テストが接続先を組み立てるため）。
// --behavior で `deny`（常に 401）・`drop-session`（常に 404）・`fail-initialized`
// （notifications/initialized だけ 500）・`drop-session-then-refuse-initialize`（tools/call は
// 404、2 回目以降の initialize は 500）・`garble-initialize`（initialize の本文が壊れている）・
// `slow-initialize`（initialize と notifications/initialized を --delay-ms だけ遅らせる）・`hang-delete`
// （DELETE に応答しない）などを
// 起こせる。
//
// 追加の検証用オプション:
//   --calls <path>         受け取った tools/call を {"name","arguments"} の JSONL で追記する
//   --session-log <path>   セッションの採番・再利用・破棄のたびに "new" / "reuse" / "close" を追記する
//   --require-auth <token> 期待する Authorization の値（未指定なら "Bearer test-token"）
import { appendFileSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { createServer } from "node:http";
import { isAbsolute } from "node:path";

const argv = process.argv.slice(2);
const option = (name) => {
  const index = argv.indexOf(name);
  return index >= 0 ? argv[index + 1] : undefined;
};

const behavior = option("--behavior") ?? "ok";
let initializeHung = false;
// drop-connection-once で、最初の tools/call だけ応答を返さずに接続を切ったかどうか。
let connectionDropped = false;
const statePath = option("--state");
const callsPath = option("--calls");
const sessionLogPath = option("--session-log");
const readyFile = option("--ready-file");
const port = Number(option("--port") ?? 0);
const expectedAuthorization = option("--require-auth") ?? "Bearer test-token";

const PAGE_SIZE = 50;

const loadState = () => {
  if (!statePath) {
    return {};
  }
  return JSON.parse(readFileSync(statePath, "utf8"));
};

// takt_get_goal の textOffset / maxBytes によるテキストページ。stdio mock と同じ形で、
// 実 takt-mcp の sliceUtf8（forward）と同じく文字境界へそろえる。
const textPage = (text, offsetRaw, maxBytesRaw) => {
  const bytes = Buffer.from(String(text), "utf8");
  const maxBytes = Number(maxBytesRaw ?? 4096);
  let start = Math.min(Number(offsetRaw ?? 0), bytes.length);
  let end = Math.min(bytes.length, start + maxBytes);
  while (start < end && (bytes[start] & 0xc0) === 0x80) start++;
  while (end > start && end < bytes.length && (bytes[end] & 0xc0) === 0x80) end--;
  return {
    content: bytes.subarray(start, end).toString("utf8"),
    totalBytes: bytes.length,
    offset: start,
    returnedBytes: end - start,
    direction: "forward",
    truncated: start > 0 || end < bytes.length,
    nextOffset: end < bytes.length ? end : null,
  };
};

// 実 takt-mcp の項目不在（selectGoalValue の throw）と同じ道具エラー。stdio mock と同じ。
const goalFieldMissing = () => ({
  content: [{ type: "text", text: "Goal read failed: Selected field does not exist" }],
  isError: true,
});

const paged = (items, key, offsetRaw) => {
  const offset = Number(offsetRaw ?? 0);
  const page = items.slice(offset, offset + PAGE_SIZE);
  const nextOffset = offset + page.length < items.length ? offset + page.length : null;
  return {
    [key]: page,
    collection: key,
    totalCount: items.length,
    returnedCount: page.length,
    omittedCount: items.length - page.length,
    offset,
    nextOffset,
    truncated: offset + page.length < items.length,
  };
};

// takt 本体の listTaktGoalRecords と同じく、末尾から limit 件を切り出して反転する。
// offset は「先頭から何件を読み終えたか」で、1ページ目が最新の記録になります。
const pagedBackward = (items, key, offsetRaw) => {
  const offset = Number(offsetRaw ?? 0);
  const end = Math.max(0, items.length - offset);
  const start = Math.max(0, end - PAGE_SIZE);
  const page = items.slice(start, end).reverse();
  const nextOffset = offset + page.length < items.length ? offset + page.length : null;
  return {
    [key]: page,
    collection: key,
    totalCount: items.length,
    returnedCount: page.length,
    omittedCount: items.length - page.length,
    offset,
    nextOffset,
    truncated: nextOffset !== null,
  };
};

const asText = (payload) => ({ content: [{ type: "text", text: JSON.stringify(payload) }] });

// takt_get_run の view 応答は、実 takt-mcp と stdio 版モック（mock_mcp_server.mjs）と同じ形にします。
const runReportMissing = () => ({
  content: [{ type: "text", text: "Selected report does not exist" }],
  isError: true,
});

const runInstructionMissing = () => ({
  content: [{ type: "text", text: "Selected instruction does not exist" }],
  isError: true,
});

const runReportResponse = (state, args) => {
  const run = (state.runs ?? {})[args.runSlug];
  const report = (run?.reports ?? []).find((entry) => entry.filename === args.report);
  if (args.report === undefined || report === undefined) {
    return runReportMissing();
  }
  return asText({
    runSlug: args.runSlug,
    report: { filename: report.filename, ...textPage(report.content ?? "", args.offset, args.maxBytes) },
  });
};

const runInstructionsResponse = (state, args) => {
  const run = (state.runs ?? {})[args.runSlug];
  if (args.instructionId !== undefined) {
    const instruction = (run?.instructions ?? []).find((entry) => entry.instructionId === Number(args.instructionId));
    if (instruction === undefined) {
      return runInstructionMissing();
    }
    return asText({
      runSlug: args.runSlug,
      instruction: {
        instructionId: instruction.instructionId,
        issuedAt: instruction.issuedAt,
        state: instruction.state,
        ...textPage(instruction.content ?? "", args.offset, args.maxBytes),
      },
    });
  }
  if (run === undefined) {
    return asText({});
  }
  const instructions = run.instructions ?? [];
  const offset = Number(args.offset ?? 0);
  const limit = Number(args.limit ?? 50);
  const window = instructions.slice(offset, offset + limit);
  const nextOffset = offset + window.length < instructions.length ? offset + window.length : null;
  return asText({
    runSlug: args.runSlug,
    instructions: window,
    collection: "instructions",
    totalCount: instructions.length,
    returnedCount: window.length,
    omittedCount: instructions.length - window.length,
    offset,
    nextOffset,
    truncated: nextOffset !== null,
  });
};

const runTaskResponse = (state, args) => {
  const run = (state.runs ?? {})[args.runSlug];
  if (run === undefined) {
    return asText({});
  }
  return asText({ runSlug: args.runSlug, task: textPage(run.taskText ?? "", args.offset, args.maxBytes) });
};


// 実 takt-mcp は、登録案の一覧に持ち主の会話（conversationId）を付け、承認・却下で必須にする
// （takt-custom features/mcp/schemas.js の goalRegistrationOwnerSchema）。状態が
// conversationId を持たない提案には、この既定の持ち主を付ける。
const DEFAULT_CONVERSATION_ID = `manager-${"a".repeat(64)}`;
const CONVERSATION_ID_PATTERN = /^(manager|director)-[a-f0-9]{64}$/;

// 実 takt-mcp の一覧は、登録案の本文（summary）を返さず、読める項目の一覧（fields）だけを返す。
// 本文は takt_get_goal_registration_proposal で読む。
const listedProposal = ({ summary, ...identity }) => ({
  ...identity,
  fields:
    summary !== null && typeof summary === "object"
      ? Object.keys(summary).map((key) => ({ path: `summary.${key}` }))
      : [{ path: "summary" }],
});

// takt_get_goal_registration_proposal（path なし）の応答。持ち主でない会話と、保留中の登録案が
// 無い cwd は、実 takt-mcp と同じ文言の道具エラーにする。
const readProposal = (args, pending) => {
  const fail = (text) => ({ content: [{ type: "text", text: `Goal registration proposal read failed: ${text}` }], isError: true });
  const owner = pending[0]?.conversationId ?? DEFAULT_CONVERSATION_ID;
  if (args.conversationId !== owner) {
    return fail(`The conversationId ${args.conversationId} does not own the pending registration proposal of ${args.cwd}`);
  }
  if (pending.length === 0) {
    return fail(`No goal registration proposal is pending for ${args.cwd}`);
  }
  return asText({ proposal: pending[0] });
};

const withOwner = (proposals) =>
  proposals.map((proposal) => ({ conversationId: DEFAULT_CONVERSATION_ID, ...proposal }));

// 承認・却下の conversationId を、実 takt-mcp と同じ順に確かめる。形の誤りは入力の検証、
// 持ち主の食い違いは道具エラー。問題が無ければ undefined を返す。
const registrationOwnerError = (name, args, pending) => {
  const invalid = (detail) => ({
    content: [
      { type: "text", text: `MCP error -32602: Input validation error: Invalid arguments for tool ${name}: ${detail} at conversationId` },
    ],
    isError: true,
  });
  if (typeof args.conversationId !== "string") {
    return invalid("Invalid input: expected string, received undefined");
  }
  if (!CONVERSATION_ID_PATTERN.test(args.conversationId)) {
    return invalid("Invalid string: must match pattern /^(manager|director)-[a-f0-9]{64}$/");
  }
  const owner = pending[0]?.conversationId ?? DEFAULT_CONVERSATION_ID;
  if (args.conversationId !== owner) {
    const label = name === "takt_approve_goal_registration" ? "approval" : "rejection";
    return {
      content: [
        {
          type: "text",
          text: `Goal registration ${label} failed: The conversationId ${args.conversationId} does not own the pending registration proposal of ${args.cwd}`,
        },
      ],
      isError: true,
    };
  }
  return undefined;
};

// tools/call に応答する。mock_mcp_server.mjs と同じく、状態ファイルの内容を読み取り道具の応答にする。
const handleToolCall = (name, args) => {
  const state = loadState();
  const isError = new Set(state.isError ?? []);
  const scopedKey = `${name}:${args.goalId ?? args.runSlug ?? args.workflow ?? ""}`;
  if (isError.has(name) || isError.has(scopedKey)) {
    return { content: [{ type: "text", text: `${name} は失敗しました。` }], isError: true };
  }
  // isError ではないのに本文（content[0].text）の無い応答を返す（issue #395）。
  if ((state.noText ?? []).includes(name)) {
    return { content: [] };
  }
  // 道具エラーの文言の指定。実 takt-mcp が返す文言（stale_summary など）を再現する。
  if (state.toolErrors !== undefined && Object.prototype.hasOwnProperty.call(state.toolErrors, name)) {
    return { content: [{ type: "text", text: state.toolErrors[name] }], isError: true };
  }
  switch (name) {
    case "takt_list_goals":
      return asText(paged(state.goals ?? [], "goals", args.offset));
    case "takt_list_tasks":
      return asText(paged(state.tasks ?? [], "tasks", args.offset));
    case "takt_list_goal_questions":
      return asText({ goalId: args.goalId, ...paged((state.goalQuestions ?? {})[args.goalId] ?? [], "questions", args.offset) });
    case "takt_list_goal_work_log":
      return asText(pagedBackward((state.goalDetails ?? {})[args.goalId]?.workLog ?? [], "workLog", args.offset));
    case "takt_list_standalone_inquiries":
      return asText(paged(state.standaloneInquiries ?? [], "inquiries", args.offset));
    case "takt_get_goal": {
      if (args.path === "notifications") {
        return asText(paged((state.notifications ?? {})[args.goalId] ?? [], "items", args.offset));
      }
      const detail = (state.goalDetails ?? {})[args.goalId] ?? {};
      if (args.path === undefined) {
        return asText({ goal: detail.summary ?? {} });
      }
      if ((detail.omitFields ?? []).includes(String(args.path))) {
        return asText({ goalId: args.goalId, path: args.path });
      }
      if (Object.hasOwn(detail.malformedFields ?? {}, String(args.path))) {
        return asText(detail.malformedFields[String(args.path)]);
      }
      if (String(args.path).startsWith("specifications.")) {
        const notation = String(args.path).slice("specifications.".length);
        return asText(textPage((detail.specifications ?? {})[notation] ?? "", args.textOffset, args.maxBytes));
      }
      const value = detail[args.path];
      if (Array.isArray(value)) {
        return asText(paged(value, "items", args.offset));
      }
      if (typeof value === "string") {
        return asText(textPage(value, args.textOffset, args.maxBytes));
      }
      if (value === undefined) {
        return goalFieldMissing();
      }
      return asText({ items: value });
    }
    case "takt_get_run": {
      if (args.view === "report") {
        return runReportResponse(state, args);
      }
      if (args.view === "instructions") {
        return runInstructionsResponse(state, args);
      }
      if (args.view === "task") {
        return runTaskResponse(state, args);
      }
      const run = (state.runs ?? {})[args.runSlug];
      if (run === undefined) {
        return asText({});
      }
      if (args.report !== undefined) {
        const report = (run.reports ?? []).find((entry) => entry.filename === args.report);
        return asText(report === undefined ? {} : { filename: report.filename, content: report.content ?? "", sizeBytes: report.sizeBytes ?? 0 });
      }
      if (args.instructionId !== undefined) {
        const instruction = (run.instructions ?? []).find((entry) => entry.instructionId === Number(args.instructionId));
        return asText(instruction === undefined ? {} : instruction);
      }
      const stepLogs = run.stepLogs ?? [];
      const offset = Number(args.offset ?? 0);
      const window = stepLogs.slice(offset, offset + PAGE_SIZE);
      const nextOffset = offset + window.length < stepLogs.length ? offset + window.length : null;
      return asText({
        ...run,
        stepLogs: window,
        stepLogPage: {
          collection: "stepLogs",
          totalCount: stepLogs.length,
          returnedCount: window.length,
          omittedCount: stepLogs.length - window.length,
          offset,
          nextOffset,
          truncated: offset + window.length < stepLogs.length,
        },
      });
    }
    case "takt_list_goal_registration_proposals":
      return asText({ proposals: withOwner(state.proposals ?? []).map(listedProposal) });
    case "takt_get_goal_registration_proposal":
      return readProposal(args, withOwner(state.proposals ?? []));
    // 書き込み系。応答の形は stdio 版モック（mock_mcp_server.mjs）と同じにします。
    case "takt_answer_goal_question":
      return asText({ goalId: args.goalId, questionId: args.questionId, answer: args.answer });
    case "takt_approve_goal_registration":
      return (
        registrationOwnerError(name, args, withOwner(state.proposals ?? [])) ??
        asText({ revision: args.revision, goal: { id: `approved-${args.revision}` } })
      );
    case "takt_reject_goal_registration":
      return (
        registrationOwnerError(name, args, withOwner(state.proposals ?? [])) ??
        asText({ revision: args.revision, notice: "却下しました" })
      );
    case "takt_merge_goal_pr":
      return asText({
        taskName: "merge",
        status: "enqueued",
        goalId: args.goalId,
        prNumber: args.prNumber,
        mergeTask: "merge",
        workflow: "merge-goal-pr",
        until: args.until,
      });
    default:
      return { content: [{ type: "text", text: `未知の道具です: ${name}` }], isError: true };
  }
};

// 採番済みセッションの集合。未知の ID は 404 で返す（実機の serverHttpTransport.js と同じ）。
const sessions = new Set();
let sessionCounter = 0;

const jsonBody = (response, status, message, extraHeaders = {}) => {
  const body = JSON.stringify(message);
  response.writeHead(status, {
    "content-type": "application/json",
    "content-length": Buffer.byteLength(body),
    ...extraHeaders,
  });
  response.end(body);
};

const server = createServer((request, response) => {
  if (behavior === "deny") {
    jsonBody(response, 401, { jsonrpc: "2.0", error: { code: -32001, message: "unauthorized" }, id: null });
    return;
  }
  const authorization = request.headers["authorization"];
  if (authorization === undefined) {
    jsonBody(response, 401, { jsonrpc: "2.0", error: { code: -32001, message: "unauthorized" }, id: null });
    return;
  }
  if (authorization !== expectedAuthorization) {
    jsonBody(response, 403, { jsonrpc: "2.0", error: { code: -32003, message: "forbidden" }, id: null });
    return;
  }
  if (request.url !== "/") {
    jsonBody(response, 404, { jsonrpc: "2.0", error: { code: -32601, message: "not found" }, id: null });
    return;
  }
  if (request.method === "DELETE") {
    // 実機は Mcp-Session-Id の無い DELETE を 400、未知の ID を 404 で返し、既知の ID は破棄する。
    const sessionId = request.headers["mcp-session-id"];
    request.resume();
    if (sessionId === undefined) {
      response.writeHead(400).end();
      return;
    }
    if (behavior === "hang-delete") {
      // DELETE に応答せず、セッションも破棄しない。応答しないサーバーを閉じる段で待つ状態を再現する。
      return;
    }
    if (!sessions.delete(sessionId)) {
      response.writeHead(404).end();
      return;
    }
    if (sessionLogPath !== undefined) {
      appendFileSync(sessionLogPath, `close ${sessionId}\n`);
    }
    response.writeHead(200).end();
    return;
  }
  if (request.method !== "POST") {
    response.writeHead(405).end();
    return;
  }
  const chunks = [];
  request.on("data", (chunk) => chunks.push(chunk));
  request.on("end", () => {
    let message;
    try {
      message = JSON.parse(Buffer.concat(chunks).toString("utf8"));
    } catch {
      jsonBody(response, 400, { jsonrpc: "2.0", error: { code: -32700, message: "parse error" }, id: null });
      return;
    }
    if (behavior === "drop-session") {
      jsonBody(response, 404, { jsonrpc: "2.0", error: { code: -32002, message: "session not found" }, id: message.id ?? null });
      return;
    }
    if (message.method === "initialize") {
      if (behavior === "drop-session-then-refuse-initialize" && sessionCounter > 0) {
        // 2 回目以降の initialize を拒否する。消えたセッションを作り直せない状態を再現する。
        jsonBody(response, 500, { jsonrpc: "2.0", error: { code: -32000, message: "unavailable" }, id: message.id ?? null });
        return;
      }
      if (behavior === "hang-initialize-once" && !initializeHung) {
        // 最初の initialize だけ応答しない。セッションの確立が時間切れになり、
        // 道具の呼び出しが一度も届いていない状態を再現する。
        initializeHung = true;
        return;
      }
      sessionCounter += 1;
      const sessionId = `sess-${sessionCounter}-${Date.now()}`;
      sessions.add(sessionId);
      if (sessionLogPath !== undefined) {
        appendFileSync(sessionLogPath, `new ${sessionId}\n`);
      }
      if (behavior === "garble-initialize") {
        // セッションは採番して Mcp-Session-Id を返すが、本文が JSON として壊れている状態を再現する。
        response.writeHead(200, { "content-type": "application/json", "mcp-session-id": sessionId });
        response.end("{ 壊れた本文");
        return;
      }
      if (behavior === "slow-initialize") {
        // セッションは採番するが、応答を遅らせる。確立の途中で閉じる・同時に待つ状態を再現する。
        const delay = Number(option("--delay-ms") ?? 300);
        setTimeout(() => {
          jsonBody(
            response,
            200,
            { jsonrpc: "2.0", id: message.id, result: { protocolVersion: "2025-06-18", capabilities: { tools: {} }, serverInfo: { name: "taktdash-mock-http", version: "0.0.0" } } },
            { "mcp-session-id": sessionId },
          );
        }, delay);
        return;
      }
      jsonBody(
        response,
        200,
        {
          jsonrpc: "2.0",
          id: message.id,
          result: {
            protocolVersion: message.params?.protocolVersion ?? "2025-06-18",
            capabilities: { tools: {} },
            serverInfo: { name: "taktdash-mock-http", version: "0.0.0" },
          },
        },
        { "mcp-session-id": sessionId },
      );
      return;
    }
    const sessionId = request.headers["mcp-session-id"];
    if (sessionId !== undefined) {
      if (!sessions.has(sessionId)) {
        jsonBody(response, 404, { jsonrpc: "2.0", error: { code: -32002, message: "session not found" }, id: message.id ?? null });
        return;
      }
      if (sessionLogPath !== undefined) {
        appendFileSync(sessionLogPath, `reuse ${sessionId}\n`);
      }
    }
    if (message.method === "notifications/initialized" && behavior === "slow-initialize") {
      // 通知の受け付けも遅らせ、確立が 2 往復ぶん鍵を持つ状態を再現する。
      setTimeout(() => response.writeHead(202).end(), Number(option("--delay-ms") ?? 300));
      return;
    }
    if (message.method === "notifications/initialized") {
      if (behavior === "fail-initialized") {
        // セッションは採番済みだが、初期化の通知が失敗する状態を再現する。
        response.writeHead(500).end();
        return;
      }
      response.writeHead(202).end();
      return;
    }
    if (message.method === "tools/call") {
      const { name, arguments: args } = message.params ?? {};
      if (behavior === "drop-connection-once" && !connectionDropped) {
        // 最初の tools/call は、サーバーが要求を受け取った後で応答が届く前に接続が
        // 切れた状態を再現する。呼び出しの記録は残し、応答は返さずに接続を切る。
        connectionDropped = true;
        if (callsPath !== undefined) {
          appendFileSync(callsPath, `${JSON.stringify({ name, arguments: args })}\n`);
        }
        request.socket.destroy();
        return;
      }
      if (behavior === "drop-session-on-call" || behavior === "drop-session-then-refuse-initialize") {
        // initialize は成功してセッションが採番されるが、tools/call の時点では
        // セッションが消えている状態を再現する。呼び出しの記録は残す。
        if (callsPath !== undefined) {
          appendFileSync(callsPath, `${JSON.stringify({ name, arguments: args })}\n`);
        }
        jsonBody(response, 404, { jsonrpc: "2.0", error: { code: -32002, message: "session not found" }, id: message.id ?? null });
        return;
      }
      if (callsPath !== undefined) {
        appendFileSync(callsPath, `${JSON.stringify({ name, arguments: args })}\n`);
      }
      const cwd = args?.cwd;
      if (typeof cwd !== "string" || !isAbsolute(cwd)) {
        jsonBody(response, 200, { jsonrpc: "2.0", id: message.id, error: { code: -32602, message: "cwd は絶対パスの文字列で指定してください" } });
        return;
      }
      // 状態の delayMs（mock_mcp_server.mjs と同じ形式）で、道具ごとに応答を遅らせる。
      const delayMs = loadState().delayMs?.[name] ?? 0;
      setTimeout(() => {
        jsonBody(response, 200, { jsonrpc: "2.0", id: message.id, result: handleToolCall(name, args) });
      }, delayMs);
      return;
    }
    if (message.id !== undefined) {
      jsonBody(response, 200, { jsonrpc: "2.0", id: message.id, error: { code: -32601, message: `未知のメソッドです: ${message.method}` } });
    } else {
      response.writeHead(202).end();
    }
  });
});

server.listen(port, "127.0.0.1", () => {
  const address = server.address();
  const url = `http://127.0.0.1:${address.port}/`;
  if (readyFile !== undefined) {
    // テストは ready ファイルが現れたらすぐ読む。書きかけを読ませないよう、別名で書き終えてから置き換える。
    const partial = `${readyFile}.partial`;
    writeFileSync(partial, JSON.stringify({ port: address.port, url }));
    renameSync(partial, readyFile);
  }
  process.stdout.write(`MCP HTTP listening on ${url}\n`);
});

const shutdown = () => {
  server.close(() => process.exit(0));
};
process.on("SIGTERM", shutdown);
process.on("SIGINT", shutdown);

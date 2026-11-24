#!/usr/bin/env node
// taktDash のバックエンドテスト用の、takt-mcp の HTTP transport（`takt-mcp --http`）を模す mock サーバー。
//
// 実機 dist の `infra/mcp/serverHttpTransport.js` と同じ契約を再現する:
//   - 127.0.0.1 のみで待ち受ける
//   - `Authorization` ヘッダの完全一致（不一致は 403、欠落は 401）
//   - path は `/` のみ（他は 404）
//   - `POST /` で `Mcp-Session-Id` を採番し、既知の ID の要求は同一セッションへ再利用する
//   - 未知の `Mcp-Session-Id`（サーバー再起動で消えた場合）は 404
//   - body は JSON-RPC（initialize・notifications/initialized・tools/call）
//
// 状態ファイル（--state）は mock_mcp_server.mjs と同じ形式を使い、読み取り道具の応答を返す。
// --port を省略した場合は OS が空きポートを採番する。採番した実際のポートと URL は
// --ready-file に JSON で書き出す（テストが接続先を組み立てるため）。
// --behavior で `deny`（常に 401）・`drop-session`（常に 404）を起こせる。
//
// 追加の検証用オプション:
//   --calls <path>         受け取った tools/call を {"name","arguments"} の JSONL で追記する
//   --session-log <path>   セッション採番と再利用のたびに "new" / "reuse" を追記する
//   --require-auth <token> 期待する Authorization の値（未指定なら "Bearer test-token"）
import { appendFileSync, readFileSync, writeFileSync } from "node:fs";
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


// tools/call に応答する。mock_mcp_server.mjs と同じく、状態ファイルの内容を読み取り道具の応答にする。
const handleToolCall = (name, args) => {
  const state = loadState();
  const isError = new Set(state.isError ?? []);
  const scopedKey = `${name}:${args.goalId ?? args.runSlug ?? args.workflow ?? ""}`;
  if (isError.has(name) || isError.has(scopedKey)) {
    return { content: [{ type: "text", text: `${name} は失敗しました。` }], isError: true };
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
      return asText({ proposals: state.proposals ?? [] });
    // 書き込み系。応答の形は stdio 版モック（mock_mcp_server.mjs）と同じにします。
    case "takt_answer_goal_question":
      return asText({ goalId: args.goalId, questionId: args.questionId, answer: args.answer });
    case "takt_approve_goal_registration":
      return asText({ revision: args.revision, goal: { id: `approved-${args.revision}` } });
    case "takt_reject_goal_registration":
      return asText({ revision: args.revision, notice: "却下しました" });
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
      if (behavior === "hang-initialize-once" && !initializeHung) {
        // 最初の initialize だけ応答しない。セッションの確立が時間切れになり、
        // 道具の呼び出しが一度も届いていない状態を再現する。
        initializeHung = true;
        return;
      }
      const sessionId = `sess-${sessions.size + 1}-${Date.now()}`;
      sessions.add(sessionId);
      if (sessionLogPath !== undefined) {
        appendFileSync(sessionLogPath, `new ${sessionId}\n`);
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
    if (message.method === "notifications/initialized") {
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
      if (behavior === "drop-session-on-call") {
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
      jsonBody(response, 200, { jsonrpc: "2.0", id: message.id, result: handleToolCall(name, args) });
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
    writeFileSync(readyFile, JSON.stringify({ port: address.port, url }));
  }
  process.stdout.write(`MCP HTTP listening on ${url}\n`);
});

const shutdown = () => {
  server.close(() => process.exit(0));
};
process.on("SIGTERM", shutdown);
process.on("SIGINT", shutdown);

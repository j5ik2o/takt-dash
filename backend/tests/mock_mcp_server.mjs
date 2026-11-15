#!/usr/bin/env node
// taktDash のバックエンドテスト用の mock MCP サーバ。
//
// 標準入力の改行区切り JSON-RPC を読み、標準出力へ改行区切り JSON-RPC で応答する。
// 状態ファイル（--state）の内容を takt の読み取り道具の応答として返す。
// --behavior で意図的に起動失敗・タイムアウト・不正応答・道具エラーを起こせる。
//
// 実 takt-mcp の入力契約に合わせ、各読み取り道具の引数を検証する:
//   - cwd は絶対パスの文字列で必須
//   - 道具ごとに既知のキー以外は拒否（strict）
//   - 検証不合格は JSON-RPC error（InvalidParams, -32602）
//
// 状態ファイルの形式:
//   {
//     "goals": [ {"id","status","executionStatus","objective","pullRequest"} ],
//     "tasks": [ {"name","goalId","status","startedAt","runSlug","currentStep","mergeTask"} ],
//     "goalQuestions": { "<goalId>": [ {"id","status","kind","body","options","createdAt"} ] },
//     "standaloneInquiries": [ {"id","status","kind","body","options","createdAt"} ],
//     "runs": { "<runSlug>": {"runSlug","currentStep","startedAt","currentStepStartedAt","stepLogs",
//                              "sessionId","summary","workflow","status","currentIteration","currentStepExecutionCount",
//                              "phase","logTail","lastLogAt","taskText","reports","instructions"} },
//     "workflows": { "<workflow名>": ["plan", "implement", ...] },
//     "notifications": { "<goalId>": [ {"id","kind","body","recordedAt"} ] },
//     "proposals": [ {"revision","repositoryDirectory","summary"} ],
//     "proposalsByCwd": { "<cwd>": [ {"revision","repositoryDirectory","summary"} ] },
//     "responses": { "<道具名>": {...} },  // 指定した書き込み道具の応答を丸ごと差し替える
//     "isError": ["takt_list_tasks", "takt_get_run:r1"]  // 指定した道具を isError で失敗させる。
//                                                        // "<道具名>:<goalId または runSlug>" で対象を絞れる
//   }
// --calls <path> を渡すと、受け取った tools/call を {"name","arguments"} の JSONL として追記する。
// --processes <path> を渡すと、起動時に自分の pid を 1 行追記する。
import { appendFileSync, readFileSync, writeFileSync } from "node:fs";
import { createInterface } from "node:readline";
import { isAbsolute } from "node:path";

const argv = process.argv.slice(2);
const option = (name) => {
  const index = argv.indexOf(name);
  return index >= 0 ? argv[index + 1] : undefined;
};

const behavior = option("--behavior") ?? "ok";
const statePath = option("--state");
// 受け取った tools/call を JSONL で追記する。テストは書き込み道具へ渡った引数を
// ここから読み、backend が組み立てた引数を直接観測する。
const callsPath = option("--calls");
// 起動時に自分の pid を追記する。テストは読み取りと書き込みが同じ子プロセスを使うことを
// この記録で観測する。
const processesPath = option("--processes");
// 起動時の PID の書き出し先。子プロセスの終了をテストから観測するために使う。
const pidFile = option("--pid-file");
// 実 takt-mcp と同じページサイズ（limit 既定 50）。
const PAGE_SIZE = 50;
// takt の入力契約では goalId は UUID（z.uuid()）。
const UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

if (behavior === "exit") {
  process.exit(3);
}

if (processesPath !== undefined) {
  appendFileSync(processesPath, `${process.pid}\n`);
}

if (pidFile) {
  writeFileSync(pidFile, String(process.pid));
}

const loadState = () => {
  if (!statePath) {
    return {};
  }
  return JSON.parse(readFileSync(statePath, "utf8"));
};

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

// 実契約（schemas.ts の strict スキーマ）に対応する、道具ごとの必須キーと許容キー。
const TOOL_ARGUMENTS = {
  takt_list_goals: { required: ["cwd"], optional: ["offset", "limit", "view"] },
  takt_list_tasks: { required: ["cwd"], optional: ["offset", "limit", "view", "path"] },
  takt_list_goal_questions: { required: ["cwd", "goalId"], optional: ["offset", "limit"] },
  takt_list_standalone_inquiries: { required: ["cwd"], optional: ["offset", "limit"] },
  takt_get_run: { required: ["cwd", "runSlug"], optional: ["offset", "limit", "view", "stepIndex", "instructionId", "report", "maxBytes"] },
  takt_get_goal: { required: ["cwd", "goalId"], optional: ["path", "offset", "limit"] },
  takt_get_workflow: { required: ["cwd", "workflow"], optional: ["offset", "limit"] },
  // 書き込み系。実 takt-mcp（features/mcp/schemas.js ほか）の入力契約に合わせる。
  takt_answer_goal_question: { required: ["cwd", "goalId", "questionId", "answer"], optional: [] },
  takt_list_goal_registration_proposals: { required: ["cwd"], optional: [] },
  takt_approve_goal_registration: { required: ["cwd", "revision"], optional: [] },
  takt_reject_goal_registration: { required: ["cwd", "revision"], optional: [] },
  takt_merge_goal_pr: { required: ["cwd", "goalId", "prNumber", "until"], optional: [] },
};

// 引数の検証。実契約と同じく cwd 必須・絶対パス、未知キーは拒否する。
const validateArguments = (name, args) => {
  const schema = TOOL_ARGUMENTS[name];
  if (schema === undefined) {
    return `未知の道具です: ${name}`;
  }
  if (args === null || typeof args !== "object" || Array.isArray(args)) {
    return "arguments はオブジェクトである必要があります";
  }
  const cwd = args.cwd;
  if (typeof cwd !== "string" || !isAbsolute(cwd)) {
    return "cwd は絶対パスの文字列で指定してください";
  }
  const allowed = new Set([...schema.required, ...schema.optional]);
  for (const key of Object.keys(args)) {
    if (!allowed.has(key)) {
      return `未知の引数です: ${key}`;
    }
  }
  for (const key of schema.required) {
    if (args[key] === undefined || args[key] === null) {
      return `${key} は必須です`;
    }
  }
  // takt の入力契約では goalId は UUID（z.uuid()）。空文字や UUID 以外は拒否する。
  if (name === "takt_answer_goal_question" && !UUID_PATTERN.test(args.goalId)) {
    return "goalId は UUID で指定してください";
  }
  return undefined;
};

const asText = (payload) => ({ content: [{ type: "text", text: JSON.stringify(payload) }] });

const reply = (message) => {
  process.stdout.write(`${JSON.stringify(message)}\n`);
};

const handleServerNotifications = () => {
  reply({ jsonrpc: "2.0", method: "notifications/message", params: { level: "info", data: "起動しました" } });
};

// takt_get_run の応答。stepLogs を PAGE_SIZE 件ずつ窓切りし、stepLogPage.nextOffset を返す。
// report 引数と instructionId 引数が来たら、その本文だけを取り出して返す。
const runResponse = (state, args) => {
  const run = (state.runs ?? {})[args.runSlug];
  if (run === undefined) {
    return {};
  }
  if (args.report !== undefined) {
    const report = (run.reports ?? []).find((entry) => entry.filename === args.report);
    return report === undefined ? {} : { filename: report.filename, content: report.content ?? "", sizeBytes: report.sizeBytes ?? 0 };
  }
  if (args.instructionId !== undefined) {
    const instruction = (run.instructions ?? []).find((entry) => entry.instructionId === Number(args.instructionId));
    return instruction === undefined ? {} : instruction;
  }
  const stepLogs = run.stepLogs ?? [];
  const offset = Number(args.offset ?? 0);
  const window = stepLogs.slice(offset, offset + PAGE_SIZE);
  const nextOffset = offset + window.length < stepLogs.length ? offset + window.length : null;
  return {
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
  };
};

const handleToolCall = (name, args) => {
  const state = loadState();
  // 指定した道具の呼び出しで子プロセスが応答せずに終了する輸送エラーを再現する。
  if ((state.transportErrors ?? []).includes(name)) {
    process.exit(4);
  }
  const isError = new Set(state.isError ?? []);
  // 道具名単位の指定に加え、"<道具名>:<goalId または runSlug または workflow>" の限定キーで対象を絞れる。
  const scopedKey = `${name}:${args.goalId ?? args.runSlug ?? args.workflow ?? ""}`;
  if (isError.has(name) || isError.has(scopedKey)) {
    return { content: [{ type: "text", text: `${name} は失敗しました。` }], isError: true };
  }
  // 応答の差し替え。必須値の欠落を再現するために、書き込み道具の応答を丸ごと置き換える。
  if (state.responses !== undefined && Object.prototype.hasOwnProperty.call(state.responses, name)) {
    return asText(state.responses[name]);
  }
  switch (name) {
    case "takt_list_goals":
      return asText(paged(state.goals ?? [], "goals", args.offset));
    case "takt_list_tasks":
      return asText(paged(state.tasks ?? [], "tasks", args.offset));
    case "takt_list_goal_questions":
      return asText({ goalId: args.goalId, ...paged((state.goalQuestions ?? {})[args.goalId] ?? [], "questions", args.offset) });
    case "takt_list_standalone_inquiries":
      return asText(paged(state.standaloneInquiries ?? [], "inquiries", args.offset));
    case "takt_get_run":
      return asText(runResponse(state, args));
    case "takt_get_goal":
      if (args.path === "notifications") {
        return asText(paged((state.notifications ?? {})[args.goalId] ?? [], "items", args.offset));
      }
      return asText({});
    case "takt_get_workflow":
      return asText(
        paged(
          ((state.workflows ?? {})[args.workflow] ?? []).map((name) => ({ name })),
          "steps",
          args.offset,
        ),
      );
    case "takt_answer_goal_question":
      return asText({ goalId: args.goalId, questionId: args.questionId, answer: args.answer });
    case "takt_list_goal_registration_proposals":
      // 実 takt-mcp と同じく、提案は最大1件の配列で返す。無ければ空配列。
      // `proposalsByCwd` があれば、道具は cwd でプロジェクトを選ぶ契約に合わせて
      // その cwd の提案だけを返す。無ければ `proposals` をそのまま返す。
      if (state.proposalsByCwd !== undefined) {
        return asText({ proposals: state.proposalsByCwd[args.cwd] ?? [] });
      }
      return asText({ proposals: state.proposals ?? [] });
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

// 受け取った道具呼び出しを記録する。記録は検証の合否に関与しない。
const recordCall = (name, args) => {
  if (callsPath === undefined) {
    return;
  }
  appendFileSync(callsPath, `${JSON.stringify({ name, arguments: args })}\n`);
};

const onLine = (line) => {
  let message;
  try {
    message = JSON.parse(line);
  } catch {
    process.stdout.write("これは JSON ではありません\n");
    return;
  }
  if (message.method === "initialize") {
    reply({
      jsonrpc: "2.0",
      id: message.id,
      result: {
        protocolVersion: message.params?.protocolVersion ?? "2025-06-18",
        capabilities: { tools: {} },
        serverInfo: { name: "taktdash-mock", version: "0.0.0" },
      },
    });
    handleServerNotifications();
    return;
  }
  if (message.method === "notifications/initialized") {
    return;
  }
  if (message.method === "tools/call") {
    if (behavior === "hang") {
      return;
    }
    if (behavior === "garbage") {
      process.stdout.write("これは JSON ではありません\n");
      return;
    }
    if (behavior === "drip") {
      // tools/call に対して、全体の期限より十分に長い間、途中経過だけを細切れに返し続ける。
      // 行ごとのタイムアウトは満たすが、呼び出し全体は終わらないサーバーを模す。
      const drip = (remaining) => {
        if (remaining <= 0) {
          return;
        }
        process.stdout.write(JSON.stringify({ jsonrpc: "2.0", method: "notifications/progress", params: { progress: 1 } }) + "\n");
        setTimeout(() => drip(remaining - 1), 100);
      };
      drip(100);
      return;
    }
    const { name, arguments: args } = message.params ?? {};
    recordCall(name, args);
    const invalid = validateArguments(name, args);
    if (invalid !== undefined) {
      reply({ jsonrpc: "2.0", id: message.id, error: { code: -32602, message: invalid } });
      return;
    }
    reply({ jsonrpc: "2.0", id: message.id, result: handleToolCall(name, args) });
    return;
  }
  if (message.id !== undefined) {
    reply({ jsonrpc: "2.0", id: message.id, error: { code: -32601, message: `未知のメソッドです: ${message.method}` } });
  }
};

const reader = createInterface({ input: process.stdin, crlfDelay: Infinity });
reader.on("line", onLine);
reader.on("close", () => process.exit(0));

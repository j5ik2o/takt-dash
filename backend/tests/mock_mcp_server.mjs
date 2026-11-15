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
//     "notifications": { "<goalId>": [ {"id","kind","body","recordedAt"} ] },
//     "goalDetails": {
//       "<goalId>": {
//         "summary": {"id","status","executionStatus","branch","integrationBranch","omissions"},
//         "objective": "長文",
//         "outOfScope": ["..."],
//         "acceptanceCriteria": ["..."],
//         "constraints": ["..."],
//         "specifications": { "<notation>": "長文" },
//         "workLog": [ {"id","recordedAt","summary"} ],
//         "questions": [ {"id","kind","status","body","options","answer","createdAt"} ],
//         "omitFields": ["outOfScope"],  // 成功応答から content/items を省く path（応答の省略を再現する）
//         "malformedFields": { "objective": { "content": 42 } }  // path ごとの生の応答（型不正を再現する）
//       }
//     },
//   実 takt-mcp と同じく、存在しない path は
//   "Goal read failed: Selected field does not exist" の道具エラーで返す。
//     "isError": ["takt_list_tasks", "takt_get_run:r1"]  // 指定した道具を isError で失敗させる。
//                                                        // "<道具名>:<goalId または runSlug>" で対象を絞れる
//   }
import { readFileSync } from "node:fs";
import { createInterface } from "node:readline";
import { isAbsolute } from "node:path";

const argv = process.argv.slice(2);
const option = (name) => {
  const index = argv.indexOf(name);
  return index >= 0 ? argv[index + 1] : undefined;
};

const behavior = option("--behavior") ?? "ok";
const statePath = option("--state");
// 実 takt-mcp と同じページサイズ（limit 既定 50）。
const PAGE_SIZE = 50;

if (behavior === "exit") {
  process.exit(3);
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

// takt_get_goal の textOffset / maxBytes によるテキストページ。実契約の既定は 4096 バイト。
// 実 takt-mcp の sliceUtf8（forward）と同じく、開始を継続バイトの後ろへ進め、
// 終端を UTF-8 の文字境界へ戻す。文字の途中を nextOffset にしない。
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

// 実 takt-mcp の項目不在（selectGoalValue の throw）と同じ道具エラー。
const goalFieldMissing = () => ({
  content: [{ type: "text", text: "Goal read failed: Selected field does not exist" }],
  isError: true,
});

// 実契約（schemas.ts の strict スキーマ）に対応する、道具ごとの必須キーと許容キー。
const TOOL_ARGUMENTS = {
  takt_list_goals: { required: ["cwd"], optional: ["offset", "limit", "view"] },
  takt_list_tasks: { required: ["cwd"], optional: ["offset", "limit", "view", "path"] },
  takt_list_goal_questions: { required: ["cwd", "goalId"], optional: ["offset", "limit"] },
  takt_list_goal_work_log: { required: ["cwd", "goalId"], optional: ["offset", "limit"] },
  takt_list_standalone_inquiries: { required: ["cwd"], optional: ["offset", "limit"] },
  takt_get_run: { required: ["cwd", "runSlug"], optional: ["offset", "limit", "view", "stepIndex", "instructionId", "report", "maxBytes"] },
  takt_get_goal: { required: ["cwd", "goalId"], optional: ["path", "offset", "limit", "recordId", "textOffset", "maxBytes"] },
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
  const isError = new Set(state.isError ?? []);
  // 道具名単位の指定に加え、"<道具名>:<goalId または runSlug>" の限定キーで対象を絞れる。
  const scopedKey = `${name}:${args.goalId ?? args.runSlug ?? ""}`;
  if (isError.has(name) || isError.has(scopedKey)) {
    return { content: [{ type: "text", text: `${name} は失敗しました。` }], isError: true };
  }
  switch (name) {
    case "takt_list_goals":
      return asText(paged(state.goals ?? [], "goals", args.offset));
    case "takt_list_tasks":
      return asText(paged(state.tasks ?? [], "tasks", args.offset));
    case "takt_list_goal_questions": {
      const questions = (state.goalDetails ?? {})[args.goalId]?.questions ?? (state.goalQuestions ?? {})[args.goalId] ?? [];
      return asText({ goalId: args.goalId, ...paged(questions, "questions", args.offset) });
    }
    case "takt_list_goal_work_log": {
      const workLog = (state.goalDetails ?? {})[args.goalId]?.workLog ?? [];
      return asText(pagedBackward(workLog, "workLog", args.offset));
    }
    case "takt_list_standalone_inquiries":
      return asText(paged(state.standaloneInquiries ?? [], "inquiries", args.offset));
    case "takt_get_run":
      return asText(runResponse(state, args));
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
    default:
      return { content: [{ type: "text", text: `未知の道具です: ${name}` }], isError: true };
  }
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
    const { name, arguments: args } = message.params ?? {};
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

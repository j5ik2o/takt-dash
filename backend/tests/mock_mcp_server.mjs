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
//       reports は [ {"filename","sizeBytes","content"} ]。content は view "report" の本文になる。
//     "workflows": { "<workflow名>": ["plan", "implement", ...] },
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
//     "proposals": [ {"revision","repositoryDirectory","summary","conversationId"?} ],
//   conversationId を省いた提案には、既定の持ち主（manager-aaaa…）を付けて返す。
//     "proposalsByCwd": { "<cwd>": [ {"revision","repositoryDirectory","summary"} ] },
//     "responses": { "<道具名>": {...} },  // 指定した書き込み道具の応答を丸ごと差し替える
//     "toolErrors": { "<道具名>": "<文言>" },  // 指定した道具を、その文言の isError で失敗させる
//     "isError": ["takt_list_tasks", "takt_get_run:r1"]  // 指定した道具を isError で失敗させる。
//                                                        // "<道具名>:<goalId または runSlug>" で対象を絞れる
//     "hang": ["takt_list_goal_questions:g1", "takt_get_run:r1@120"], // 指定した呼び出しだけ応答しない。
//     "delayMs": { "takt_list_goals": 1000 },            // 道具ごとに応答を遅らせる。
//     "delays": { "takt_list_goals": 1500 },  // 指定した道具の応答を指定ミリ秒だけ遅らせる
//     "hangOnce": ["takt_list_goals"]  // 指定した道具は最初の子プロセスだけ応答しない（マーカーが残る）
//   }
// --calls <path> を渡すと、受け取った tools/call を {"name","arguments"} の JSONL として追記する。
// --processes <path> を渡すと、起動時に自分の pid を 1 行追記する。
import { appendFileSync, existsSync, readFileSync, writeFileSync } from "node:fs";
import { createInterface } from "node:readline";
import { isAbsolute } from "node:path";

const argv = process.argv.slice(2);
const option = (name) => {
  const index = argv.indexOf(name);
  return index >= 0 ? argv[index + 1] : undefined;
};

const behavior = option("--behavior") ?? "ok";
const statePath = option("--state");
const callLogPath = option("--call-log");
// 受け取った tools/call を JSONL で追記する。テストは書き込み道具へ渡った引数を
// ここから読み、backend が組み立てた引数を直接観測する。
const callsPath = option("--calls");
// 起動時に自分の pid を追記する。テストは読み取りと書き込みが同じ子プロセスを使うことを
// この記録で観測する。
const processesPath = option("--processes");
// 起動時の PID の書き出し先。子プロセスの終了をテストから観測するために使う。
const pidFile = option("--pid-file");
// initialize の応答を遅らせる時間（ミリ秒）。高負荷で起動と初期化が遅いサーバーを模す。
const initializeDelayMs = Number(option("--initialize-delay-ms") ?? 0);
// 起動と初期化の記録先。起動時に start、initialize の応答直前に ready を、
// pid とミリ秒の時刻とともに追記する。テストは起動が重なる本数をこの記録で観測する。
const spawnLogPath = option("--spawn-log");
// 実 takt-mcp と同じページサイズ（limit 既定 50）。
const PAGE_SIZE = 50;
// takt の入力契約では goalId は UUID（z.uuid()）。
const UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

if (spawnLogPath !== undefined) {
  appendFileSync(spawnLogPath, `start ${process.pid} ${Date.now()}\n`);
}

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
  takt_get_workflow: { required: ["cwd", "workflow"], optional: ["offset", "limit"] },
  // 書き込み系。実 takt-mcp（features/mcp/schemas.js ほか）の入力契約に合わせる。
  takt_answer_goal_question: { required: ["cwd", "goalId", "questionId", "answer"], optional: [] },
  takt_list_goal_registration_proposals: { required: ["cwd"], optional: [] },
  takt_get_goal_registration_proposal: {
    required: ["cwd", "conversationId"],
    optional: ["path", "limit", "offset", "textOffset", "maxBytes"],
  },
  // conversationId の必須と形は、実 takt-mcp の文言で返すため registrationOwnerError が確かめる。
  takt_approve_goal_registration: { required: ["cwd", "revision"], optional: ["conversationId"] },
  takt_reject_goal_registration: { required: ["cwd", "revision"], optional: ["conversationId"] },
  takt_merge_goal_pr: { required: ["cwd", "goalId", "prNumber", "until"], optional: [] },
  // manager 専用の終了依頼。実機の takt-mcp の入力契約（reason 必須）は、窓口が takt-custom の
  // dist の実物で確認済みです。backend が投函する {cwd, goalId, reason} を受け付けます。
  takt_finish_goal: { required: ["cwd", "goalId", "reason"], optional: [] },
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

// 実 takt-mcp の対象レポート不在（runSessionReader の throw）と同じ道具エラー。
const runReportMissing = () => ({
  content: [{ type: "text", text: "Selected report does not exist" }],
  isError: true,
});

// 実 takt-mcp の対象指示書不在（runSessionReader の throw）と同じ道具エラー。
const runInstructionMissing = () => ({
  content: [{ type: "text", text: "Selected instruction does not exist" }],
  isError: true,
});

// takt_get_run の view "report" の応答。実 takt-mcp（runSessionReader の loadReports）と同じく、
// reports[].content の UTF-8 バイト列を offset と maxBytes（既定 4096）で切り出し、
// { runSlug, report: { filename, content, totalBytes, offset, returnedBytes, direction, truncated, nextOffset } }
// を返す。対象のレポートが無ければ道具エラーにする。
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

// takt_get_run の view "instructions" の応答。実 takt-mcp（runSessionReader）と同じく、
// { runSlug, instructions: [{instructionId, issuedAt, state}], collection, totalCount,
// returnedCount, omittedCount, offset, nextOffset, truncated } を返す。limit の既定は 50。
// instructionId の指定がある場合は一覧ではなく、その指示書の本文を
// { runSlug, instruction: { instructionId, issuedAt, state, content, totalBytes, offset,
// returnedBytes, direction, truncated, nextOffset } } で返し、対象が無ければ道具エラーにする。
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
    return {};
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

// takt_get_run の view "task" の応答。実 takt-mcp（runSessionReader）と同じく、
// { runSlug, task: { content, totalBytes, offset, returnedBytes, direction, truncated,
// nextOffset } } を返す。本文は state の taskText を使う。
const runTaskResponse = (state, args) => {
  const run = (state.runs ?? {})[args.runSlug];
  if (run === undefined) {
    return {};
  }
  return asText({ runSlug: args.runSlug, task: textPage(run.taskText ?? "", args.offset, args.maxBytes) });
};

// takt_get_run の応答。stepLogs を PAGE_SIZE 件ずつ窓切りし、stepLogPage.nextOffset を返す。
// instructionId 引数が来たら、その本文だけを取り出して返す。
const runResponse = (state, args) => {
  const run = (state.runs ?? {})[args.runSlug];
  if (run === undefined) {
    return {};
  }
  if (args.instructionId !== undefined) {
    const instruction = (run.instructions ?? []).find((entry) => entry.instructionId === Number(args.instructionId));
    return instruction === undefined ? {} : instruction;
  }
  const stepLogs = run.stepLogs ?? [];
  const offset = Number(args.offset ?? 0);
  const window = stepLogs.slice(offset, offset + PAGE_SIZE);
  const nextOffset = offset + window.length < stepLogs.length ? offset + window.length : null;
  // 実 takt-mcp の要約は、指示書の一覧とタスク本文を含まない（どちらも view で読む）。
  const { instructions: _instructions, taskText: _taskText, ...summary } = run;
  return {
    ...summary,
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

const scopeOf = (args) => args.goalId ?? args.runSlug;

// "hang" の項目は "<道具名>"、"<道具名>:<goalId または runSlug>"、
// "<道具名>:<goalId または runSlug>@<offset>" の順で照合する。
const isHung = (state, name, args) => {
  const hang = state.hang ?? [];
  if (hang.includes(name)) {
    return true;
  }
  const scope = scopeOf(args);
  if (scope === undefined) {
    return false;
  }
  if (hang.includes(`${name}:${scope}`)) {
    return true;
  }
  return args.offset !== undefined && hang.includes(`${name}:${scope}@${Number(args.offset)}`);
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

// takt_get_goal_registration_proposal の応答（path なしは登録案の全体、path ありは項目のページ）。持ち主でない会話と、保留中の登録案が
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
  const { summary, ...identity } = pending[0];
  if (args.path === undefined) {
    return asText(boundedResponse({ proposal: pending[0] }));
  }
  // path は summary.<項目> と summary.<一覧>.<番号>。実 takt-mcp の selectGoalValue と同じく、
  // 無い項目は道具エラーにする。
  let value = { summary };
  for (const part of args.path.split(".")) {
    if (value === null || typeof value !== "object" || !Object.hasOwn(value, part)) {
      return fail("Selected field does not exist");
    }
    value = value[part];
  }
  const extra = { ...identity, path: args.path };
  if (Array.isArray(value)) {
    const offset = Number(args.offset ?? 0);
    const limit = Number(args.limit ?? PAGE_SIZE);
    const page = value.slice(offset, offset + limit);
    const nextOffset = offset + page.length < value.length ? offset + page.length : null;
    return asText(
      boundedResponse({
        ...extra,
        items: page,
        collection: "items",
        totalCount: value.length,
        returnedCount: page.length,
        omittedCount: value.length - page.length,
        offset,
        nextOffset,
        truncated: nextOffset !== null,
      }),
    );
  }
  if (typeof value === "string") {
    return asText({ ...extra, ...textPage(value, args.textOffset, args.maxBytes) });
  }
  return asText({ ...extra, value });
};

// 実 takt-mcp の jsonResult と同じく、応答が 64 KiB に収まらなければ、長い文字列を先頭 4096
// バイトの文字境界までにし、truncated と omissions（番号は [] にまとめる）を足す。
const RESPONSE_MAX_BYTES = 64 * 1024;
const boundedResponse = (payload) => {
  if (Buffer.byteLength(JSON.stringify(payload)) <= RESPONSE_MAX_BYTES) {
    return payload;
  }
  const omissions = new Map();
  const shorten = (entry, path) => {
    if (typeof entry === "string") {
      const page = textPage(entry, 0, 4096);
      if (page.returnedBytes < page.totalBytes) {
        const key = path.replace(/\.\d+(?=\.|$)/g, "[]");
        const previous = omissions.get(key);
        omissions.set(key, {
          path: key,
          totalBytes: (previous?.totalBytes ?? 0) + page.totalBytes,
          returnedBytes: (previous?.returnedBytes ?? 0) + page.returnedBytes,
        });
      }
      return page.content;
    }
    if (Array.isArray(entry)) {
      return entry.map((item, index) => shorten(item, `${path}.${index}`));
    }
    if (entry !== null && typeof entry === "object") {
      return Object.fromEntries(
        Object.entries(entry).map(([key, item]) => [key, shorten(item, path ? `${path}.${key}` : key)]),
      );
    }
    return entry;
  };
  return { ...shorten(payload, ""), truncated: true, omissions: [...omissions.values()] };
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

const pendingProposals = (state, cwd) =>
  withOwner(state.proposalsByCwd !== undefined ? (state.proposalsByCwd[cwd] ?? []) : (state.proposals ?? []));

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
  // 道具エラーの文言の指定。実 takt-mcp が返す文言（stale_summary など）を再現する。
  if (state.toolErrors !== undefined && Object.prototype.hasOwnProperty.call(state.toolErrors, name)) {
    return { content: [{ type: "text", text: state.toolErrors[name] }], isError: true };
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
      if (args.view === "report") {
        return runReportResponse(state, args);
      }
      if (args.view === "instructions") {
        return runInstructionsResponse(state, args);
      }
      if (args.view === "task") {
        return runTaskResponse(state, args);
      }
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
      return asText({ proposals: pendingProposals(state, args.cwd).map(listedProposal) });
    case "takt_get_goal_registration_proposal":
      return readProposal(args, pendingProposals(state, args.cwd));
    case "takt_approve_goal_registration":
      return (
        registrationOwnerError(name, args, pendingProposals(state, args.cwd)) ??
        asText({ revision: args.revision, goal: { id: `approved-${args.revision}` } })
      );
    case "takt_reject_goal_registration":
      return (
        registrationOwnerError(name, args, pendingProposals(state, args.cwd)) ??
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
    case "takt_finish_goal":
      // 依頼を受け付けたことを表す最小の応答。実 takt-mcp の応答形は未確認。
      return asText({ goalId: args.goalId, status: "finished" });
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

const onLine = async (line) => {
  let message;
  try {
    message = JSON.parse(line);
  } catch {
    process.stdout.write("これは JSON ではありません\n");
    return;
  }
  if (message.method === "initialize") {
    const respond = () => {
      if (spawnLogPath !== undefined) {
        appendFileSync(spawnLogPath, `ready ${process.pid} ${Date.now()}\n`);
      }
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
    };
    if (initializeDelayMs > 0) {
      setTimeout(respond, initializeDelayMs);
    } else {
      respond();
    }
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
    if (callLogPath) {
      appendFileSync(callLogPath, `${name}\n`);
    }
    recordCall(name, args);
    const invalid = validateArguments(name, args);
    if (invalid !== undefined) {
      reply({ jsonrpc: "2.0", id: message.id, error: { code: -32602, message: invalid } });
      return;
    }
    const state = loadState();
    if (isHung(state, name, args)) {
      return;
    }
    const delayMs = state.delayMs?.[name] ?? 0;
    if (delayMs > 0) {
      await new Promise((resolve) => setTimeout(resolve, delayMs));
    }
    // hangOnce に指定した道具は、最初の子プロセスだけが応答しない。マーカーが残るため、
    // 作り直した子プロセス（再試行側）は応答する。「1回だけ時間切れし、その後は回復する」
    // サーバーを模す。
    if (statePath !== undefined && (state.hangOnce ?? []).includes(name)) {
      const marker = `${statePath}.hang-once.${name}`;
      if (!existsSync(marker)) {
        writeFileSync(marker, String(process.pid));
        return;
      }
    }
    // delays に指定した道具は、応答を指定ミリ秒だけ遅らせる。期限より遅い応答を模す。
    const replyDelayMs = Number((state.delays ?? {})[name] ?? 0);
    if (replyDelayMs > 0) {
      setTimeout(() => reply({ jsonrpc: "2.0", id: message.id, result: handleToolCall(name, args) }), replyDelayMs);
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
// 応答の遅延中も受信順を保つため、1 行ずつ直列に処理する。
let queue = Promise.resolve();
reader.on("line", (line) => {
  queue = queue.then(() => onLine(line)).catch(() => process.exit(3));
});
reader.on("close", () => process.exit(0));

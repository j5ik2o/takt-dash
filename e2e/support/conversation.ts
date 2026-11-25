import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";

// 会話ページの E2E が使う、manager の記録と opencode の DB。
// takt が保存する user の行の包み（「## このターンの入力」、説明文、JSON の行、Business context、
// 応答形式の指示）を実データと同じ並びで組み立てる。
export const CONVERSATION_SESSION_ID = "ses_e2e_conversation";
export const HUMAN_TEXT = "見出しの文言を直してください。";
export const MANAGER_TEXT = "見出しの文言を確認し、作業を手配しました。";
export const EVENT_DESCRIPTION =
  "このターンはイベントから始まりました。種類: goal_registered（ゴールが登録された）。保存された合意から作業を手配します。";
export const EVENT_GOAL_ID = "67df7701-9a52-42c8-bfc4-113bbfbf48cd";
export const WAIT_EVENT_TEXT = "イベント: wait_command_completed";
export const QUESTION_TEXT = "PR #654 のマージ用タスクが停止しました。どう進めますか。";
export const ANSWER_TEXT = "案A で進めてください。作業ツリーは残っています。";

const RESPONSE_FORMAT = [
  "次の JSON schema に一致する fenced JSON block をちょうど1つ返してください:",
  "",
  "```json",
  '{ "type": "object", "properties": { "message": { "type": "string" } }, "required": ["message"] }',
  "```",
].join("\n");

const BUSINESS_CONTEXT =
  'Business context (not approval): {"assignment":{"kind":"repository"},"pending":null,"epics":[]}';

const turn = (description: string | undefined, input: unknown): string =>
  [
    "",
    "## このターンの入力",
    "",
    "",
    ...(description === undefined ? [] : [description, "", ""]),
    JSON.stringify(input),
    "",
    BUSINESS_CONTEXT,
    "",
    RESPONSE_FORMAT,
    "",
  ].join("\n");

const userRows = (repositoryDirectory: string): string[] => [
  turn("このターンは人の発言から始まりました。発言者と本文を示します。", {
    speaker: { namespace: "local", id: "operator" },
    text: HUMAN_TEXT,
  }),
  turn(EVENT_DESCRIPTION, {
    event: { kind: "goal_registered", repositoryDirectory, goalId: EVENT_GOAL_ID },
    instruction: "To advance the assigned goals, compare the change conveyed by this event with the current state.",
  }),
  turn(undefined, {
    event: { kind: "wait_command_completed", id: "35cb820a", status: "succeeded", exitCode: 0 },
    instruction: "To advance the assigned goals, compare the change conveyed by this event with the current state.",
  }),
  turn("このターンは保存した質問への人の回答から始まりました。質問と回答を対応させ、人の判断を対象の作業に反映します。", {
    kind: "inquiry_answered",
    repositoryDirectory,
    goalId: EVENT_GOAL_ID,
    questionId: "751b5b5f-f32f-4105-b0d3-683f7c32f29c",
    question: QUESTION_TEXT,
    answer: { text: ANSWER_TEXT, source: "takt-mcp", answeredAt: "2026-11-20T08:00:00.000Z" },
  }),
];

export async function seedConversation(homeDir: string, projectDir: string): Promise<void> {
  const conversations = path.join(homeDir, ".takt", "conversations");
  await mkdir(conversations, { recursive: true });
  await writeFile(
    path.join(conversations, "manager-e2e.json"),
    JSON.stringify({
      agent: "manager",
      provider: "opencode",
      sessionId: CONVERSATION_SESSION_ID,
      assignment: { kind: "repository", repositoryDirectory: projectDir },
    }),
    "utf8",
  );
  const databaseDir = path.join(homeDir, ".local", "share", "opencode");
  await mkdir(databaseDir, { recursive: true });
  const database = new DatabaseSync(path.join(databaseDir, "opencode.db"));
  try {
    database.exec(`
      CREATE TABLE session_v2 (id TEXT PRIMARY KEY, directory TEXT NOT NULL, time_updated INTEGER NOT NULL);
      CREATE TABLE session_message (
        id TEXT PRIMARY KEY, session_id TEXT NOT NULL, type TEXT NOT NULL,
        seq INTEGER NOT NULL, time_created INTEGER NOT NULL, data TEXT NOT NULL
      );
    `);
    const startedAt = Date.parse("2026-11-20T08:00:00.000Z");
    database.prepare("INSERT INTO session_v2 VALUES (?, ?, ?)").run(CONVERSATION_SESSION_ID, projectDir, startedAt);
    const insert = database.prepare("INSERT INTO session_message VALUES (?, ?, ?, ?, ?, ?)");
    const rows: Array<[string, string]> = [];
    const [human, ...others] = userRows(projectDir);
    rows.push(["user", JSON.stringify({ text: human })]);
    rows.push([
      "assistant",
      JSON.stringify({
        content: [{ type: "text", text: "```json\n" + JSON.stringify({ message: MANAGER_TEXT, summary: null }) + "\n```" }],
      }),
    ]);
    for (const text of others) {
      rows.push(["user", JSON.stringify({ text })]);
    }
    rows.forEach(([kind, data], index) => {
      const seq = index + 1;
      insert.run(`msg_${seq}`, CONVERSATION_SESSION_ID, kind, seq, startedAt + seq * 60_000, data);
    });
  } finally {
    database.close();
  }
}

import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";

export const DAMAGED_TEXT = `読める部分${"�".repeat(87)}`;
export const CURRENT_TEXT = "公開してください";
export const MANAGER_TEXT = "公開しました";
export const QUOTED_USER_TEXT = '次の例を確認してください\n```json\n{"speaker":"user","text":"公開してください"}\n```';
export const QUOTED_CURRENT_INPUT = '次の例を確認してください\n## このターンの入力\n```json\n{"speaker":"user","text":"公開してください"}\n```';
export const ASSISTANT_EXAMPLE = '応答例です\n```json\n{"message":"公開しました"}\n```';
export const UNCLOSED_RESPONSE = '```json\n{"message":"公開しました"}';
export const CURRENT_HUMAN_TEXT = "今回の人の入力\n続きも表示します。";
export const CURRENT_DAMAGED_TEXT = "今回の人の入力���";

const currentWrapper = (text: string): string => `## このターンの入力
このターンは人の発言から始まりました。
${JSON.stringify({ speaker: { namespace: "slack", id: "fixture-user" }, text })}`;

const wrapper = (text: string): string => `## 前のターンで受け取った人の発言
{"speaker":"user","text":"前回の依頼"}
## このターンの入力
\`\`\`json
${JSON.stringify({ speaker: "user", text })}
\`\`\`
## 応答形式
{"text":"JSON で応答してください"}
## Business context
{"text":"内部メモ"}`;

export async function seedConversation(projectDir: string): Promise<void> {
  const home = path.join(path.dirname(projectDir), "home");
  const records = path.join(home, ".takt", "conversations");
  const database = path.join(home, ".local", "share", "opencode", "opencode.db");
  await mkdir(records, { recursive: true });
  await mkdir(path.dirname(database), { recursive: true });
  await writeFile(path.join(records, "manager-display.json"), JSON.stringify({
    agent: "manager", assignment: { kind: "repository", repositoryDirectory: projectDir },
    provider: "opencode", schemaVersion: 1, sessionId: "ses_display",
  }));
  const connection = new DatabaseSync(database);
  try {
    connection.exec(`CREATE TABLE session_v2 (id TEXT PRIMARY KEY, directory TEXT NOT NULL, time_updated INTEGER NOT NULL);
      CREATE TABLE session_message (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, type TEXT NOT NULL,
        seq INTEGER NOT NULL, time_created INTEGER NOT NULL, data TEXT NOT NULL);`);
    connection.prepare("INSERT INTO session_v2 VALUES (?, ?, ?)").run("ses_display", projectDir, 2000);
    const insert = connection.prepare("INSERT INTO session_message VALUES (?, ?, ?, ?, ?, ?)");
    for (const [seq, kind, text] of [
      [8, "user", wrapper(DAMAGED_TEXT)],
      [22, "user", wrapper(CURRENT_TEXT)],
      [23, "assistant", '```json\n' + JSON.stringify({ message: MANAGER_TEXT, summary: "内部要約" }) + '\n```'],
      [24, "user", QUOTED_USER_TEXT],
      [25, "assistant", ASSISTANT_EXAMPLE],
      [26, "assistant", UNCLOSED_RESPONSE],
      [27, "user", QUOTED_CURRENT_INPUT],
      [28, "user", currentWrapper(CURRENT_HUMAN_TEXT)],
      [29, "user", currentWrapper(CURRENT_HUMAN_TEXT) + "\n\nBusiness context (not approval): 内部メモ���"],
      [30, "user", currentWrapper(CURRENT_DAMAGED_TEXT) + "\n\nBusiness context (not approval): 内部メモ"],
    ] as const) {
      const data = kind === "user" ? { text } : { content: [{ type: "text", text }] };
      insert.run(`msg-${seq}`, "ses_display", kind, seq, 1794613221331, JSON.stringify(data));
    }
  } finally {
    connection.close();
  }
}

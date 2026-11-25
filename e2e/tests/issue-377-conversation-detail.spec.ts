import { expect, test } from "../support/test";
import {
  ANSWER_TEXT,
  EVENT_DESCRIPTION,
  EVENT_GOAL_ID,
  HUMAN_TEXT,
  MANAGER_TEXT,
  QUESTION_TEXT,
  WAIT_EVENT_TEXT,
} from "../support/conversation";

// issue #377: 人の発言ではないターン（出来事、保存した質問への回答）の包みを全文で出さない。
// 出来事は話者「システム」で説明文を、質問への回答は話者「ユーザー」で回答だけを出し、
// 元の入力は折りたたみを開くと読める。応答形式の指示と Business context はどこにも出さない。
test("出来事と質問への回答のターンは要点だけを出し、元の入力は折りたたみで開閉できる", async ({
  page,
  conversationStack: stack,
}, testInfo) => {
  await page.goto(`${stack.webUrl}/conversations`);
  await expect(page.getByRole("heading", { name: "会話", level: 1 })).toBeVisible();
  const messages = page.locator("li.conversation-message");
  await expect(messages).toHaveCount(5);

  const [human, manager, event, wait, answer] = [0, 1, 2, 3, 4].map((index) => messages.nth(index));
  await expect(human.locator(".conversation-speaker")).toHaveText("ユーザー");
  await expect(human.locator(".conversation-text")).toHaveText(HUMAN_TEXT);
  await expect(manager.locator(".conversation-text")).toHaveText(MANAGER_TEXT);
  await expect(human.locator("details")).toHaveCount(0);
  await expect(manager.locator("details")).toHaveCount(0);

  await expect(event.locator(".conversation-speaker")).toHaveText("システム");
  await expect(event.locator(".conversation-text")).toHaveText(EVENT_DESCRIPTION);
  await expect(wait.locator(".conversation-speaker")).toHaveText("システム");
  await expect(wait.locator(".conversation-text")).toHaveText(WAIT_EVENT_TEXT);
  await expect(answer.locator(".conversation-speaker")).toHaveText("ユーザー");
  await expect(answer.locator(".conversation-text").first()).toHaveText(ANSWER_TEXT);

  const list = page.locator("ol.conversation");
  await expect(list).not.toContainText("## このターンの入力");
  await expect(list).not.toContainText("JSON schema");
  await expect(list).not.toContainText("Business context");

  const eventJson = event.locator("details pre");
  const question = answer.getByText(QUESTION_TEXT, { exact: true });
  await expect(eventJson).toBeHidden();
  await expect(question).toBeHidden();
  await page.screenshot({ path: testInfo.outputPath("conversation-closed.png"), fullPage: true });

  await event.getByText("イベントの JSON を表示します。").click();
  await expect(eventJson).toBeVisible();
  await expect(eventJson).toContainText(EVENT_GOAL_ID);
  await expect(eventJson).not.toContainText("instruction");
  await answer.getByText("質問と元の JSON を表示します。").click();
  await expect(question).toBeVisible();
  await expect(answer.locator("details pre")).toContainText("inquiry_answered");
  await expect(list).not.toContainText("Business context");
  await page.screenshot({ path: testInfo.outputPath("conversation-open.png"), fullPage: true });

  await event.getByText("イベントの JSON を表示します。").click();
  await answer.getByText("質問と元の JSON を表示します。").click();
  await expect(eventJson).toBeHidden();
  await expect(question).toBeHidden();
});

// E2E の各 spec が使う fixture。実 backend が takt-mcp モックから読み、画面へ射影する。
//
// ゴールの objective と実行中タスクの name は同じ識別文字列にし、概要のタスク表示
// （overview-content.tsx の `タスク「...」`）と、ゴール状態由来の「人のマージ待ち」件数の
// 両方を同じデータから観測できるようにする。マージ待ちは mergeTask のタスクだけを見るため、
// 実行中タスクの mergeTask は false にする（backend/src/store.rs の is_human_merge_wait）。
// ゴール所属の未回答問い合わせは、自由記述の質問として判断待ちページの入力経路を通す。
export const SERVER_ID = "e2e-mac-mini";
export const SERVER_NAME = "E2E Mac mini";
export const PROJECT_NAME = "e2e-corporate-site";
export const PROJECT_ABOUT = "E2E 用のプロジェクト";
export const GOAL_TEXT = "E2E 概要表示のゴール";
export const GOAL_ID = "11111111-1111-4111-8111-111111111111";
export const RUN_SLUG = "e2e-run-overview";
export const PULL_REQUEST_NUMBER = 1;
export const PULL_REQUEST_URL = "https://example.com/e2e/pull/1";
export const QUESTION_ID = "e2e-question-1";
export const QUESTION_BODY = "E2E 判断待ちの質問の本文";

export function buildMockState(now: Date): Record<string, unknown> {
  const at = now.toISOString();
  return {
    goals: [
      {
        id: GOAL_ID,
        status: "awaiting_merge",
        executionStatus: "active",
        objective: GOAL_TEXT,
        pullRequest: { number: PULL_REQUEST_NUMBER, url: PULL_REQUEST_URL },
      },
    ],
    tasks: [
      {
        name: GOAL_TEXT,
        goalId: GOAL_ID,
        status: "running",
        startedAt: at,
        runSlug: RUN_SLUG,
        currentStep: "implement",
        mergeTask: false,
      },
    ],
    goalQuestions: {
      [GOAL_ID]: [
        {
          id: QUESTION_ID,
          status: "pending",
          kind: "question",
          body: QUESTION_BODY,
          options: [],
          createdAt: at,
        },
      ],
    },
    runs: {
      [RUN_SLUG]: {
        runSlug: RUN_SLUG,
        currentStep: "implement",
        startedAt: at,
        currentStepStartedAt: at,
        stepLogs: [],
        sessionId: "e2e-session",
        summary: "",
        workflow: "e2e",
        status: "running",
        lastLogAt: at,
      },
    },
  };
}

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

// 実行中の run の本体ログのファイル名。run のディレクトリの logs/ の下に置く。
export const RUN_BODY_LOG = "e2e-run-overview.jsonl";

// 実行中の run（RUN_SLUG）を、takt がプロジェクトの .takt へ書く保存形式で表したもの。
// backend は実行中のタスクの run を tasks.yaml の対応表、meta.json、本体ログから組み立てるため、
// takt-mcp モックの状態（buildMockState）と同じ run をファイルでも用意する。
export type RunFiles = {
  // `<project>/.takt/tasks.yaml` の本文。
  tasksYaml: string;
  // `<project>/.takt/runs/<RUN_SLUG>/meta.json` の本文。
  meta: Record<string, unknown>;
  // `<project>/.takt/runs/<RUN_SLUG>/logs/<RUN_BODY_LOG>` の最初のレコード。
  bodyLogRecords: Record<string, unknown>[];
};

export function buildRunFiles(now: Date): RunFiles {
  const at = now.toISOString();
  return {
    tasksYaml: `tasks:\n  - name: ${GOAL_TEXT}\n    run_slug: ${RUN_SLUG}\n`,
    meta: {
      workflow: "e2e",
      status: "running",
      currentStep: "implement",
      currentIteration: 1,
      phase: 1,
      startTime: at,
      updatedAt: at,
      task: GOAL_TEXT,
    },
    bodyLogRecords: [
      { type: "workflow_start", workflow: "e2e", timestamp: at },
      {
        type: "step_start",
        step: "implement",
        persona: "coder",
        iteration: 1,
        timestamp: at,
        workflow: "e2e",
        stack: [{ workflow: "e2e", step: "implement", kind: "agent", occurrence: 1 }],
      },
    ],
  };
}

// 本体ログの1行（JSONL）にする。
export const toJsonLines = (records: readonly Record<string, unknown>[]): string =>
  records.map((record) => `${JSON.stringify(record)}\n`).join("");

// 撮影に使う5つのデータ状態（0件・1件・多数・長文・プロジェクト多数）。各状態はモックの takt-mcp の
// 状態を作る関数と、詳細ページ（ゴール・タスク・run）を開くための識別子を持つ。
//
// 文字列はすべて撮影の確認のために合成した文で、実物のプロンプト本文や秘密の値は含めない。
// 名前類（タスク名・run slug・ゴール ID）は backend の検証（domain.rs の validate_text）に合わせ、
// 改行と制御文字を含めない。長文の状態も、空白を含む長い1行で作る。
import { PROJECT_ABOUT, PROJECT_NAME } from "../support/fixture";
import type { StackProject } from "../support/stack";

export type ScreenshotState = {
  name: "empty" | "one" | "many" | "long" | "many-projects";
  projects: readonly StackProject[];
  mockState: (now: Date) => Record<string, unknown>;
  // 詳細ページで開く対象。0件の状態では実在しない識別子を指し、404 の画面を撮る。
  detail: { goalId: string; taskName: string; runSlug: string; exists: boolean };
};

const DEFAULT_PROJECTS: readonly StackProject[] = [{ name: PROJECT_NAME, about: PROJECT_ABOUT }];

// 既存の fixture.ts の GOAL_ID と同じ UUID の形で、連番から識別子を作る。
const goalId = (index: number): string => `22222222-2222-4222-8222-${String(index).padStart(12, "0")}`;

const minutesBefore = (now: Date, minutes: number): string => new Date(now.getTime() - minutes * 60_000).toISOString();

// 長文の状態で使う文。同じ合成の文を繰り返して長さを作る。
const repeatSentence = (sentence: string, times: number): string => Array.from({ length: times }, () => sentence).join(" ");

type RunInput = { slug: string; taskName: string; goalId: string; startedAt: string; stepCount: number; text: string };

const buildRun = (input: RunInput): Record<string, unknown> => ({
  runSlug: input.slug,
  currentStep: "implement",
  startedAt: input.startedAt,
  currentStepStartedAt: input.startedAt,
  stepLogs: Array.from({ length: input.stepCount }, (_, index) => ({
    step: index % 2 === 0 ? "plan" : "implement",
    persona: "coder",
    status: "completed",
    content: `${input.text} ステップの記録 ${index + 1}`,
    timestamp: input.startedAt,
    startedAt: input.startedAt,
    endedAt: input.startedAt,
    iteration: index + 1,
  })),
  sessionId: `${input.slug}-session`,
  summary: `${input.text} run の要約`,
  workflow: "screenshot",
  status: "running",
  lastLogAt: input.startedAt,
  taskName: input.taskName,
  goalId: input.goalId,
  taskText: `${input.text} タスクの本文`,
});

type GoalDetailInput = {
  id: string;
  status: string;
  executionStatus: string;
  text: string;
  listCount: number;
  workLogCount: number;
  questions: readonly Record<string, unknown>[];
  at: string;
};

const buildGoalDetail = (input: GoalDetailInput): Record<string, unknown> => {
  const list = (label: string): string[] =>
    Array.from({ length: input.listCount }, (_, index) => `${input.text} ${label} ${index + 1}`);
  return {
    summary: {
      id: input.id,
      status: input.status,
      executionStatus: input.executionStatus,
      branch: `takt/screenshot-${input.id}`,
      integrationBranch: "takt/screenshot-integration",
    },
    objective: `${input.text} ゴールの目的`,
    outOfScope: list("対象外"),
    acceptanceCriteria: list("受け入れ条件"),
    constraints: list("制約"),
    specifications: { gherkin: `${input.text} 仕様の記述` },
    workLog: Array.from({ length: input.workLogCount }, (_, index) => ({
      id: `${input.id}-work-${index + 1}`,
      recordedAt: input.at,
      summary: `${input.text} 作業記録 ${index + 1}`,
    })),
    questions: input.questions,
  };
};

const pendingQuestion = (id: string, body: string, at: string): Record<string, unknown> => ({
  id,
  status: "pending",
  kind: "question",
  body,
  options: [],
  createdAt: at,
});

const answeredQuestion = (id: string, body: string, at: string): Record<string, unknown> => ({
  id,
  status: "answered",
  kind: "question",
  body,
  options: ["はい", "いいえ"],
  answer: { text: "撮影確認用の回答", decision: "allow", report: "completed", answeredAt: at },
  createdAt: at,
});

// takt-mcp の状態に何も無いことを表すモック状態。0件の状態とプロジェクト多数の状態で使う。
const EMPTY_MOCK_STATE = (): Record<string, unknown> => ({
  goals: [],
  tasks: [],
  goalQuestions: {},
  standaloneInquiries: [],
  runs: {},
  goalDetails: {},
});

// 詳細ページの対象が実在しない状態（0件・プロジェクト多数）で開く識別子。404 の画面を撮る。
const MISSING_DETAIL: ScreenshotState["detail"] = {
  goalId: goalId(999),
  taskName: "撮影確認用の存在しないタスク",
  runSlug: "screenshot-missing-run",
  exists: false,
};

const empty: ScreenshotState = {
  name: "empty",
  projects: DEFAULT_PROJECTS,
  mockState: EMPTY_MOCK_STATE,
  detail: MISSING_DETAIL,
};

// 1件の状態と長文の状態は同じ構成（ゴール・タスク・run・未回答の質問を1件ずつ）で、文の長さだけが違う。
const singleState = (
  name: "one" | "long",
  texts: { goal: string; task: string; question: string; body: string },
  projects: readonly StackProject[],
): ScreenshotState => {
  const id = goalId(1);
  const runSlug = `screenshot-${name}-run`;
  const questionId = `screenshot-${name}-question`;
  return {
    name,
    projects,
    mockState: (now) => {
      const at = minutesBefore(now, 5);
      const question = pendingQuestion(questionId, texts.question, at);
      return {
        goals: [{ id, status: "created", executionStatus: "active", objective: texts.goal }],
        tasks: [
          { name: texts.task, goalId: id, status: "running", startedAt: at, runSlug, currentStep: "implement", mergeTask: false },
        ],
        goalQuestions: { [id]: [question] },
        standaloneInquiries: [],
        runs: { [runSlug]: buildRun({ slug: runSlug, taskName: texts.task, goalId: id, startedAt: at, stepCount: 1, text: texts.body }) },
        goalDetails: {
          [id]: buildGoalDetail({
            id,
            status: "created",
            executionStatus: "active",
            text: texts.body,
            listCount: 1,
            workLogCount: 1,
            questions: [question],
            at,
          }),
        },
      };
    },
    detail: { goalId: id, taskName: texts.task, runSlug, exists: true },
  };
};

const one = singleState(
  "one",
  {
    goal: "撮影確認用のゴール",
    task: "撮影確認用のタスク",
    question: "撮影確認用の質問の本文",
    body: "撮影確認用の文",
  },
  DEFAULT_PROJECTS,
);

const LONG_SENTENCE = "撮影確認用の長い文で、画面の折り返しと余白とはみ出しを確かめるために同じ内容を繰り返しています。";

const long = singleState(
  "long",
  {
    goal: repeatSentence(LONG_SENTENCE, 6),
    task: repeatSentence(LONG_SENTENCE, 3),
    question: repeatSentence(LONG_SENTENCE, 8),
    body: repeatSentence(LONG_SENTENCE, 10),
  },
  [{ name: PROJECT_NAME, about: repeatSentence(LONG_SENTENCE, 4) }],
);

// 判断待ちページで、改行を含む本文と長さの違う本文を並べて撮る（issue #121）。
// ゴール 1 の質問は詳細ページにも出るため、従来の1行の本文のまま変えない。
const manyQuestionBody = (index: number): string => {
  const head = `撮影確認用の質問の本文 ${index}`;
  if (index === 1) {
    return head;
  }
  switch (index % 3) {
    case 0:
      return `${head}\n2行目の文です。\n3行目の文です。`;
    case 1:
      return `${head} ${repeatSentence(LONG_SENTENCE, 2)}`;
    default:
      return head;
  }
};

const MANY_GOAL_COUNT = 20;
const MANY_RUNNING_COUNT = 3;
const MANY_TARGET_TASK_COUNT = 5;

const many: ScreenshotState = {
  name: "many",
  projects: DEFAULT_PROJECTS,
  mockState: (now) => {
    const goals: Record<string, unknown>[] = [];
    const tasks: Record<string, unknown>[] = [];
    const goalQuestions: Record<string, Record<string, unknown>[]> = {};
    const runs: Record<string, unknown> = {};
    for (let index = 1; index <= MANY_GOAL_COUNT; index += 1) {
      const id = goalId(index);
      const at = minutesBefore(now, index * 3);
      // 末尾の数件はマージ待ちにし、判断待ちページのマージ待ちの欄にも件数を出す。
      const awaitingMerge = index > MANY_GOAL_COUNT - 3;
      goals.push({
        id,
        status: awaitingMerge ? "awaiting_merge" : "created",
        executionStatus: index % 7 === 0 ? "paused" : "active",
        objective: `撮影確認用のゴール ${index}`,
        ...(awaitingMerge ? { pullRequest: { number: index, url: `https://example.com/screenshot/pull/${index}` } } : {}),
      });
      const running = index <= MANY_RUNNING_COUNT;
      const taskName = `撮影確認用のタスク ${index}`;
      const runSlug = `screenshot-many-run-${index}`;
      tasks.push({
        name: taskName,
        goalId: id,
        status: running ? "running" : index % 2 === 0 ? "completed" : "pending",
        startedAt: at,
        ...(running ? { runSlug, currentStep: "implement" } : {}),
        mergeTask: false,
      });
      if (running) {
        runs[runSlug] = buildRun({ slug: runSlug, taskName, goalId: id, startedAt: at, stepCount: 12, text: `撮影確認用の文 ${index}` });
      }
      if (index % 2 === 1) {
        goalQuestions[id] = [pendingQuestion(`screenshot-many-question-${index}`, manyQuestionBody(index), at)];
      }
    }
    // 詳細の対象（1番目のゴール）には、紐づくタスクを複数持たせる。
    const targetId = goalId(1);
    const targetAt = minutesBefore(now, 3);
    for (let index = 2; index <= MANY_TARGET_TASK_COUNT; index += 1) {
      tasks.push({
        name: `撮影確認用のゴール 1 のタスク ${index}`,
        goalId: targetId,
        status: "completed",
        startedAt: targetAt,
        mergeTask: false,
      });
    }
    const targetQuestions = [
      ...(goalQuestions[targetId] ?? []),
      ...Array.from({ length: 5 }, (_, index) =>
        answeredQuestion(`screenshot-many-answered-${index + 1}`, `撮影確認用の回答済みの質問 ${index + 1}`, targetAt),
      ),
    ];
    return {
      goals,
      tasks,
      goalQuestions,
      standaloneInquiries: [
        `撮影確認用の単独の問い合わせ 1\n2行目の文です。\n3行目の文です。`,
        `撮影確認用の単独の問い合わせ 2 ${repeatSentence(LONG_SENTENCE, 2)}`,
        "撮影確認用の単独の問い合わせ 3",
      ].map((body, index) => pendingQuestion(`screenshot-many-inquiry-${index + 1}`, body, targetAt)),
      runs,
      goalDetails: {
        [targetId]: buildGoalDetail({
          id: targetId,
          status: "created",
          executionStatus: "active",
          text: "撮影確認用の文",
          listCount: 8,
          workLogCount: 15,
          questions: targetQuestions,
          at: targetAt,
        }),
      },
    };
  },
  detail: { goalId: goalId(1), taskName: "撮影確認用のタスク 1", runSlug: "screenshot-many-run-1", exists: true },
};

// プロジェクトが多い状態（issue #273）。概要のプロジェクト欄でカードが複数の列へ折り返し、
// 上限（6件）を超えた分が「ほか M 件のプロジェクトを見る」で /projects を指す様子を撮る。
// プロジェクトはバックエンドの設定から作られ、モックの takt-mcp の状態には依存しないため、
// 0件の状態と同じ空データを返し、詳細ページは 404 で撮る。
const MANY_PROJECTS_COUNT = 7;

const manyProjects: ScreenshotState = {
  name: "many-projects",
  projects: Array.from({ length: MANY_PROJECTS_COUNT }, (_, index) => ({
    name: `${PROJECT_NAME}-${index + 1}`,
    about: `撮影確認用のプロジェクトの説明 ${index + 1}`,
  })),
  mockState: EMPTY_MOCK_STATE,
  detail: MISSING_DETAIL,
};

export const SCREENSHOT_STATES: readonly ScreenshotState[] = [empty, one, many, long, manyProjects];

import { test as base } from "@playwright/test";
import { startStack, type Stack } from "./stack";
import { buildMockState } from "./fixture";

// 長い目的文を持つゴールのカードで、題名が1行に収まり親の幅を超えないことを観測するためのスタック。
// 共有の既定 fixture（buildMockState）を変えず、この spec 専用に長い題名のゴールを1件足す。
const LONG_TITLE_GOAL_ID = "22222222-2222-4222-8222-222222222222";
const LONG_TITLE_GOAL_TEXT =
  "採用ページのトップページにある見出しと本文とボタンの配置を見直して、スマートフォン幅での折り返しとタップ領域の広さを確認しながら余白と文字サイズを整える。";

const longTitleStack = async ({}, use: (stack: Stack) => Promise<void>): Promise<void> => {
  const stack = await startStack({
    mockState: (now: Date) => {
      const base = buildMockState(now);
      const goals = base.goals as Array<Record<string, unknown>>;
      const tasks = base.tasks as Array<Record<string, unknown>>;
      return {
        ...base,
        goals: [
          ...goals,
          {
            id: LONG_TITLE_GOAL_ID,
            status: "created",
            executionStatus: "active",
            objective: LONG_TITLE_GOAL_TEXT,
          },
        ],
        tasks: [
          ...tasks,
          {
            name: LONG_TITLE_GOAL_TEXT,
            goalId: LONG_TITLE_GOAL_ID,
            status: "running",
            startedAt: now.toISOString(),
          },
        ],
      };
    },
  });
  try {
    await use(stack);
  } finally {
    await stack.stop();
  }
};

// 同じ行に内容高さの異なる複数のリンクカードを並べる検証用のスタック。
// 既定の stack fixture（1プロジェクト）や他の spec の状態を変えないよう、テスト専用に起動する。
const twoProjectStack = async ({}, use: (stack: Stack) => Promise<void>): Promise<void> => {
  const stack = await startStack({
    projects: [
      { name: "e2e-short-about", about: "短い説明" },
      {
        name: "e2e-long-about",
        about:
          "概要画面のカードで内容の高さが変わることを確かめるため、この説明は意図的に長くして複数行へ折り返させます。同じ行のカードの高さが揃うことを観測します。",
      },
    ],
  });
  try {
    await use(stack);
  } finally {
    await stack.stop();
  }
};

// 長いサーバー名を表示する検証用のスタック。サイドバーのサーバー名が1行に収まるかを
// 測るため、既定より長い名前で専用に起動する。
const longServerStack = async ({}, use: (stack: Stack) => Promise<void>): Promise<void> => {
  const stack = await startStack({ serverName: "E2E Mac mini とても長いサーバーの名前" });
  try {
    await use(stack);
  } finally {
    await stack.stop();
  }
};

// スタックはワーカーごとに1回だけ起動し、spec が失敗しても teardown で必ず止める。
export const test = base.extend<
  { isolatedStack: Stack; twoProjectStack: Stack; longServerStack: Stack; longTitleStack: Stack },
  { stack: Stack }
>({
  stack: [
    async ({}, use) => {
      const stack = await startStack();
      try {
        await use(stack);
      } finally {
        await stack.stop();
      }
    },
    { scope: "worker" },
  ],
  // 状態を変える・backend やモックを止める spec は、他の spec へ影響させないため
  // テストごとに独立したスタックを使う。
  isolatedStack: async ({}, use) => {
    const stack = await startStack();
    try {
      await use(stack);
    } finally {
      await stack.stop();
    }
  },
  twoProjectStack,
  longServerStack,
  longTitleStack,
});

export { expect } from "@playwright/test";

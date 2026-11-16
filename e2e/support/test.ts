import { test as base } from "@playwright/test";
import { startStack, type Stack } from "./stack";

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

// スタックはワーカーごとに1回だけ起動し、spec が失敗しても teardown で必ず止める。
export const test = base.extend<
  { isolatedStack: Stack; twoProjectStack: Stack },
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
});

export { expect } from "@playwright/test";

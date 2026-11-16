import { test as base } from "@playwright/test";
import { startStack, type Stack } from "./stack";

// スタックはワーカーごとに1回だけ起動し、spec が失敗しても teardown で必ず止める。
export const test = base.extend<{ isolatedStack: Stack }, { stack: Stack }>({
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
});

export { expect } from "@playwright/test";

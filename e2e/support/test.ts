import { test as base } from "@playwright/test";
import { startStack, type Stack } from "./stack";

// スタックはワーカーごとに1回だけ起動し、spec が失敗しても teardown で必ず止める。
export const test = base.extend<Record<string, never>, { stack: Stack }>({
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
});

export { expect } from "@playwright/test";

// ページ計測の配線（AC72A-1）。
//
// layout と 2 つのローダーの実 entry を接続できない backend で実行し、console.info に
// page 行が出ることを観測する。RPC が失敗しても finally で page 行が出るため、
// 接続先は未接続の空きポートでよい。
import test from "node:test";
import assert from "node:assert/strict";
import { reserveClosedAddress } from "../support/grpc-server.ts";
import { closeBackendClient } from "../../src/grpc/client.ts";
import RootLayout from "../../app/layout.tsx";
import { ActivityLoader } from "../../src/components/activity-loader.tsx";
import { ProjectsLoader } from "../../src/components/projects-loader.tsx";

const withClosedBackend = async <T>(run: () => Promise<T>): Promise<T> => {
  await closeBackendClient();
  const closedAddress = await reserveClosedAddress();
  const previous = process.env.TAKTDASH_BACKEND_ADDR;
  process.env.TAKTDASH_BACKEND_ADDR = closedAddress;
  try {
    return await run();
  } finally {
    await closeBackendClient();
    if (previous === undefined) {
      delete process.env.TAKTDASH_BACKEND_ADDR;
    } else {
      process.env.TAKTDASH_BACKEND_ADDR = previous;
    }
  }
};

const capturePageLines = async (run: () => Promise<unknown>): Promise<string[]> => {
  const lines: string[] = [];
  const original = console.info;
  console.info = (...data: unknown[]) => {
    lines.push(data.map((value) => String(value)).join(" "));
  };
  try {
    await run();
  } finally {
    console.info = original;
  }
  return lines.filter((line) => line.startsWith("[taktDash][timing] scope=page"));
};

const assertPageLine = (lines: string[], expected: { page: string; rpcs: string }): void => {
  const pattern = new RegExp(
    `^\\[taktDash\\]\\[timing\\] scope=page page=${expected.page} total_ms=\\d+\\.\\d{2} rpc_ms=\\d+\\.\\d{2} next_ms=\\d+\\.\\d{2} rpcs=${expected.rpcs}$`,
  );
  const line = lines.find((candidate) => pattern.test(candidate));
  assert.ok(
    line !== undefined,
    `page=${expected.page} rpcs=${expected.rpcs} の page 行が必要です: ${lines.join(" | ")}`,
  );
};

test("layout は GetOverview の往復を page=layout の行に記録する", async () => {
  const lines = await withClosedBackend(() => capturePageLines(async () => RootLayout({ children: null, params: Promise.resolve({}) })));
  assertPageLine(lines, { page: "layout", rpcs: "GetOverview" });
});

test("プロジェクトのローダーは自身の RPC を持たず page=/projects の行に記録する", async () => {
  const lines = await withClosedBackend(() => capturePageLines(() => ProjectsLoader()));
  assertPageLine(lines, { page: "/projects", rpcs: "" });
});

test("アクティビティのローダーは GetActivity の往復を page=/activity の行に記録する", async () => {
  const lines = await withClosedBackend(() => capturePageLines(() => ActivityLoader()));
  assertPageLine(lines, { page: "/activity", rpcs: "GetActivity" });
});

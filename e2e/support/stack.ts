import { spawn, type ChildProcess } from "node:child_process";
import { closeSync, openSync, readFileSync } from "node:fs";
import { mkdir, mkdtemp, readFile, rename, rm, writeFile } from "node:fs/promises";
import { createServer, connect, type AddressInfo } from "node:net";
import os from "node:os";
import path from "node:path";
import { buildMockState, PROJECT_ABOUT, PROJECT_NAME, SERVER_ID, SERVER_NAME } from "./fixture";

// リポジトリのルートは e2e/support から2階層上。ビルド済み成果物と実行ファイルをここから解決する。
const REPO_ROOT = path.resolve(__dirname, "../..");
const BACKEND_BIN = path.join(REPO_ROOT, "target", "debug", "takt-dash-backend");
const MOCK_SCRIPT = path.join(REPO_ROOT, "backend", "tests", "mock_mcp_server.mjs");
const WEB_DIR = path.join(REPO_ROOT, "frontend", "web");
const NEXT_BIN = path.join(WEB_DIR, "node_modules", ".bin", "next");

export type ToolCall = {
  name: string;
  arguments: Record<string, unknown>;
};

export type Stack = {
  webUrl: string;
  projectDir: string;
  readCalls: () => Promise<ToolCall[]>;
  updateState: (mutate: (state: Record<string, unknown>) => Record<string, unknown>) => Promise<void>;
  stopMock: () => Promise<void>;
  stopBackend: () => Promise<void>;
  stop: () => Promise<void>;
};

type ProcessEnv = { [key: string]: string | undefined };

let tempCounter = 0;

const delay = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));

// 実行時に空きポートを確保する。固定ポート（本番や並行タスクが使う）を避ける。
const freePort = (): Promise<number> =>
  new Promise((resolve, reject) => {
    const server = createServer();
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address() as AddressInfo;
      const port = address.port;
      server.close((error) => (error === undefined ? resolve(port) : reject(error)));
    });
  });

const canConnect = (port: number): Promise<boolean> =>
  new Promise((resolve) => {
    const socket = connect({ host: "127.0.0.1", port });
    socket.once("connect", () => {
      socket.destroy();
      resolve(true);
    });
    socket.once("error", () => {
      socket.destroy();
      resolve(false);
    });
  });

const waitForPort = async (port: number, timeoutMs: number, what: string, hasExited: () => boolean): Promise<void> => {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (hasExited()) {
      throw new Error(`${what} が待ち受ける前に終了しました。`);
    }
    if (await canConnect(port)) {
      return;
    }
    await delay(100);
  }
  throw new Error(`${what} が 127.0.0.1:${port} で待ち受けませんでした。`);
};

const waitForHttp = async (url: string, timeoutMs: number, hasExited: () => boolean): Promise<void> => {
  const deadline = Date.now() + timeoutMs;
  let lastError = "";
  while (Date.now() < deadline) {
    if (hasExited()) {
      throw new Error(`web が応答する前に終了しました。`);
    }
    try {
      const response = await fetch(url);
      if (response.ok) {
        return;
      }
      lastError = `HTTP ${response.status}`;
    } catch (error) {
      lastError = String(error);
    }
    await delay(100);
  }
  throw new Error(`${url} が応答しませんでした: ${lastError}`);
};

// backend は起動直後の最初の集約を終えるまで概要を返せない。ポートの待ち受けだけでは
// 足りないため、レイアウトが backend 不達の告知を出さなくなるまで待つ。
const waitForBackendReady = async (url: string, timeoutMs: number, hasExited: () => boolean): Promise<void> => {
  const deadline = Date.now() + timeoutMs;
  let lastError = "";
  while (Date.now() < deadline) {
    if (hasExited()) {
      throw new Error(`web が backend の応答を得る前に終了しました。`);
    }
    try {
      const response = await fetch(url);
      const body = await response.text();
      if (response.ok && !body.includes("backend に接続できません")) {
        return;
      }
      lastError = `HTTP ${response.status}`;
    } catch (error) {
      lastError = String(error);
    }
    await delay(100);
  }
  throw new Error(`${url} が backend の応答を得られませんでした: ${lastError}`);
};

const isAlive = (pid: number): boolean => {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return (error as { code?: string }).code === "EPERM";
  }
};

const waitForExit = async (pid: number, timeoutMs: number): Promise<boolean> => {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (!isAlive(pid)) {
      return true;
    }
    await delay(50);
  }
  return !isAlive(pid);
};

// detached で起動した子はプロセスグループの先頭になる。グループごと止めて取りこぼしを防ぐ。
const signalGroup = (pid: number, signal: "SIGTERM" | "SIGKILL"): void => {
  try {
    process.kill(-pid, signal);
  } catch {
    try {
      process.kill(pid, signal);
    } catch {
      // 既に終了している。
    }
  }
};

const stopGroup = async (pid: number): Promise<void> => {
  signalGroup(pid, "SIGTERM");
  if (await waitForExit(pid, 5_000)) {
    return;
  }
  signalGroup(pid, "SIGKILL");
  await waitForExit(pid, 5_000);
};

const stopPid = async (pid: number): Promise<void> => {
  try {
    process.kill(pid, "SIGTERM");
  } catch {
    // 既に終了している。
  }
  if (await waitForExit(pid, 3_000)) {
    return;
  }
  try {
    process.kill(pid, "SIGKILL");
  } catch {
    // 既に終了している。
  }
  await waitForExit(pid, 3_000);
};

const readMockPid = async (pidFile: string): Promise<number | undefined> => {
  try {
    const pid = Number((await readFile(pidFile, "utf8")).trim());
    return Number.isInteger(pid) && pid > 0 ? pid : undefined;
  } catch {
    return undefined;
  }
};

const readMockPidSync = (pidFile: string): number | undefined => {
  try {
    const pid = Number(readFileSync(pidFile, "utf8").trim());
    return Number.isInteger(pid) && pid > 0 ? pid : undefined;
  } catch {
    return undefined;
  }
};

const spawnWithLog = (
  command: string,
  args: string[],
  cwd: string,
  logPath: string,
  env: ProcessEnv,
): ChildProcess => {
  const fd = openSync(logPath, "a");
  try {
    const child = spawn(command, args, {
      cwd,
      env,
      detached: true,
      stdio: ["ignore", fd, fd],
    });
    return child;
  } finally {
    closeSync(fd);
  }
};

const logTail = async (logPath: string): Promise<string> => {
  try {
    const text = await readFile(logPath, "utf8");
    return text.slice(-2_000);
  } catch {
    return "";
  }
};

type StackConfigInput = {
  backendAddress: string;
  projectDir: string;
  statePath: string;
  callsPath: string;
  mockPidPath: string;
};

const buildConfig = (input: StackConfigInput): Record<string, unknown> => ({
  mcpServers: {
    [SERVER_ID]: {
      command: "node",
      args: [
        MOCK_SCRIPT,
        "--state",
        input.statePath,
        "--calls",
        input.callsPath,
        "--pid-file",
        input.mockPidPath,
      ],
      env: {},
    },
  },
  servers: {
    [SERVER_ID]: { name: SERVER_NAME, kind: "mac-mini", concurrency: 1 },
  },
  projects: [{ name: PROJECT_NAME, server: SERVER_ID, about: PROJECT_ABOUT, cwd: input.projectDir }],
  intervalSeconds: 1,
  thresholds: { pendingDecisionStaleMinutes: 240, runLogSilentMinutes: 10 },
  listen: input.backendAddress,
});

export async function startStack(): Promise<Stack> {
  // 状態・設定・SQLite・ログは実行ごとの一時ディレクトリへ置き、リポジトリ内に残さない。
  const tempDir = await mkdtemp(path.join(os.tmpdir(), "taktdash-e2e-"));
  const projectDir = path.join(tempDir, "project");
  await mkdir(projectDir, { recursive: true });
  const statePath = path.join(tempDir, "state.json");
  const configPath = path.join(tempDir, "config.json");
  const sqlitePath = path.join(tempDir, "db.sqlite");
  const callsPath = path.join(tempDir, "calls.jsonl");
  const mockPidPath = path.join(tempDir, "mock.pid");
  const backendLog = path.join(tempDir, "backend.log");
  const webLog = path.join(tempDir, "web.log");
  const homeDir = path.join(tempDir, "home");

  let backend: ChildProcess | undefined;
  let web: ChildProcess | undefined;
  let backendExited = false;
  let webExited = false;

  const stopBackendProcess = async (): Promise<void> => {
    if (backend?.pid !== undefined) {
      await stopGroup(backend.pid);
    }
  };

  // backend は読み取り失敗の次のサイクルでモックを起動し直すため、停止済みフラグでは
  // 短絡せず毎回 pid ファイルを読む。既に終了している pid を指していても失敗しない。
  const stopMockProcess = async (): Promise<void> => {
    const mockPid = await readMockPid(mockPidPath);
    if (mockPid !== undefined) {
      await stopPid(mockPid);
    }
  };

  const readCalls = async (): Promise<ToolCall[]> => {
    let text: string;
    try {
      text = await readFile(callsPath, "utf8");
    } catch (error) {
      if ((error as { code?: string }).code === "ENOENT") {
        return [];
      }
      throw error;
    }
    const endsWithNewline = text.endsWith("\n");
    const lines = text.split("\n");
    if (endsWithNewline) {
      lines.pop();
    }
    const calls: ToolCall[] = [];
    for (const [index, line] of lines.entries()) {
      if (line.trim() === "") {
        continue;
      }
      try {
        calls.push(JSON.parse(line) as ToolCall);
      } catch (error) {
        // 追記途中の末尾1行だけは、次の書き込みで完成するため読み飛ばす。
        if (!endsWithNewline && index === lines.length - 1) {
          continue;
        }
        throw error;
      }
    }
    return calls;
  };

  // 状態ファイルはモックが毎回 JSON.parse するため、同じディレクトリの一時ファイルへ書いて
  // rename し、途中状態の JSON を読ませない。
  const updateState = async (
    mutate: (state: Record<string, unknown>) => Record<string, unknown>,
  ): Promise<void> => {
    const current = JSON.parse(await readFile(statePath, "utf8")) as Record<string, unknown>;
    const tempPath = path.join(path.dirname(statePath), `state.${process.pid}.${tempCounter}.tmp`);
    tempCounter += 1;
    await writeFile(tempPath, JSON.stringify(mutate(current)), "utf8");
    try {
      await rename(tempPath, statePath);
    } catch (error) {
      await rm(tempPath, { force: true });
      throw error;
    }
  };

  const cleanup = async (): Promise<void> => {
    if (web?.pid !== undefined) {
      await stopGroup(web.pid);
    }
    await stopBackendProcess();
    await stopMockProcess();
    await rm(tempDir, { recursive: true, force: true });
  };

  // process.exit は finally を迂回するため、同期の最終手段を exit にだけ用意する。
  const onExit = (): void => {
    for (const pid of [web?.pid, backend?.pid]) {
      if (pid !== undefined) {
        signalGroup(pid, "SIGKILL");
      }
    }
    const killed = readMockPidSync(mockPidPath);
    if (killed !== undefined) {
      try {
        process.kill(killed, "SIGKILL");
      } catch {
        // 既に終了している。
      }
    }
  };
  process.once("exit", onExit);

  let backendAddress = "";
  let webUrl = "";

  try {
    await writeFile(statePath, JSON.stringify(buildMockState(new Date())), "utf8");
    await mkdir(homeDir, { recursive: true });
    const backendPort = await freePort();
    backendAddress = `127.0.0.1:${backendPort}`;
    await writeFile(
      configPath,
      JSON.stringify(buildConfig({ backendAddress, projectDir, statePath, callsPath, mockPidPath })),
      "utf8",
    );
    const webPort = await freePort();
    // Next は route handler の request.url を listen ホスト名ではなく localhost で組み立てる。
    // origin 境界の検証と一致させるため、利用時と同じ localhost を webUrl に使う。
    webUrl = `http://localhost:${webPort}`;

    backend = spawnWithLog(
      BACKEND_BIN,
      ["--config", configPath, "--listen", backendAddress, "--sqlite", sqlitePath],
      REPO_ROOT,
      backendLog,
      { ...process.env, HOME: homeDir },
    );
    if (backend.pid === undefined) {
      throw new Error("backend を起動できませんでした。");
    }
    backend.once("exit", () => {
      backendExited = true;
    });
    await waitForPort(backendPort, 30_000, "backend", () => backendExited);

    web = spawnWithLog(
      NEXT_BIN,
      ["start", "-H", "127.0.0.1", "-p", String(webPort)],
      WEB_DIR,
      webLog,
      { ...process.env, TAKTDASH_BACKEND_ADDR: backendAddress },
    );
    if (web.pid === undefined) {
      throw new Error("web を起動できませんでした。");
    }
    web.once("exit", () => {
      webExited = true;
    });
    await waitForHttp(webUrl, 60_000, () => webExited);
    await waitForBackendReady(webUrl, 30_000, () => webExited);
  } catch (error) {
    const backendTail = await logTail(backendLog);
    const webTail = await logTail(webLog);
    process.removeListener("exit", onExit);
    await cleanup();
    throw new Error(
      `${(error as Error).message}\n--- backend.log ---\n${backendTail}\n--- web.log ---\n${webTail}`,
    );
  }

  let stopped = false;
  const stop = async (): Promise<void> => {
    if (stopped) {
      return;
    }
    stopped = true;
    process.removeListener("exit", onExit);
    await cleanup();
  };

  return {
    webUrl,
    projectDir,
    readCalls,
    updateState,
    stopMock: stopMockProcess,
    stopBackend: stopBackendProcess,
    stop,
  };
}

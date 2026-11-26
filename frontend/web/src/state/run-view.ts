import type { AnomalyThresholds, Goal, Project, Run, Server } from "../proto/taktdash";
import { formatClock } from "./format";
import { goalDisplayName } from "./goal-summary";
import { CONNECTION_STATE } from "./proto-values";

// 復元境界の内部状態では、ゴール非所属の Run.goalId は未指定になるため、
// 表示用の変換は proto の必須形ではなく未指定を許す入力型で受ける。
export type RunInput = Omit<Run, "goalId"> & { goalId: string | undefined };

export type RunView = {
  run: RunInput;
  server: Server | undefined;
  connected: boolean;
  state: "live" | "warn" | "idle";
  label: string;
  note: string;
  // ゴールの表示名。takt が切った preview には省略記号を付けてある（issue #230）。
  goalName?: string;
  currentStepName?: string;
  stepNumber?: number;
  totalSteps?: number;
  iteration?: number;
  model?: string;
  elapsedSeconds?: number;
};

export const projectKey = (serverId: string, name: string): string => `${serverId}\u0000${name}`;

export function toRunView(
  run: RunInput,
  serverById: ReadonlyMap<string, Server>,
  thresholds: AnomalyThresholds,
  now: Date,
  goalsById?: ReadonlyMap<string, Goal>,
  projectByKey?: ReadonlyMap<string, Project>,
): RunView {
  const server = serverById.get(run.serverId);
  const lastStep = run.steps.at(-1);
  const goal = run.goalId !== undefined ? goalsById?.get(run.goalId) : undefined;
  const goalName = goal !== undefined && goal.name !== "" ? goalDisplayName(goal) : undefined;
  const elapsedSeconds = run.lastLogAt
    ? Math.max(0, Math.floor((now.getTime() - run.lastLogAt.getTime()) / 1_000))
    : undefined;
  const derived = {
    run,
    server,
    goalName,
    currentStepName:
      run.currentStepName !== undefined && run.currentStepName !== "" ? run.currentStepName : undefined,
    model: lastStep?.model !== undefined && lastStep.model !== "" ? lastStep.model : undefined,
    stepNumber: run.currentStepNumber,
    totalSteps: run.totalSteps,
    iteration: run.currentIteration,
    elapsedSeconds,
  };
  if (server === undefined) {
    return {
      ...derived,
      connected: false,
      state: "idle",
      label: "接続未確認（サーバー未登録）",
      note: "接続先のサーバーが登録されていないため、接続を確認できません。",
    };
  }
  const project = projectByKey?.get(projectKey(run.serverId, run.project));
  if (!server.connected || project?.connectionState === CONNECTION_STATE.UNRESPONSIVE) {
    const lastReadAt = project?.lastReadAt ?? server.lastReadAt;
    const at = lastReadAt ? formatClock(lastReadAt) : "不明";
    return {
      ...derived,
      connected: false,
      state: "idle",
      label: `実行中（${at} 時点）`,
      note: "サーバーから応答がないため、最後に読めた時点の状態です。",
    };
  }
  // run の表示は、プロジェクトの読み取り状態ではなく run 自身のログの鮮度で決める。
  // 読み取りが遅れている（DELAYED）だけでは run の表示を変えず、読み取り時刻を注記に
  // 使わない。「遅れているのに最後に読めたのは 0 秒前」のような自己矛盾が起きるため、
  // 遅延の注記は最後のログの時刻で書く（issue #194）。
  const silentMinutes = run.lastLogAt
    ? Math.max(0, Math.floor((now.getTime() - run.lastLogAt.getTime()) / 60_000))
    : undefined;
  // 無音は最後のログの時刻と閾値の比較ひとつで決める。backend の is_log_silent_at と同じく
  // 閾値ちょうどは含めず、ミリ秒で比べる。分に丸めて比べたり、残った古い logSilent も
  // 採ったりすると、新しいログが来た後も警告が残る（issue #239, #138）。
  const silent =
    run.lastLogAt !== undefined &&
    now.getTime() - run.lastLogAt.getTime() > thresholds.runLogSilentMinutes * 60_000;
  if (silent && silentMinutes !== undefined) {
    return {
      ...derived,
      connected: true,
      state: "warn",
      label: "ログが止まっています",
      note: `ログが ${silentMinutes}分 出ていません。`,
    };
  }
  return { ...derived, connected: true, state: "live", label: "実行中", note: "" };
}

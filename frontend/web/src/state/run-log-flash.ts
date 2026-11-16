// 新しいログが届いた run 行の発光を決める。
//
// SSE で届く lastLogAt の前進だけを「新しいログ」として扱い、初回観測では光らせない。
import type { Run } from "../proto/taktdash";

export type LogSeen = Readonly<Record<string, number>>;

export const FLASH_MS = 1_000;

const loggedAt = (run: Run): number | undefined => run.lastLogAt?.getTime();

export function initialLogSeen(runs: readonly Run[]): LogSeen {
  const seen: Record<string, number> = {};
  for (const run of runs) {
    const value = loggedAt(run);
    if (value !== undefined) {
      seen[run.runSlug] = value;
    }
  }
  return seen;
}

export function nextLogSeen(
  prev: LogSeen,
  runs: readonly Run[],
): { seen: LogSeen; flashed: readonly string[] } {
  const seen: Record<string, number> = {};
  const flashed: string[] = [];
  for (const run of runs) {
    const value = loggedAt(run);
    if (value === undefined) {
      continue;
    }
    seen[run.runSlug] = value;
    const previous = prev[run.runSlug];
    if (previous === undefined || value > previous) {
      flashed.push(run.runSlug);
    }
  }
  return { seen, flashed };
}

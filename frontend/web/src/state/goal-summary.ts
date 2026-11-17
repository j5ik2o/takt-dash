// ゴール表示に使う見出しとタスク進捗（issue #78）。
//
// API はゴールの題名を返さないため、目的の最初の1文を見出しにする。引用符や
// 括弧の内側の句点は文の境界ではないので、開き括弧の深さを追って深さ0の
// 文末記号だけで切る。タスクの進み具合は6種類の状態の合計と、失敗3種の合計から作る。
import type { TaskStatusCounts } from "../proto/taktdash";

const CLOSER_BY_OPENER = new Map<string, string>([
  ["「", "」"],
  ["『", "』"],
  ["（", "）"],
  ["(", ")"],
  ["【", "】"],
  ["[", "]"],
  ["〈", "〉"],
  ["《", "》"],
]);

const CLOSING_BRACKETS = new Set(CLOSER_BY_OPENER.values());
const SENTENCE_END = new Set(["。", "！", "？"]);

// 括弧・引用の外側にある最初の文末記号までを1文として取り出す。対応する開き括弧が
// 無い閉じ括弧は深さを負にせず無視し、文末記号が無ければ前後の空白を除いた全体を返す。
export function goalHeading(name: string): string {
  const expectedClosers: string[] = [];
  for (let index = 0; index < name.length; index += 1) {
    const char = name.charAt(index);
    const closer = CLOSER_BY_OPENER.get(char);
    if (closer !== undefined) {
      expectedClosers.push(closer);
      continue;
    }
    if (CLOSING_BRACKETS.has(char)) {
      if (expectedClosers.length > 0 && expectedClosers[expectedClosers.length - 1] === char) {
        expectedClosers.pop();
      }
      continue;
    }
    if (SENTENCE_END.has(char) && expectedClosers.length === 0) {
      return name.slice(0, index + 1).trim();
    }
  }
  return name.trim();
}

export type GoalTaskProgress = {
  completed: number;
  total: number;
  failed: number;
  running: number;
};

// タスクの全体は6種類の合計、失敗は失敗・超過・PR失敗の合計。件数が無いゴールは
// 実在しない件数を補わず、すべて0として扱う。実行中の件数は、ゴールの状態表示が
// 実際に run が動いているかどうかを区別するために使う（issue #189）。
export function goalTaskProgress(goal: {
  taskCounts?: TaskStatusCounts | undefined;
}): GoalTaskProgress {
  const counts = goal.taskCounts;
  if (counts === undefined) {
    return { completed: 0, total: 0, failed: 0, running: 0 };
  }
  return {
    completed: counts.completed,
    total:
      counts.pending +
      counts.running +
      counts.completed +
      counts.failed +
      counts.exceeded +
      counts.prFailed,
    failed: counts.failed + counts.exceeded + counts.prFailed,
    running: counts.running,
  };
}

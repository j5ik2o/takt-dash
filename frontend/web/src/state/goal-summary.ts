// ゴール表示に使う見出しとタスク進捗（issue #78）。
//
// API はゴールの題名を返さないため、目的の最初の1文を見出しにする。引用符や
// 括弧の内側の句点は文の境界ではないので、開き括弧の深さを追って深さ0の
// 文末記号だけで切る。タスクの進み具合は6種類の状態の合計と、失敗3種の合計から作る。
import type { TaskStatusCounts } from "../proto/taktdash";
import { toTaskCount, type TaskCount } from "./domain-primitives";

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
// complete は、文末記号で閉じた1文を取り出せたか。
function firstSentence(name: string): { text: string; complete: boolean } {
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
      return { text: name.slice(0, index + 1).trim(), complete: true };
    }
  }
  return { text: name.trim(), complete: false };
}

export function goalHeading(name: string): string {
  return firstSentence(name).text;
}

// takt-mcp の一覧応答は、ゴールの objective を UTF-8 の 256 バイトで切り詰めた
// preview を名前として返す。backend は takt の応答の印（要素の omissions.objective.totalBytes
// と、ページの omissions）から切り詰めを判定し、Goal.nameTruncated に載せる（issue #230）。
// 表示側は文の形やバイト長から推定せず、この印だけで省略記号を付ける。name 自体には
// backend も省略記号を混ぜないため、省略記号は1つだけになる。
export type GoalNameInput = { name: string; nameTruncated: boolean };

const ELLIPSIS = "…";

// 一覧で名前の全体を出す場合の表示名。切り詰められた preview には省略記号を付ける。
export function goalDisplayName(goal: GoalNameInput): string {
  return goal.nameTruncated ? `${goal.name.trim()}${ELLIPSIS}` : goal.name;
}

// 一覧の見出し。最初の1文が preview の範囲で文末記号まで完結する場合はその文を出し、
// 1文自体が切り詰められている場合だけ省略記号を付ける。
export function goalListHeading(goal: GoalNameInput): string {
  const heading = firstSentence(goal.name);
  return goal.nameTruncated && !heading.complete ? `${heading.text}${ELLIPSIS}` : heading.text;
}

export type GoalTaskProgress = {
  completed: number;
  total: number;
  failed: number;
  running: TaskCount;
};

// タスクの全体は6種類の合計、失敗は失敗・超過・PR失敗の合計。件数が無いゴールは
// 実在しない件数を補わず、すべて0として扱う。実行中の件数は、ゴールの状態表示が
// 実際に run が動いているかどうかを区別するために使う（issue #189）。件数の
// 不変条件は専用型 TaskCount で保持する。
export function goalTaskProgress(goal: {
  taskCounts?: TaskStatusCounts | undefined;
}): GoalTaskProgress {
  const counts = goal.taskCounts;
  if (counts === undefined) {
    return { completed: 0, total: 0, failed: 0, running: toTaskCount(0) };
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
    running: toTaskCount(counts.running),
  };
}

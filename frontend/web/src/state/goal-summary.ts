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

// takt-mcp の一覧応答は、ゴールの objective を UTF-8 の 256 バイトで切り詰めた
// preview を名前として返す。切り詰めの印は応答の別項目にあり、proto の Goal.name
// には載らないため、表示側はバイト長と文末の形から切り詰めを推定する。バイト長が
// preview の上限に達し、文末記号でも閉じ括弧でも終わっていない名前は、文の途中で
// 切れた preview だと判断して省略記号を付け、単語の途中で黙って切れたように見せない
// （issue #153）。恒久対策は backend が切り詰めの印を proto へ載せることであり、
// ここでの推定はそれまでの表示側の対処である。
const PREVIEW_BYTE_LIMIT = 256;
// 切り詰めは文字の境界で行われるため、結果は上限から最大で1文字分（4バイト）短くなる。
const PREVIEW_BYTE_FLOOR = PREVIEW_BYTE_LIMIT - 4;

const utf8Encoder = new TextEncoder();

const endsComplete = (value: string): boolean => {
  const last = value.charAt(value.length - 1);
  return SENTENCE_END.has(last) || CLOSING_BRACKETS.has(last);
};

// 一覧応答の preview として切り詰められた名前かどうかを推定する。
export function goalNameIsTruncatedPreview(name: string): boolean {
  const trimmed = name.trim();
  if (trimmed === "" || endsComplete(trimmed)) {
    return false;
  }
  return utf8Encoder.encode(trimmed).length >= PREVIEW_BYTE_FLOOR;
}

// 一覧で名前の全体を出す場合の表示名。切り詰められた preview には省略記号を付ける。
export function goalDisplayName(name: string): string {
  return goalNameIsTruncatedPreview(name) ? `${name.trim()}…` : name;
}

// 一覧の見出し。最初の1文が preview の範囲で完結する場合はその文を出し、
// 1文自体が切り詰められている場合は省略記号を付ける。
export function goalListHeading(name: string): string {
  const heading = goalHeading(name);
  return heading === name.trim() && goalNameIsTruncatedPreview(name) ? `${heading}…` : heading;
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

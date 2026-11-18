// run 詳細のログの追記分を、画面の状態へ適用する。
//
// 初期表示は RSC が GetRunDetail から得た log_tail の行です。開いている間は Route Handler
// （/api/runs/[slug]/updates）から追記分を取り、reset なら行を置き換え、そうでなければ末尾へ足す。
// 続きの位置（世代と最後の行の番号）は、backend が返した値だけから作る。
//
// 生成コード（src/proto/taktdash.ts）の fromJSON は @grpc/grpc-js を値として読み込むため、
// ブラウザ側のバンドルへ持ち込めない。応答の JSON はここで復元する。

/** 画面が保持する行の上限。backend が番号付きで保持する行の上限と同じです。 */
export const MAX_RUN_LOG_LINES = 200;

/** 前回受け取った行の位置。 */
export type RunLogCursor = {
  epoch: number;
  sequence: number;
};

/** 画面に出す1行。番号は backend から届いた行だけが持つ。 */
export type RunLogLineView = {
  sequence?: number;
  text: string;
};

/** セッションログからの活動の印。 */
export type RunSessionActivityView = {
  latestTool: string | undefined;
  toolUseCount: number;
  updatedAt: Date | undefined;
};

/** run 詳細のログの節の状態。 */
export type RunLogView = {
  lines: readonly RunLogLineView[];
  // 位置を持たない間は、次の取得で全体を受け取る。
  cursor: RunLogCursor | undefined;
  activity: RunSessionActivityView | undefined;
  logsRemoved: boolean;
};

/** Route Handler から届いた追記分。 */
export type RunUpdates = {
  epoch: number;
  reset: boolean;
  lines: readonly { sequence: number; text: string }[];
  activity: RunSessionActivityView;
  logsRemoved: boolean;
};

export function initialRunLogView(logTail: string, logsRemoved: boolean): RunLogView {
  return {
    lines: logTail === "" ? [] : logTail.split("\n").map((text) => ({ text })),
    cursor: undefined,
    activity: undefined,
    logsRemoved,
  };
}

/** 追記分を適用した新しい状態を返す。元の状態は変えない。 */
export function applyRunUpdates(view: RunLogView, updates: RunUpdates): RunLogView {
  const merged = updates.reset ? updates.lines : [...view.lines, ...updates.lines];
  const last = updates.lines.at(-1);
  let cursor: RunLogCursor | undefined;
  if (last !== undefined) {
    cursor = { epoch: updates.epoch, sequence: last.sequence };
  } else if (!updates.reset) {
    cursor = view.cursor;
  }
  return {
    lines: merged.slice(Math.max(0, merged.length - MAX_RUN_LOG_LINES)),
    cursor,
    activity: updates.activity,
    logsRemoved: updates.logsRemoved,
  };
}

/** 追記分を返す Route Handler の URL。位置が無ければ問い合わせを付けない。 */
export function runUpdatesUrl(runSlug: string, cursor: RunLogCursor | undefined): string {
  const base = `/api/runs/${encodeURIComponent(runSlug)}/updates`;
  if (cursor === undefined) {
    return base;
  }
  const params = new URLSearchParams({ epoch: String(cursor.epoch), after: String(cursor.sequence) });
  return `${base}?${params.toString()}`;
}

// proto の JSON 表現は int64 を文字列で表すことがあるため、数値と数値の文字列の両方を受ける。
const toNumber = (value: unknown, what: string): number => {
  if (value === undefined || value === null) {
    return 0;
  }
  const number = typeof value === "number" ? value : typeof value === "string" ? Number(value) : Number.NaN;
  if (!Number.isFinite(number)) {
    throw new Error(`追記分の応答の ${what} が数値ではありません。`);
  }
  return number;
};

const toOptionalDate = (value: unknown): Date | undefined => {
  if (value === undefined || value === null) {
    return undefined;
  }
  const date = typeof value === "string" ? new Date(value) : undefined;
  if (date === undefined || Number.isNaN(date.getTime())) {
    throw new Error("追記分の応答の更新時刻が時刻ではありません。");
  }
  return date;
};

const toRecord = (value: unknown, what: string): Record<string, unknown> => {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new Error(`追記分の応答の ${what} が不正です。`);
  }
  return value as Record<string, unknown>;
};

/**
 * Route Handler が返す GetRunUpdatesResponse.toJSON の JSON を復元する。
 * proto の JSON 表現は既定値の項目（reset=false、空の行、0件）を省くため、省かれた項目は既定値にする。
 */
export function restoreRunUpdates(json: unknown): RunUpdates {
  const raw = toRecord(json, "本体");
  const lines = raw.lines ?? [];
  if (!Array.isArray(lines)) {
    throw new Error("追記分の応答の行が配列ではありません。");
  }
  const activity = raw.activity === undefined ? {} : toRecord(raw.activity, "活動の印");
  const latestTool = activity.latestTool;
  if (latestTool !== undefined && typeof latestTool !== "string") {
    throw new Error("追記分の応答の道具名が文字列ではありません。");
  }
  return {
    epoch: toNumber(raw.epoch, "世代"),
    reset: raw.reset === true,
    lines: lines.map((line) => {
      const record = toRecord(line, "行");
      const text = record.text ?? "";
      if (typeof text !== "string") {
        throw new Error("追記分の応答の行の本文が文字列ではありません。");
      }
      return { sequence: toNumber(record.sequence, "行の番号"), text };
    }),
    activity: {
      latestTool,
      toolUseCount: toNumber(activity.toolUseCount, "道具の呼び出しの件数"),
      updatedAt: toOptionalDate(activity.updatedAt),
    },
    logsRemoved: raw.logsRemoved === true,
  };
}

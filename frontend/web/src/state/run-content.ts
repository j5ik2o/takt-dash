// run 詳細のレポートと指示書の本文を、開いたときに取得する状態の遷移と応答の復元。
//
// 本文は run 詳細の最初の描画には含めず、項目を初めて開いたときに Route Handler
// （/api/runs/[slug]/content）から取得する。取得済みの本文は閉じても保持し、開き直しで
// 取得し直さない。取得に失敗した項目は、次に開いたときに取得し直す。

/** 本文を取得する対象。Route Handler のクエリへ変換する。 */
export type RunContentRequestTarget = { kind: "report"; filename: string } | { kind: "instruction"; instructionId: number };

/** 取得した本文。 */
export type RunContentBody = {
  content: string;
  sizeBytes: number;
  truncated: boolean;
};

/** 1件の本文の表示状態。 */
export type RunContentView =
  | { status: "closed" }
  | { status: "loading" }
  | { status: "loaded"; body: RunContentBody }
  | { status: "failed"; error: string };

export const initialRunContentView: RunContentView = { status: "closed" };

/**
 * 項目を開いたときの遷移。まだ取得していない項目と、取得に失敗した項目だけを取得中にし、
 * 取得を要求する。取得中と取得済みの項目は状態を変えず、取得を要求しない。
 */
export function runContentOpened(view: RunContentView): { view: RunContentView; fetch: boolean } {
  if (view.status === "closed" || view.status === "failed") {
    return { view: { status: "loading" }, fetch: true };
  }
  return { view, fetch: false };
}

/** 取得成功。 */
export function runContentLoaded(body: RunContentBody): RunContentView {
  return { status: "loaded", body };
}

/** 取得失敗。 */
export function runContentFailed(error: string): RunContentView {
  return { status: "failed", error };
}

/** 本文を返す Route Handler の URL。 */
export function runContentUrl(runSlug: string, target: RunContentRequestTarget): string {
  const params =
    target.kind === "report"
      ? new URLSearchParams({ report: target.filename })
      : new URLSearchParams({ instruction: String(target.instructionId) });
  return `/api/runs/${encodeURIComponent(runSlug)}/content?${params.toString()}`;
}

/**
 * Route Handler が返す GetRunContentResponse.toJSON の JSON を復元する。
 * proto の JSON 表現は既定値の項目を省くため、content の無い応答は空の本文として復元する。
 * content が文字列以外の応答と、object でない応答は、表示できない応答として拒否する。
 */
export function restoreRunContentResponse(json: unknown): RunContentBody {
  if (typeof json !== "object" || json === null) {
    throw new Error("本文の応答が不正です。");
  }
  const raw = json as Record<string, unknown>;
  const content = raw.content ?? "";
  if (typeof content !== "string") {
    throw new Error("本文の応答の本文が文字列ではありません。");
  }
  const sizeBytes = typeof raw.sizeBytes === "number" ? raw.sizeBytes : Number(raw.sizeBytes ?? 0);
  return {
    content,
    sizeBytes: Number.isFinite(sizeBytes) ? sizeBytes : 0,
    truncated: raw.truncated === true,
  };
}

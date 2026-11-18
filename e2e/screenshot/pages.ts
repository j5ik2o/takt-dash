// 撮影の対象のページ。frontend/web/app の page.tsx があるルート（app/api の route handler を除く）を
// すべて並べる。name はファイル名に使い、heading は表示の準備ができたことを確かめる h1 の文言。
// 詳細ページは状態ごとの対象を開き、0件の状態では実在しない対象を開いて 404 の画面を撮る。
import { PROJECT_NAME } from "../support/fixture";
import type { ScreenshotState } from "./states";

export type ScreenshotPage = {
  name: string;
  path: (state: ScreenshotState) => string;
  // 詳細ページでは、対象が実在する状態だけ h1 が出る。
  heading: (state: ScreenshotState) => string;
  detail: boolean;
};

export const SCREENSHOT_PAGES: readonly ScreenshotPage[] = [
  { name: "overview", path: () => "/", heading: () => "概要", detail: false },
  { name: "decisions", path: () => "/decisions", heading: () => "判断待ち", detail: false },
  { name: "goals", path: () => "/goals", heading: () => "ゴール", detail: false },
  {
    name: "goal-detail",
    path: (state) => `/goals/${encodeURIComponent(state.detail.goalId)}`,
    heading: (state) => `ゴール ${state.detail.goalId}`,
    detail: true,
  },
  {
    name: "task-detail",
    // goal-detail-content.tsx のタスクへのリンクと同じ形の URL。
    path: (state) =>
      `/tasks/${encodeURIComponent(PROJECT_NAME)}?task=${encodeURIComponent(state.detail.taskName)}&goal=${encodeURIComponent(
        state.detail.goalId,
      )}`,
    heading: (state) => state.detail.taskName,
    detail: true,
  },
  {
    name: "run-detail",
    path: (state) => `/runs/${encodeURIComponent(state.detail.runSlug)}`,
    heading: (state) => state.detail.taskName,
    detail: true,
  },
  { name: "activity", path: () => "/activity", heading: () => "アクティビティ", detail: false },
  { name: "conversations", path: () => "/conversations", heading: () => "会話", detail: false },
  { name: "projects", path: () => "/projects", heading: () => "プロジェクト", detail: false },
];

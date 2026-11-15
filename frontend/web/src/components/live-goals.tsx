"use client";

import type { ReactElement } from "react";
import { useLiveState } from "./state-provider";
import { GoalsContent } from "./goals-content";

type LiveGoalsProps = {
  project?: string;
};

// ゴール一覧は既存の WatchState の SSE で更新される state.goals を使う。
// 一覧のために新しいポーリング経路は増やさない。
export function LiveGoals({ project }: LiveGoalsProps): ReactElement {
  const { state } = useLiveState();
  return <GoalsContent state={state} project={project} />;
}

"use client";

import type { ReactElement } from "react";
import { buildOverviewView } from "../state/dashboard";
import { useLiveState } from "./state-provider";
import { OverviewContent } from "./overview-content";

// 自動更新の状態を概要の表示用の形へ変換する。時刻は描画のたびに現在時刻を使う。
export function LiveOverview(): ReactElement {
  const { state, disconnected } = useLiveState();
  return <OverviewContent view={buildOverviewView(state, new Date())} disconnected={disconnected} />;
}

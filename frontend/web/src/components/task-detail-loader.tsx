import type { ReactElement } from "react";
import { notFound } from "next/navigation";
import {
  BackendUnavailableError,
  backendUnavailableSituation,
  getBackendClient,
  TaskNotFoundError,
} from "../grpc/client";
import type { GetTaskDetailResponse } from "../proto/taktdash";
import { createLastLoaded } from "../state/last-loaded";
import { BackendUnavailableNotice } from "./backend-unavailable-notice";
import { LiveTaskDetail } from "./live-task-detail";

type TaskDetailLoaderProps = {
  params: Promise<{ project: string }>;
  searchParams: Promise<{ task?: string | string[]; goal?: string | string[] }>;
};

type LoadResult =
  | { kind: "found"; detail: GetTaskDetailResponse }
  | { kind: "unavailable"; error: BackendUnavailableError };

const firstValue = (value: string | string[] | undefined): string | undefined =>
  Array.isArray(value) ? value[0] : value;

// タスクが見つからないときと、task が未指定・空文字・空白のみのときは 404 を返す。
// gRPC の NOT_FOUND と応答の taskName・project・
// goalId の不一致は、どれも TaskNotFoundError として扱う。backend 自体に接続できない
// ときはページを落とさず、接続できない旨を表示する。前に読めたタスクなら、見出しと
// 内容を残してその上に切断の通知を出す（issue #309）。通知は 1 秒更新の表示が持ち、
// 次の取得が成功したら消す。
const lastTaskDetails = createLastLoaded<GetTaskDetailResponse>();

export async function TaskDetailLoader({ params, searchParams }: TaskDetailLoaderProps): Promise<ReactElement> {
  const { project } = await params;
  const query = await searchParams;
  const taskName = firstValue(query.task);
  const goalId = firstValue(query.goal);
  if (taskName === undefined || taskName.trim() === "") {
    notFound();
  }
  const client = getBackendClient();
  let result: LoadResult;
  try {
    result = { kind: "found", detail: await client.getTaskDetail(project, taskName, goalId) };
  } catch (error) {
    if (error instanceof TaskNotFoundError) {
      notFound();
    }
    if (error instanceof BackendUnavailableError) {
      result = { kind: "unavailable", error };
    } else {
      throw error;
    }
  }
  // goalId の有無で別のタスクになるため、未指定と空文字を区別してキーに含める。
  const key = JSON.stringify([project, taskName, goalId ?? null]);
  if (result.kind === "unavailable") {
    const last = lastTaskDetails.recall(key);
    if (last === undefined) {
      return <BackendUnavailableNotice situation={backendUnavailableSituation(result.error)} />;
    }
    return (
      <LiveTaskDetail initialDetail={last} project={project} taskName={taskName} goalId={goalId} initialDisconnected />
    );
  }
  lastTaskDetails.remember(key, result.detail);
  return (
    <LiveTaskDetail
      initialDetail={result.detail}
      project={project}
      taskName={taskName}
      goalId={goalId}
    />
  );
}

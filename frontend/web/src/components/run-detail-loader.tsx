import type { ReactElement } from "react";
import { notFound } from "next/navigation";
import {
  BackendUnavailableError,
  backendUnavailableSituation,
  getBackendClient,
  RunNotFoundError,
} from "../grpc/client";
import type { GetRunDetailResponse } from "../proto/taktdash";
import { createLastLoaded } from "../state/last-loaded";
import { BackendUnavailableOver } from "./backend-unavailable-notice";
import { loadServerState } from "../state/server-state";
import { RunDetailContent } from "./run-detail-content";

type RunDetailLoaderProps = {
  params: Promise<{ slug: string }>;
};

type LoadResult =
  | { kind: "found"; detail: GetRunDetailResponse }
  | { kind: "unavailable"; error: BackendUnavailableError };

// run が見つからないときは 404 を返す（order.md:27）。gRPC の NOT_FOUND と
// 応答の runSlug 不一致は、どちらも RunNotFoundError として扱う。backend 自体に
// 接続できないときはページを落とさず、接続できない旨を表示する（order.md:21）。
// 前に読めた run なら、見出しと内容を残してその上に通知を出す（issue #309）。
const lastRunDetails = createLastLoaded<GetRunDetailResponse>();

export async function RunDetailLoader({ params }: RunDetailLoaderProps): Promise<ReactElement> {
  const { slug } = await params;
  let result: LoadResult;
  try {
    result = { kind: "found", detail: await getBackendClient().getRunDetail(slug) };
  } catch (error) {
    if (error instanceof RunNotFoundError) {
      notFound();
    }
    if (error instanceof BackendUnavailableError) {
      result = { kind: "unavailable", error };
    } else {
      throw error;
    }
  }
  if (result.kind === "unavailable") {
    const last = lastRunDetails.recall(slug);
    return (
      <BackendUnavailableOver situation={backendUnavailableSituation(result.error)}>
        {last === undefined ? null : <RunDetailContent detail={last} now={new Date()} />}
      </BackendUnavailableOver>
    );
  }
  lastRunDetails.remember(slug, result.detail);
  let serverName: string | undefined;
  try {
    const loaded = await loadServerState();
    serverName = loaded.state.servers.find((server) => server.serverId === result.detail.serverId)?.name;
  } catch {
    serverName = undefined;
  }
  return (
    <BackendUnavailableOver>
      <RunDetailContent detail={result.detail} now={new Date()} serverName={serverName} />
    </BackendUnavailableOver>
  );
}

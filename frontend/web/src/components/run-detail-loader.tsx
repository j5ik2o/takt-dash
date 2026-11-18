import type { ReactElement } from "react";
import { notFound } from "next/navigation";
import {
  BackendUnavailableError,
  backendUnavailableSituation,
  getBackendClient,
  RunNotFoundError,
} from "../grpc/client";
import type { GetRunDetailResponse } from "../proto/taktdash";
import { BackendUnavailableNotice } from "./backend-unavailable-notice";
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
    return <BackendUnavailableNotice situation={backendUnavailableSituation(result.error)} />;
  }
  return <RunDetailContent detail={result.detail} now={new Date()} />;
}

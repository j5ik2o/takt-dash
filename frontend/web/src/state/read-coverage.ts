import type { Project, Server } from "../proto/taktdash";

export type ReadCoverage = "unread" | "partial" | "complete";

export function readCoverage(sources: readonly Pick<Server, "lastReadAt">[], snapshotLoaded = false): ReadCoverage {
  if (sources.length === 0) {
    return snapshotLoaded ? "complete" : "unread";
  }
  const read = sources.filter((source) => source.lastReadAt !== undefined).length;
  if (read === 0) {
    return "unread";
  }
  return read === sources.length ? "complete" : "partial";
}

export function readCountText(count: number, coverage: ReadCoverage): string {
  switch (coverage) {
    case "unread":
      return "不明";
    case "partial":
      return `読めた分 ${count}件`;
    case "complete":
      return `${count}件`;
  }
}

// 応答のないサーバー・プロジェクトから最後に読めた時刻のうち、最も古いものを集計の基準にする。
// 1件も読めた時刻が無ければ undefined のままにし、実在しない時刻を補わない。
const earliestReadAt = (sources: ReadonlyArray<Date | undefined>): Date | undefined => {
  const times = sources
    .filter((value): value is Date => value !== undefined)
    .map((value) => value.getTime());
  return times.length === 0 ? undefined : new Date(Math.min(...times));
};

export function partialReadSummary(
  projects: readonly Project[],
  unresponsiveServers: readonly Server[],
  unresponsiveProjects: readonly Project[],
): {
  partialReadSubject: "サーバー" | "プロジェクト" | "サーバーとプロジェクト" | undefined;
  partialReadAt: Date | undefined;
} {
  const supplyingServerIds = new Set(projects.map((project) => project.serverId));
  const unresponsiveSuppliers = unresponsiveServers.filter((server) =>
    supplyingServerIds.has(server.serverId),
  );
  const partialReadSubject = unresponsiveSuppliers.length > 0
    ? unresponsiveProjects.length > 0 ? "サーバーとプロジェクト" : "サーバー"
    : unresponsiveProjects.length > 0 ? "プロジェクト" : undefined;
  const partialReadAt = earliestReadAt(
    [...unresponsiveSuppliers, ...unresponsiveProjects].map((item) => item.lastReadAt),
  );
  return { partialReadSubject, partialReadAt };
}

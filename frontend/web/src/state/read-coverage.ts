import type { Server } from "../proto/taktdash";

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

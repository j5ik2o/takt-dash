import { Timestamp } from "../proto/google/protobuf/timestamp";

export const toDate = (value: unknown): Date | undefined => {
  if (value === undefined || value === null) {
    return undefined;
  }
  if (value instanceof Date) {
    return value;
  }
  const timestamp = Timestamp.fromJSON(value);
  return new Date(timestamp.seconds * 1_000 + Math.round(timestamp.nanos / 1_000_000));
};

export const recordOf = (value: unknown): Record<string, unknown> =>
  value !== null && typeof value === "object" ? (value as Record<string, unknown>) : {};

export const asOptionalNumber = (value: unknown): number | undefined => (typeof value === "number" ? value : undefined);

export const asOptionalString = (value: unknown): string | undefined =>
  typeof value === "string" && value !== "" ? value : undefined;

export const arrayOf = (value: unknown): Record<string, unknown>[] =>
  Array.isArray(value) ? value.map((item) => item as Record<string, unknown>) : [];

export const toStringArray = (value: unknown): string[] =>
  Array.isArray(value) ? value.map((item) => String(item)) : [];

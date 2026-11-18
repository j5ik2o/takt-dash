// 画面側のドメインプリミティブ（値オブジェクト）。
//
// 各型は size → lexical content → syntax → semantics の順で検証し、検証を通らない値からは
// 生成できません。branded type にして、生の string・number が検証済みの値として
// そのまま流通しないようにします。参照先の存在・一意性・一致のような外部状態を要する
// 制約は型では検証せず、既存の境界に残します。

export type DomainPrimitiveStage = "size" | "lexical" | "syntax" | "semantics";

/** 値オブジェクトの生成に失敗した段と対象の値。 */
export class DomainPrimitiveError extends Error {
  readonly stage: DomainPrimitiveStage;
  readonly field: string;

  constructor(stage: DomainPrimitiveStage, field: string) {
    super(messageFor(stage, field));
    this.name = "DomainPrimitiveError";
    this.stage = stage;
    this.field = field;
  }
}

const messageFor = (stage: DomainPrimitiveStage, field: string): string => {
  switch (stage) {
    case "size":
      return `${field}が空か範囲外です`;
    case "lexical":
      return `${field}に使用できない文字が含まれています`;
    case "syntax":
      return `${field}の書式が不正です`;
    case "semantics":
      return `${field}が意味的に不正です`;
  }
};

export type ServerId = string & { readonly __brand: "ServerId" };
export type ProjectName = string & { readonly __brand: "ProjectName" };
export type GoalId = string & { readonly __brand: "GoalId" };
export type TaskName = string & { readonly __brand: "TaskName" };
export type RunSlug = string & { readonly __brand: "RunSlug" };
export type InquiryId = string & { readonly __brand: "InquiryId" };

export type PullRequestNumber = number & { readonly __brand: "PullRequestNumber" };
export type RegistrationRevision = number & { readonly __brand: "RegistrationRevision" };
export type TaskCount = number & { readonly __brand: "TaskCount" };
export type RunCount = number & { readonly __brand: "RunCount" };
export type RunLimit = number & { readonly __brand: "RunLimit" };
export type StaleMinutes = number & { readonly __brand: "StaleMinutes" };
export type SilentMinutes = number & { readonly __brand: "SilentMinutes" };

/** 検証済みの異常判定の基準。 */
export type ValidatedAnomalyThresholds = {
  pendingDecisionStaleMinutes: StaleMinutes;
  runLogSilentMinutes: SilentMinutes;
};

const ASCII_CONTROL = /[\u0000-\u001f\u007f]/;
const I32_MAX = 2_147_483_647;

// 空なら size、空白のみ・ASCII 制御文字なら lexical content で拒否する。
// 文字列以外の wire 値は lexical content の違反として拒否する。
const text = (field: string, value: unknown): string => {
  if (typeof value !== "string") {
    throw new DomainPrimitiveError(value === undefined || value === null ? "size" : "lexical", field);
  }
  if (value.length === 0) {
    throw new DomainPrimitiveError("size", field);
  }
  if (value.trim().length === 0 || ASCII_CONTROL.test(value)) {
    throw new DomainPrimitiveError("lexical", field);
  }
  return value;
};

// size（範囲）→ lexical content（有限の数値）→ syntax（整数）の順に検証する。
const boundedInteger = (field: string, value: unknown, min: number, max: number): number => {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    throw new DomainPrimitiveError("lexical", field);
  }
  if (value < min || value > max) {
    throw new DomainPrimitiveError("size", field);
  }
  if (!Number.isInteger(value)) {
    throw new DomainPrimitiveError("syntax", field);
  }
  return value;
};

export const toServerId = (value: unknown): ServerId => text("サーバー ID", value) as ServerId;

export const toProjectName = (value: unknown): ProjectName => text("プロジェクト名", value) as ProjectName;

export const toGoalId = (value: unknown): GoalId => text("ゴール ID", value) as GoalId;

export const toTaskName = (value: unknown): TaskName => text("タスク名", value) as TaskName;

export const toInquiryId = (value: unknown): InquiryId => text("inquiry ID", value) as InquiryId;

export const toRunSlug = (value: unknown): RunSlug => {
  const slug = text("run slug", value);
  if (slug.includes("/") || slug.includes("\\") || slug === "." || slug === "..") {
    throw new DomainPrimitiveError("syntax", "run slug");
  }
  return slug as RunSlug;
};

export const toPullRequestNumber = (value: unknown): PullRequestNumber =>
  boundedInteger("プルリクエスト番号", value, 1, Number.MAX_SAFE_INTEGER) as PullRequestNumber;

export const toRegistrationRevision = (value: unknown): RegistrationRevision =>
  boundedInteger("revision", value, 1, I32_MAX) as RegistrationRevision;

export const toTaskCount = (value: unknown): TaskCount =>
  boundedInteger("タスク件数", value, 0, I32_MAX) as TaskCount;

export const toRunCount = (value: unknown): RunCount =>
  boundedInteger("run 件数", value, 0, I32_MAX) as RunCount;

export const toRunLimit = (value: unknown): RunLimit =>
  boundedInteger("run の上限", value, 0, I32_MAX) as RunLimit;

export const toStaleMinutes = (value: unknown): StaleMinutes =>
  boundedInteger("滞留の閾値", value, 0, I32_MAX) as StaleMinutes;

export const toSilentMinutes = (value: unknown): SilentMinutes =>
  boundedInteger("無音の閾値", value, 0, I32_MAX) as SilentMinutes;

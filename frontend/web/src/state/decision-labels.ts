// 判断待ちの識別子と表示ラベルの組み立て。
//
// 一覧の React key、操作結果の項目名、操作要素の対象説明は同じ識別子を使う。
// 種別を増やすときの更新をここだけに閉じ込め、表示のずれと key の重複を防ぐ。
import type { PendingDecision } from "../proto/taktdash";
import { INQUIRY_KIND, PENDING_DECISION_TYPE } from "./proto-values";

// 判断待ちの本文の表示。backend は takt の応答の印から切り詰めを判定して bodyTruncated に
// 載せ、body 自体には省略記号を混ぜない（issue #230）。切ってある本文だけ、末尾の空白を
// 除いた最後の段落の末尾に省略記号を1つ付ける。
export function decisionDisplayBody(decision: Pick<PendingDecision, "body" | "bodyTruncated">): string {
  return decision.bodyTruncated ? `${decision.body.trimEnd()}…` : decision.body;
}

const inquiryKindLabel = (kind: number): string =>
  kind === INQUIRY_KIND.OPERATION_REQUEST ? "作業の依頼" : "質問";

// 種別ごとの識別子。SSE の再描画で並び順が変わっても、同じ判断待ちに同じ結果を
// 紐づけられるよう、種別と識別子から作る。問い合わせは同じ inquiryId でもゴールが
// 異なり得るため、ゴールの区画も含める。ゴールが無い場合は空の区画にする。
type DecisionKeyBuilder = (decision: PendingDecision) => string;

const DECISION_KEY_BY_TYPE: Partial<Record<number, DecisionKeyBuilder>> = {
  [PENDING_DECISION_TYPE.AWAITING_MERGE]: (decision) =>
    `merge:${decision.serverId}:${decision.project}:${decision.goalId ?? ""}`,
  [PENDING_DECISION_TYPE.GOAL_REGISTRATION]: (decision) =>
    `goal-registration:${decision.serverId}:${decision.project}:${decision.revision ?? ""}`,
  [PENDING_DECISION_TYPE.INQUIRY]: (decision) =>
    `inquiry:${decision.serverId}:${decision.project}:${decision.goalId ?? ""}:${decision.inquiryId}`,
};

export function decisionKey(decision: PendingDecision): string {
  const build = DECISION_KEY_BY_TYPE[decision.type];
  return build === undefined
    ? `pending:${decision.serverId}:${decision.project}:${decision.type}`
    : build(decision);
}

type DecisionTypeLabelResolver = (decision: PendingDecision) => string;

const DECISION_TYPE_LABEL_BY_TYPE: Partial<Record<number, DecisionTypeLabelResolver>> = {
  [PENDING_DECISION_TYPE.AWAITING_MERGE]: () => "人のマージ待ち",
  [PENDING_DECISION_TYPE.GOAL_REGISTRATION]: () => "ゴール登録の承認待ち",
  [PENDING_DECISION_TYPE.INQUIRY]: (decision) => `問い合わせ（${inquiryKindLabel(decision.inquiryKind)}）`,
};

export function decisionTypeLabel(decision: PendingDecision): string {
  const resolve = DECISION_TYPE_LABEL_BY_TYPE[decision.type];
  return resolve === undefined ? "判断待ち" : resolve(decision);
}

// 操作結果に出す項目名と、操作要素の accessible name・入力欄の label の共通部分。
// 基本ラベル（サーバー・プロジェクト・種別）と、種別ごとの識別子を一箇所で作る。
// 種別を増やすときの更新をここだけに閉じ込め、2つの表示がずれないようにする。
type DecisionLabelParts = {
  base: string;
  identifier: string;
};

const decisionLabelIdentifier = (decision: PendingDecision): string => {
  switch (decision.type) {
    case PENDING_DECISION_TYPE.AWAITING_MERGE:
      return `PR #${decision.pullRequest?.number ?? "不明"}`;
    case PENDING_DECISION_TYPE.GOAL_REGISTRATION:
      return `revision ${decision.revision ?? "不明"}`;
    case PENDING_DECISION_TYPE.INQUIRY:
      return `ゴール ${decision.goalId ?? "不明"} ${decision.inquiryId}`;
    default:
      return "";
  }
};

const decisionLabelParts = (decision: PendingDecision): DecisionLabelParts => ({
  base: `${decision.serverId} / ${decision.project} の${decisionTypeLabel(decision)}`,
  identifier: decisionLabelIdentifier(decision),
});

const composeDecisionLabel = ({ base, identifier }: DecisionLabelParts): string =>
  identifier === "" ? base : `${base} ${identifier}`;

export function decisionLabel(decision: PendingDecision): string {
  return composeDecisionLabel(decisionLabelParts(decision));
}

// 操作ボタンの説明参照（aria-describedby）と入力欄の label に使う。一覧は全サーバー・全
// プロジェクトの判断待ちの和集合なので、同じ固定名の操作要素が複数並ぶ。種別と行固有の
// 識別子を含めて対象を一意に識別できるようにする。問い合わせだけは本文も添えて区別する。
// 操作ボタンの読み上げ名は操作の文言だけにし、この値はボタン外の説明要素から補う（issue #324）。
// 入力欄は対象を識別できる名前として、この値を aria-label に含める。
export function decisionTargetLabel(decision: PendingDecision): string {
  const parts = decisionLabelParts(decision);
  if (decision.type === PENDING_DECISION_TYPE.INQUIRY) {
    return `${parts.base} ${parts.identifier}（${decisionDisplayBody(decision)}）`;
  }
  return composeDecisionLabel(parts);
}

// 操作ボタンの説明要素に出す文面。判断待ちの項目にはサーバーの ID も表示名も出さない
// 既存契約があるため、対象の識別に必要なプロジェクト・種別・行固有の識別子だけを使う。
export function decisionTargetDescription(decision: PendingDecision): string {
  const parts = decisionLabelParts(decision);
  const base = `${decision.project} の${decisionTypeLabel(decision)}`;
  if (decision.type === PENDING_DECISION_TYPE.INQUIRY) {
    return `${base} ${parts.identifier}（${decisionDisplayBody(decision)}）`;
  }
  return parts.identifier === "" ? base : `${base} ${parts.identifier}`;
}

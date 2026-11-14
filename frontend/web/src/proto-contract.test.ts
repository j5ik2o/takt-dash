// proto/ から生成した TypeScript のクライアントへ型が揃っているかを、型検査で確かめる。
//
// このファイルは `tsc --noEmit`（make done の typecheck）で検査される。生成コードが
// 配線されていない、または proto の型と食い違う状態を、型エラーとして失敗させる。
// 画面と Route Handler は後続のタスクのため、ここでは型の存在と配線だけを観測する。
import {
  type DashboardState,
  type GetRunDetailRequest,
  type PendingDecision,
  PendingDecisionType,
} from "./proto/taktdash";

export type ProtoContract = {
  state: DashboardState;
  inquiry: PendingDecision;
  runDetail: GetRunDetailRequest;
};

export const pendingDecisionTypes: PendingDecisionType[] = [
  PendingDecisionType.PENDING_DECISION_TYPE_UNSPECIFIED,
  PendingDecisionType.PENDING_DECISION_TYPE_INQUIRY,
  PendingDecisionType.PENDING_DECISION_TYPE_AWAITING_MERGE,
];

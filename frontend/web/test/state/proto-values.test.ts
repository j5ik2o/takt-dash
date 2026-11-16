// proto-values.ts の数値が生成コードの enum と一致することを確かめる。
//
// クライアントコンポーネントは gRPC をブラウザへ持ち込まないため生成コードを値として
// 読み込めない。この定数が proto と食い違うと画面の分岐（滞留・run の状態・種類の表示）が
// 誤るため、生成コードの enum と突き合わせて検出する。
import test from "node:test";
import assert from "node:assert/strict";
import {
  ActivityKind,
  ConversationSpeaker,
  ExecutionStatus,
  GoalStatus,
  InquiryKind,
  OperationAnswerReport,
  PendingDecisionType,
  RunStatus,
  TaskStatus,
  conversationSpeakerFromJSON,
  executionStatusFromJSON,
  goalStatusFromJSON,
  inquiryKindFromJSON,
  pendingDecisionTypeFromJSON,
  runStatusFromJSON,
  taskStatusFromJSON,
} from "../../src/proto/taktdash.ts";
import {
  ACTIVITY_KIND,
  CONVERSATION_SPEAKER,
  EXECUTION_STATUS,
  GOAL_STATUS,
  INQUIRY_KIND,
  OPERATION_ANSWER_REPORT,
  PENDING_DECISION_TYPE,
  RUN_STATUS,
  enumFromWire,
  enumFromWireOrUnrecognizedName,
} from "../../src/state/proto-values.ts";
import { DomainPrimitiveError } from "../../src/state/domain-primitives.ts";

test("proto-values は生成コードの enum と一致する", () => {
  assert.equal(GOAL_STATUS.CREATED, GoalStatus.GOAL_STATUS_CREATED);
  assert.equal(GOAL_STATUS.AWAITING_MERGE, GoalStatus.GOAL_STATUS_AWAITING_MERGE);
  assert.equal(GOAL_STATUS.COMPLETED, GoalStatus.GOAL_STATUS_COMPLETED);

  assert.equal(EXECUTION_STATUS.ACTIVE, ExecutionStatus.EXECUTION_STATUS_ACTIVE);
  assert.equal(EXECUTION_STATUS.PAUSED, ExecutionStatus.EXECUTION_STATUS_PAUSED);
  assert.equal(EXECUTION_STATUS.ABORTED, ExecutionStatus.EXECUTION_STATUS_ABORTED);

  assert.equal(RUN_STATUS.RUNNING, RunStatus.RUN_STATUS_RUNNING);
  assert.equal(RUN_STATUS.COMPLETED, RunStatus.RUN_STATUS_COMPLETED);
  assert.equal(RUN_STATUS.ABORTED, RunStatus.RUN_STATUS_ABORTED);
  assert.equal(RUN_STATUS.FAILED, RunStatus.RUN_STATUS_FAILED);

  assert.equal(ACTIVITY_KIND.GOAL_CREATED, ActivityKind.ACTIVITY_KIND_GOAL_CREATED);
  assert.equal(ACTIVITY_KIND.RUN_STARTED, ActivityKind.ACTIVITY_KIND_RUN_STARTED);
  assert.equal(ACTIVITY_KIND.TASK_COMPLETED, ActivityKind.ACTIVITY_KIND_TASK_COMPLETED);
  assert.equal(ACTIVITY_KIND.INQUIRY, ActivityKind.ACTIVITY_KIND_INQUIRY);
  assert.equal(ACTIVITY_KIND.AWAITING_MERGE, ActivityKind.ACTIVITY_KIND_AWAITING_MERGE);
  assert.equal(ACTIVITY_KIND.PAUSED, ActivityKind.ACTIVITY_KIND_PAUSED);
  assert.equal(ACTIVITY_KIND.BLOCKED, ActivityKind.ACTIVITY_KIND_BLOCKED);
  assert.equal(ACTIVITY_KIND.CONNECTION_LOST, ActivityKind.ACTIVITY_KIND_CONNECTION_LOST);

  assert.equal(INQUIRY_KIND.QUESTION, InquiryKind.INQUIRY_KIND_QUESTION);
  assert.equal(INQUIRY_KIND.OPERATION_REQUEST, InquiryKind.INQUIRY_KIND_OPERATION_REQUEST);

  assert.equal(OPERATION_ANSWER_REPORT.COMPLETED, OperationAnswerReport.OPERATION_ANSWER_REPORT_COMPLETED);
  assert.equal(OPERATION_ANSWER_REPORT.UNABLE, OperationAnswerReport.OPERATION_ANSWER_REPORT_UNABLE);

  assert.equal(PENDING_DECISION_TYPE.INQUIRY, PendingDecisionType.PENDING_DECISION_TYPE_INQUIRY);
  assert.equal(PENDING_DECISION_TYPE.AWAITING_MERGE, PendingDecisionType.PENDING_DECISION_TYPE_AWAITING_MERGE);
  assert.equal(
    PENDING_DECISION_TYPE.GOAL_REGISTRATION,
    PendingDecisionType.PENDING_DECISION_TYPE_GOAL_REGISTRATION,
  );

  assert.equal(CONVERSATION_SPEAKER.USER, ConversationSpeaker.CONVERSATION_SPEAKER_USER);
  assert.equal(CONVERSATION_SPEAKER.MANAGER, ConversationSpeaker.CONVERSATION_SPEAKER_MANAGER);
  assert.equal(CONVERSATION_SPEAKER.SYSTEM, ConversationSpeaker.CONVERSATION_SPEAKER_SYSTEM);
});

test("enumFromWire は生成コードの名称→数値の対応と一致する（受信した toJSON の列挙型を復元する）", () => {
  const cases: Array<{ wire: Parameters<typeof enumFromWire>[0]; generated: Record<string, number | string>; fromJSON: (v: unknown) => number }> = [
    { wire: "GoalStatus", generated: GoalStatus, fromJSON: goalStatusFromJSON },
    { wire: "ExecutionStatus", generated: ExecutionStatus, fromJSON: executionStatusFromJSON },
    { wire: "RunStatus", generated: RunStatus, fromJSON: runStatusFromJSON },
    { wire: "TaskStatus", generated: TaskStatus, fromJSON: taskStatusFromJSON },
    { wire: "InquiryKind", generated: InquiryKind, fromJSON: inquiryKindFromJSON },
    { wire: "PendingDecisionType", generated: PendingDecisionType, fromJSON: pendingDecisionTypeFromJSON },
    { wire: "ConversationSpeaker", generated: ConversationSpeaker, fromJSON: conversationSpeakerFromJSON },
  ];
  for (const { wire, generated, fromJSON } of cases) {
    // 生成 enum の各名称（数値キーは除外）について、名称文字列を渡したときの復元値が
    // 生成コードの fromJSON と一致すること。UNRECOGNIZED は不明値のため受理しない。
    for (const name of Object.keys(generated)) {
      if (/^-?\d+$/.test(name) || name === "UNRECOGNIZED") {
        continue;
      }
      assert.equal(enumFromWire(wire, name), fromJSON(name), `${wire}.${name}`);
    }
  }
});

test("enumFromWire は宣言済みの名称と数値だけを受け、欠落は UNSPECIFIED にする", () => {
  assert.equal(enumFromWire("RunStatus", "RUN_STATUS_UNSPECIFIED"), 0);
  assert.equal(enumFromWire("RunStatus", "RUN_STATUS_FAILED"), 4);
  assert.equal(enumFromWire("RunStatus", 1), 1);
  assert.equal(enumFromWire("RunStatus", 4), 4);
  assert.equal(enumFromWire("RunStatus", undefined), 0);
  assert.equal(enumFromWire("RunStatus", null), 0);
});

test("enumFromWire は未知の名称・未知の数値・UNRECOGNIZED を semantics で拒否する", () => {
  const reject = (value: unknown): void => {
    assert.throws(
      () => enumFromWire("RunStatus", value),
      (error) => error instanceof DomainPrimitiveError && error.stage === "semantics",
    );
  };

  reject("RUN_STATUS_UNKNOWN_X");
  reject("UNRECOGNIZED");
  reject(99);
  reject(-1);
});

test("enumFromWireOrUnrecognizedName は対象外の列挙の不明挙動を保つ", () => {
  assert.equal(enumFromWireOrUnrecognizedName("InquiryKind", "INQUIRY_KIND_QUESTION"), 1);
  assert.equal(enumFromWireOrUnrecognizedName("InquiryKind", "INQUIRY_KIND_UNKNOWN_X"), -1);
  assert.equal(enumFromWireOrUnrecognizedName("InquiryKind", undefined), -1);
  assert.equal(enumFromWireOrUnrecognizedName("InquiryKind", 99), 99);

  assert.equal(enumFromWireOrUnrecognizedName("ConversationSpeaker", "CONVERSATION_SPEAKER_USER"), 1);
  assert.equal(enumFromWireOrUnrecognizedName("ConversationSpeaker", "CONVERSATION_SPEAKER_UNKNOWN_X"), -1);
  assert.equal(enumFromWireOrUnrecognizedName("ConversationSpeaker", undefined), -1);
  assert.equal(enumFromWireOrUnrecognizedName("ConversationSpeaker", 99), 99);
});

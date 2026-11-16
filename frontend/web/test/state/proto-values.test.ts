// proto-values.ts の数値が生成コードの enum と一致することを確かめる。
//
// クライアントコンポーネントは gRPC をブラウザへ持ち込まないため生成コードを値として
// 読み込めない。この定数が proto と食い違うと画面の分岐（滞留・run の状態・種類の表示）が
// 誤るため、生成コードの enum と突き合わせて検出する。
import test from "node:test";
import assert from "node:assert/strict";
import {
  ActivityKind,
  ConnectionState,
  ConversationSpeaker,
  ExecutionStatus,
  GoalStatus,
  InquiryKind,
  OperationAnswerReport,
  PendingDecisionType,
  RunStatus,
  TaskStatus,
  connectionStateFromJSON,
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
  CONNECTION_STATE,
  CONVERSATION_SPEAKER,
  EXECUTION_STATUS,
  GOAL_STATUS,
  INQUIRY_KIND,
  OPERATION_ANSWER_REPORT,
  PENDING_DECISION_TYPE,
  RUN_STATUS,
  TASK_STATUS,
  enumFromWire,
} from "../../src/state/proto-values.ts";

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

  assert.equal(TASK_STATUS.PENDING, TaskStatus.TASK_STATUS_PENDING);
  assert.equal(TASK_STATUS.RUNNING, TaskStatus.TASK_STATUS_RUNNING);
  assert.equal(TASK_STATUS.COMPLETED, TaskStatus.TASK_STATUS_COMPLETED);
  assert.equal(TASK_STATUS.FAILED, TaskStatus.TASK_STATUS_FAILED);
  assert.equal(TASK_STATUS.EXCEEDED, TaskStatus.TASK_STATUS_EXCEEDED);
  assert.equal(TASK_STATUS.PR_FAILED, TaskStatus.TASK_STATUS_PR_FAILED);

  assert.equal(ACTIVITY_KIND.GOAL_CREATED, ActivityKind.ACTIVITY_KIND_GOAL_CREATED);
  assert.equal(ACTIVITY_KIND.RUN_STARTED, ActivityKind.ACTIVITY_KIND_RUN_STARTED);
  assert.equal(ACTIVITY_KIND.TASK_COMPLETED, ActivityKind.ACTIVITY_KIND_TASK_COMPLETED);
  assert.equal(ACTIVITY_KIND.INQUIRY, ActivityKind.ACTIVITY_KIND_INQUIRY);
  assert.equal(ACTIVITY_KIND.AWAITING_MERGE, ActivityKind.ACTIVITY_KIND_AWAITING_MERGE);
  assert.equal(ACTIVITY_KIND.PAUSED, ActivityKind.ACTIVITY_KIND_PAUSED);
  assert.equal(ACTIVITY_KIND.BLOCKED, ActivityKind.ACTIVITY_KIND_BLOCKED);
  assert.equal(ACTIVITY_KIND.CONNECTION_LOST, ActivityKind.ACTIVITY_KIND_CONNECTION_LOST);
  assert.equal(ACTIVITY_KIND.CONNECTION_RESTORED, ActivityKind.ACTIVITY_KIND_CONNECTION_RESTORED);

  assert.equal(CONNECTION_STATE.CONNECTED, ConnectionState.CONNECTION_STATE_CONNECTED);
  assert.equal(CONNECTION_STATE.DELAYED, ConnectionState.CONNECTION_STATE_DELAYED);
  assert.equal(CONNECTION_STATE.UNRESPONSIVE, ConnectionState.CONNECTION_STATE_UNRESPONSIVE);

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
    { wire: "ConnectionState", generated: ConnectionState, fromJSON: connectionStateFromJSON },
    { wire: "ConversationSpeaker", generated: ConversationSpeaker, fromJSON: conversationSpeakerFromJSON },
  ];
  for (const { wire, generated, fromJSON } of cases) {
    // 生成 enum の各名称（数値キーは除外）について、名称文字列を渡したときの復元値が
    // 生成コードの fromJSON と一致すること。
    for (const name of Object.keys(generated)) {
      if (/^\d+$/.test(name)) {
        continue;
      }
      assert.equal(enumFromWire(wire, name), fromJSON(name), `${wire}.${name}`);
    }
  }
  // 未知の文字列と欠落は UNRECOGNIZED(-1)。数値はそのまま通す。
  assert.equal(enumFromWire("RunStatus", "RUN_STATUS_UNKNOWN_X"), -1);
  assert.equal(enumFromWire("RunStatus", undefined), -1);
  assert.equal(enumFromWire("RunStatus", 1), 1);
});

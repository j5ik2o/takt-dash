// proto/taktdash.proto の enum 値を、クライアントコンポーネントが数値として比較するための定数。
//
// 生成コード（src/proto/taktdash.ts）は @grpc/grpc-js を値として読み込むため、
// これをブラウザ向けビルドへ持ち込むとネイティブ依存の解決に失敗する。BFF 構成では
// ブラウザは backend へ直接接続しないため、表示の分岐に必要な enum の数値だけをここに置く。
// 値が生成コードと一致することは test/state/proto-values.test.ts で検査する。
export const GOAL_STATUS = {
  CREATED: 1,
  AWAITING_MERGE: 2,
  COMPLETED: 3,
} as const;

export const EXECUTION_STATUS = {
  ACTIVE: 1,
  PAUSED: 2,
  ABORTED: 3,
} as const;

export const RUN_STATUS = {
  RUNNING: 1,
  COMPLETED: 2,
  ABORTED: 3,
  FAILED: 4,
} as const;

export const TASK_STATUS = {
  PENDING: 1,
  RUNNING: 2,
  COMPLETED: 3,
  FAILED: 4,
  EXCEEDED: 5,
  PR_FAILED: 6,
} as const;

export const ACTIVITY_KIND = {
  GOAL_CREATED: 1,
  RUN_STARTED: 2,
  TASK_COMPLETED: 3,
  INQUIRY: 4,
  AWAITING_MERGE: 5,
  PAUSED: 6,
  BLOCKED: 7,
  CONNECTION_LOST: 8,
  CONNECTION_RESTORED: 9,
} as const;

export const CONNECTION_STATE = {
  CONNECTED: 1,
  DELAYED: 2,
  UNRESPONSIVE: 3,
} as const;

export const INQUIRY_KIND = {
  QUESTION: 1,
  OPERATION_REQUEST: 2,
} as const;

// operation_request への回答。画面の「実施した」「対応できない」が送る値。
export const OPERATION_ANSWER_REPORT = {
  COMPLETED: 1,
  UNABLE: 2,
} as const;

export const PENDING_DECISION_TYPE = {
  INQUIRY: 1,
  AWAITING_MERGE: 2,
  GOAL_REGISTRATION: 3,
} as const;

export const CONVERSATION_SPEAKER = {
  USER: 1,
  MANAGER: 2,
  SYSTEM: 3,
} as const;

// proto の JSON 表現（DashboardState.toJSON）は列挙型を名称文字列で出す。受信側で
// 数値へ戻すため、生成コードの *FromJSON と同じ対応を名称→数値で持つ。生成コードと
// 一致することは test/state/proto-values.test.ts で検査する。
const ENUM_NAME_TO_NUMBER: Record<string, Record<string, number>> = {
  GoalStatus: {
    GOAL_STATUS_UNSPECIFIED: 0,
    GOAL_STATUS_CREATED: 1,
    GOAL_STATUS_AWAITING_MERGE: 2,
    GOAL_STATUS_COMPLETED: 3,
    UNRECOGNIZED: -1,
  },
  ExecutionStatus: {
    EXECUTION_STATUS_UNSPECIFIED: 0,
    EXECUTION_STATUS_ACTIVE: 1,
    EXECUTION_STATUS_PAUSED: 2,
    EXECUTION_STATUS_ABORTED: 3,
    UNRECOGNIZED: -1,
  },
  RunStatus: {
    RUN_STATUS_UNSPECIFIED: 0,
    RUN_STATUS_RUNNING: 1,
    RUN_STATUS_COMPLETED: 2,
    RUN_STATUS_ABORTED: 3,
    RUN_STATUS_FAILED: 4,
    UNRECOGNIZED: -1,
  },
  TaskStatus: {
    TASK_STATUS_UNSPECIFIED: 0,
    TASK_STATUS_PENDING: 1,
    TASK_STATUS_RUNNING: 2,
    TASK_STATUS_COMPLETED: 3,
    TASK_STATUS_FAILED: 4,
    TASK_STATUS_EXCEEDED: 5,
    TASK_STATUS_PR_FAILED: 6,
    UNRECOGNIZED: -1,
  },
  InquiryKind: {
    INQUIRY_KIND_UNSPECIFIED: 0,
    INQUIRY_KIND_QUESTION: 1,
    INQUIRY_KIND_OPERATION_REQUEST: 2,
    UNRECOGNIZED: -1,
  },
  PendingDecisionType: {
    PENDING_DECISION_TYPE_UNSPECIFIED: 0,
    PENDING_DECISION_TYPE_INQUIRY: 1,
    PENDING_DECISION_TYPE_AWAITING_MERGE: 2,
    PENDING_DECISION_TYPE_GOAL_REGISTRATION: 3,
    UNRECOGNIZED: -1,
  },
  ConversationSpeaker: {
    CONVERSATION_SPEAKER_UNSPECIFIED: 0,
    CONVERSATION_SPEAKER_USER: 1,
    CONVERSATION_SPEAKER_MANAGER: 2,
    CONVERSATION_SPEAKER_SYSTEM: 3,
    UNRECOGNIZED: -1,
  },
  ConnectionState: {
    CONNECTION_STATE_UNSPECIFIED: 0,
    CONNECTION_STATE_CONNECTED: 1,
    CONNECTION_STATE_DELAYED: 2,
    CONNECTION_STATE_UNRESPONSIVE: 3,
    UNRECOGNIZED: -1,
  },
};

// 生成コードの *FromJSON と同じ扱い: 数値はそのまま、名称文字列は対応する数値、
// 未知の文字列・欠落は UNRECOGNIZED(-1) にそろえる。
export function enumFromWire(enumName: keyof typeof ENUM_NAME_TO_NUMBER, value: unknown): number {
  if (typeof value === "number") {
    return value;
  }
  if (typeof value === "string") {
    const mapped = ENUM_NAME_TO_NUMBER[enumName][value];
    return mapped === undefined ? -1 : mapped;
  }
  return -1;
}

// SSE／RSC で受け取る proto の JSON 表現を内部型へ復元する境界（C01〜C09、C11、C12）の契約。
//
// 対象フィールドは専用型の検証を通し、不正値は状態を返す前に拒否する。外部状態を要する
// 意味制約（一覧に存在する・対応が設定にある・保留中に一致する）は型へ押し込まず、
// 復元の境界では構文上正しい値をそのまま通す。PR URL・自由文・Date は生のまま保つ。
import test from "node:test";
import assert from "node:assert/strict";
import {
  DashboardState,
  GetGoalDetailResponse,
  GetTaskDetailResponse,
  GoalStatus,
  InquiryKind,
  PendingDecisionType,
  RunStatus,
  TaskStatus,
} from "../../src/proto/taktdash.ts";
import {
  createActionableDashboardState,
  createDashboardState,
  createGoalDetailResponse,
  createTaskDetailResponse,
  NOW,
} from "../support/fixtures.ts";
import {
  dashboardStateFromProto,
  restoreDashboardState,
  restoreGoalDetail,
  restoreTaskDetail,
} from "../../src/state/restore-state.ts";
import { DomainPrimitiveError } from "../../src/state/domain-primitives.ts";

type Stage = "size" | "lexical" | "syntax" | "semantics";

const isStage = (stage: Stage) => (error: unknown): boolean =>
  error instanceof DomainPrimitiveError && error.stage === stage;

const restoreError = (payload: unknown, stage: Stage): void => {
  assert.throws(() => restoreDashboardState(payload), isStage(stage));
};

const wireOf = (state: DashboardState): unknown => JSON.parse(JSON.stringify(DashboardState.toJSON(state)));

test("proto の完全な JSON 表現から専用型の値と既知の状態を復元する", () => {
  const restored = restoreDashboardState(wireOf(createDashboardState()));

  assert.equal(restored.servers[0]?.serverId, "mini");
  assert.equal(restored.projects[0]?.name, "corporate-site");
  assert.equal(restored.projects[0]?.serverId, "mini");

  const goal = restored.goals[0]!;
  assert.equal(goal.goalId, "goal-1");
  assert.equal(goal.serverId, "mini");
  assert.equal(goal.project, "corporate-site");
  assert.equal(goal.status, GoalStatus.GOAL_STATUS_AWAITING_MERGE);
  assert.equal(goal.status, 2);
  assert.equal(goal.pullRequest?.number, 418);

  const run = restored.runs[0]!;
  assert.equal(run.runSlug, "run-aaa");
  assert.equal(run.serverId, "mini");
  assert.equal(run.project, "corporate-site");
  assert.equal(run.goalId, "goal-1");
  assert.equal(run.taskName, "採用ページのレイアウトを直す");
  assert.equal(run.status, RunStatus.RUN_STATUS_RUNNING);

  assert.equal(restored.pendingDecisions[0]?.type, PendingDecisionType.PENDING_DECISION_TYPE_INQUIRY);
  assert.equal(restored.pendingDecisions[0]?.inquiryId, "inquiry-schedule");
  assert.equal(restored.thresholds?.pendingDecisionStaleMinutes, 240);
  assert.equal(restored.thresholds?.runLogSilentMinutes, 10);
});

test("必須の ID と名前は空を size、空白のみと制御文字を lexical で拒否する", () => {
  restoreError({ servers: [{ serverId: "" }] }, "size");
  restoreError({ servers: [{ serverId: "a\tb" }] }, "lexical");
  restoreError({ projects: [{ name: "", serverId: "mini" }] }, "size");
  restoreError({ projects: [{ name: "esa", serverId: "" }] }, "size");

  const goal = { goalId: "goal-1", serverId: "mini", project: "esa" };
  restoreError({ goals: [{ ...goal, goalId: "" }] }, "size");
  restoreError({ goals: [{ ...goal, serverId: "  " }] }, "lexical");
  restoreError({ goals: [{ ...goal, project: "esa\n" }] }, "lexical");

  const run = { runSlug: "run-1", serverId: "mini", project: "esa", goalId: "goal-1", taskName: "task" };
  restoreError({ runs: [{ ...run, runSlug: "" }] }, "size");
  restoreError({ runs: [{ ...run, taskName: "  " }] }, "lexical");
  // ゴール非所属の run は未指定として受理するが、空白のみは未指定へ丸めず lexical で拒否する。
  restoreError({ runs: [{ ...run, goalId: "  " }] }, "lexical");

  const decision = { serverId: "mini", project: "esa", inquiryId: "inquiry-1" };
  restoreError({ pendingDecisions: [{ ...decision, serverId: "" }] }, "size");
  restoreError({ pendingDecisions: [{ ...decision, inquiryId: "  " }] }, "lexical");
});

test("run slug はパス区切りと親ディレクトリを syntax で拒否する", () => {
  const run = { runSlug: "run-1", serverId: "mini", project: "esa", goalId: "goal-1", taskName: "task" };
  for (const runSlug of ["a/b", "a\\b", ".", "..", "../x"]) {
    restoreError({ runs: [{ ...run, runSlug }] }, "syntax");
  }
});

test("ゴールに属さない run は JSON 経路と proto 経路の両方でゴール ID を未指定として受理する", () => {
  const run = { runSlug: "run-1", serverId: "mini", project: "esa", taskName: "task" };

  for (const goalId of ["", undefined]) {
    const restored = restoreDashboardState({ runs: [{ ...run, goalId }] });
    assert.equal(restored.runs[0]?.goalId, undefined, `JSON 経路（goalId: ${String(goalId)}）`);
  }

  for (const goalId of ["", undefined]) {
    const restored = dashboardStateFromProto(DashboardState.create({ runs: [{ ...run, goalId }] }));
    assert.equal(restored.runs[0]?.goalId, undefined, `proto 経路（goalId: ${String(goalId)}）`);
  }
});

test("同じ入力は JSON 経路と proto 経路で同じ内部状態へ復元する", () => {
  // 対象外の InquiryKind は wire が 0 を省略するため、0 と欠落の区別が経路で揺れる。
  // ここでは対象の契約が曖昧にならないよう、明示した既知値で両経路の一致を固定する。
  const state = createDashboardState();
  const comparable = DashboardState.create({
    ...state,
    runs: [{ ...state.runs[0]!, goalId: "" }],
    pendingDecisions: state.pendingDecisions.map((decision) => ({
      ...decision,
      inquiryKind: InquiryKind.INQUIRY_KIND_QUESTION,
    })),
  });

  const fromJSON = restoreDashboardState(wireOf(comparable));
  const fromProto = dashboardStateFromProto(comparable);

  assert.deepEqual(fromJSON, fromProto);
  assert.equal(fromJSON.runs[0]?.goalId, undefined);
  assert.equal(
    fromJSON.pendingDecisions.find((decision) => decision.inquiryId === undefined)?.goalId,
    "goal-1",
  );
});

test("任意のゴール ID と inquiry ID は欠落と空文字を未指定にする", () => {
  const restored = restoreDashboardState({
    pendingDecisions: [
      {
        type: "PENDING_DECISION_TYPE_GOAL_REGISTRATION",
        serverId: "mini",
        project: "takt",
        goalId: "",
        inquiryId: "",
        revision: 7,
      },
      {
        type: "PENDING_DECISION_TYPE_INQUIRY",
        serverId: "mini",
        project: "esa",
        inquiryId: "inquiry-1",
      },
    ],
  });

  assert.equal(restored.pendingDecisions[0]?.goalId, undefined);
  assert.equal(restored.pendingDecisions[0]?.inquiryId, undefined);
  assert.equal(restored.pendingDecisions[0]?.revision, 7);
  assert.equal(restored.pendingDecisions[1]?.goalId, undefined);
  assert.equal(restored.pendingDecisions[1]?.inquiryId, "inquiry-1");
  assert.equal(restored.pendingDecisions[1]?.revision, undefined);
});

test("プルリクエスト番号と revision は大きさ・有限性・整数性を順に検証する", () => {
  const goalWithPr = (number: unknown) => ({
    goals: [
      {
        goalId: "goal-1",
        serverId: "mini",
        project: "esa",
        pullRequest: { number, url: "https://example.test/pr/1" },
      },
    ],
  });
  restoreError(goalWithPr(0), "size");
  restoreError(goalWithPr(-1), "size");
  restoreError(goalWithPr(1.5), "syntax");
  restoreError(goalWithPr("418"), "lexical");
  restoreError(goalWithPr(Number.NaN), "lexical");

  const decisionWithRevision = (revision: unknown) => ({
    pendingDecisions: [{ serverId: "mini", project: "esa", inquiryId: "inquiry-1", revision }],
  });
  restoreError(decisionWithRevision(0), "size");
  restoreError(decisionWithRevision(-1), "size");
  restoreError(decisionWithRevision(1.5), "syntax");
  restoreError(decisionWithRevision("7"), "lexical");
});

test("タスク件数は 0 以上 i32::MAX 以下で、wire が省略した 0 は 0 として復元する", () => {
  const goalWithCounts = (taskCounts: unknown) => ({
    goals: [{ goalId: "goal-1", serverId: "mini", project: "esa", taskCounts }],
  });

  restoreError(goalWithCounts({ pending: -1 }), "size");
  restoreError(goalWithCounts({ running: 2_147_483_648 }), "size");
  restoreError(goalWithCounts({ completed: 1.5 }), "syntax");

  const restored = restoreDashboardState(goalWithCounts({}));
  const counts = restored.goals[0]?.taskCounts;
  assert.equal(counts?.pending, 0);
  assert.equal(counts?.running, 0);
  assert.equal(counts?.completed, 0);
  assert.equal(counts?.failed, 0);
  assert.equal(counts?.exceeded, 0);
  assert.equal(counts?.prFailed, 0);
  assert.equal(restoreDashboardState({ goals: [{ goalId: "g", serverId: "mini", project: "esa" }] }).goals[0]?.taskCounts, undefined);
});

test("閾値は 0 以上 i32::MAX 以下で、wire が省略した 0 は 0 として復元する", () => {
  restoreError({ thresholds: { pendingDecisionStaleMinutes: -1, runLogSilentMinutes: 10 } }, "size");
  restoreError({ thresholds: { pendingDecisionStaleMinutes: 240, runLogSilentMinutes: 2_147_483_648 } }, "size");
  restoreError({ thresholds: { pendingDecisionStaleMinutes: 240, runLogSilentMinutes: 1.5 } }, "syntax");

  const restored = restoreDashboardState({ thresholds: {} });
  assert.equal(restored.thresholds?.pendingDecisionStaleMinutes, 0);
  assert.equal(restored.thresholds?.runLogSilentMinutes, 0);
});

// issue #224：プロジェクトの concurrency（実行上限）は概要の組み立てで初めて RunLimit
// として検証され、復元の境界では number のまま負数や小数が通っていた。復元の段階でも
// RunLimit と同じ検証を通し、不正な上限を画面の状態へ持ち込まないことを固定する。
test("実行上限は復元の段階で負数と整数でない値を拒否し、概要の組み立てまで進まない", () => {
  const jsonWithConcurrency = (concurrency: unknown): unknown => {
    const wire = wireOf(createDashboardState()) as { projects: { concurrency: unknown }[] };
    wire.projects[0]!.concurrency = concurrency;
    return wire;
  };
  const protoWithConcurrency = (concurrency: number): DashboardState => {
    const state = createDashboardState();
    return DashboardState.create({
      ...state,
      projects: [{ ...state.projects[0]!, concurrency }],
    });
  };

  for (const concurrency of [-1, -4]) {
    const rejection = (error: unknown): boolean =>
      error instanceof DomainPrimitiveError && error.stage === "size" && error.field === "run の上限";
    assert.throws(
      () => restoreDashboardState(jsonWithConcurrency(concurrency)),
      rejection,
      `JSON 経路の負の実行上限 ${concurrency} は復元の段階で拒む`,
    );
    assert.throws(
      () => dashboardStateFromProto(protoWithConcurrency(concurrency)),
      rejection,
      `proto 経路の負の実行上限 ${concurrency} は復元の段階で拒む`,
    );
  }

  for (const concurrency of [1.5, 0.5]) {
    const rejection = (error: unknown): boolean =>
      error instanceof DomainPrimitiveError && error.stage === "syntax" && error.field === "run の上限";
    assert.throws(
      () => restoreDashboardState(jsonWithConcurrency(concurrency)),
      rejection,
      `JSON 経路の整数でない実行上限 ${concurrency} は復元の段階で拒む`,
    );
    assert.throws(
      () => dashboardStateFromProto(protoWithConcurrency(concurrency)),
      rejection,
      `proto 経路の整数でない実行上限 ${concurrency} は復元の段階で拒む`,
    );
  }

  for (const concurrency of [0, 2_147_483_647]) {
    assert.equal(
      restoreDashboardState(jsonWithConcurrency(concurrency)).projects[0]?.concurrency,
      concurrency,
      `型が許す実行上限 ${concurrency} は復元する`,
    );
    assert.equal(
      dashboardStateFromProto(protoWithConcurrency(concurrency)).projects[0]?.concurrency,
      concurrency,
      `proto 経路でも型が許す実行上限 ${concurrency} は復元する`,
    );
  }

  // proto の toJSON は concurrency が 0 のときフィールドを省略するため、実際の wire では
  // 0 が「無い」形で届く。明示した 0 だけでなく、省略された 0 も復元の段階で 0 として
  // 戻せることを固定する。
  const zeroLimitState = createDashboardState();
  const zeroLimitWire = wireOf(
    DashboardState.create({
      ...zeroLimitState,
      projects: [{ ...zeroLimitState.projects[0]!, concurrency: 0 }],
    }),
  ) as { projects: Record<string, unknown>[] };
  assert.equal(
    "concurrency" in zeroLimitWire.projects[0]!,
    false,
    "concurrency が 0 の wire はフィールドを省略する",
  );
  assert.equal(
    restoreDashboardState(zeroLimitWire).projects[0]?.concurrency,
    0,
    "省略された実行上限 0 は 0 として復元する",
  );
});

test("状態の種別は known な名称と数値だけを受け、未知の値と UNRECOGNIZED を semantics で拒否する", () => {
  const goalWithStatus = (status: unknown) => ({
    goals: [{ goalId: "goal-1", serverId: "mini", project: "esa", status }],
  });
  restoreError(goalWithStatus("GOAL_STATUS_UNKNOWN"), "semantics");
  restoreError(goalWithStatus("UNRECOGNIZED"), "semantics");
  restoreError(goalWithStatus(99), "semantics");
  restoreError(goalWithStatus(-1), "semantics");

  assert.equal(restoreDashboardState(goalWithStatus("GOAL_STATUS_CREATED")).goals[0]?.status, GoalStatus.GOAL_STATUS_CREATED);
  assert.equal(restoreDashboardState(goalWithStatus("GOAL_STATUS_UNSPECIFIED")).goals[0]?.status, GoalStatus.GOAL_STATUS_UNSPECIFIED);
  assert.equal(restoreDashboardState(goalWithStatus(undefined)).goals[0]?.status, GoalStatus.GOAL_STATUS_UNSPECIFIED);

  const run = { runSlug: "run-1", serverId: "mini", project: "esa", goalId: "goal-1", taskName: "task" };
  restoreError({ runs: [{ ...run, status: 99 }] }, "semantics");
  restoreError({ runs: [{ ...run, status: "RUN_STATUS_UNKNOWN" }] }, "semantics");
  assert.equal(restoreDashboardState({ runs: [{ ...run, status: "RUN_STATUS_RUNNING" }] }).runs[0]?.status, RunStatus.RUN_STATUS_RUNNING);

  const decision = { serverId: "mini", project: "esa", inquiryId: "inquiry-1" };
  restoreError({ pendingDecisions: [{ ...decision, type: 99 }] }, "semantics");
  assert.equal(
    restoreDashboardState({ pendingDecisions: [{ ...decision, type: "PENDING_DECISION_TYPE_INQUIRY" }] }).pendingDecisions[0]?.type,
    PendingDecisionType.PENDING_DECISION_TYPE_INQUIRY,
  );

  const goalWithMergeTaskLast = (mergeTaskLast: unknown) => ({
    goals: [{ goalId: "goal-1", serverId: "mini", project: "esa", mergeTaskLast }],
  });
  restoreError(goalWithMergeTaskLast("TASK_STATUS_UNKNOWN"), "semantics");
  assert.equal(
    restoreDashboardState(goalWithMergeTaskLast("TASK_STATUS_COMPLETED")).goals[0]?.mergeTaskLast,
    TaskStatus.TASK_STATUS_COMPLETED,
  );
});

test("構文上正しいが一覧に無い参照は型で拒否せず、復元の境界では受理する", () => {
  const restored = restoreDashboardState({
    servers: [{ serverId: "ghost", name: "Ghost", kind: "Ghost" }],
    projects: [{ name: "unknown-project", serverId: "ghost" }],
    pendingDecisions: [
      {
        type: "PENDING_DECISION_TYPE_INQUIRY",
        serverId: "ghost",
        project: "unknown-project",
        goalId: "missing-goal",
        inquiryId: "missing-inquiry",
      },
    ],
  });

  assert.equal(restored.servers[0]?.serverId, "ghost");
  assert.equal(restored.projects[0]?.name, "unknown-project");
  assert.equal(restored.pendingDecisions[0]?.goalId, "missing-goal");
  assert.equal(restored.pendingDecisions[0]?.inquiryId, "missing-inquiry");
});

test("プルリクエストの URL、自由文、時刻は生のまま復元する", () => {
  const wire = wireOf(createDashboardState());
  const restored = restoreDashboardState(wire);

  assert.equal(restored.goals[0]?.pullRequest?.url, "https://example.test/pr/418");
  assert.equal(restored.pendingDecisions[0]?.body, "リリースの日取りはどうしますか。");
  assert.deepEqual(restored.pendingDecisions[0]?.options, ["今週", "来週"]);
  assert.equal(restored.goals[1]?.pausedReason, "公開の日取りが決まるまで止めています。");
  assert.equal(restored.runs[0]?.summary, "レイアウトを直しています。");
  assert.equal(restored.projects[0]?.concurrency, 4);
  assert.ok(restored.readAt instanceof Date);
  assert.equal(restored.readAt?.toISOString(), NOW.toISOString());
});

test("proto の初期状態も同じ検証を通して内部型へ変換する", () => {
  const restored = dashboardStateFromProto(createDashboardState());

  assert.ok(restored.readAt instanceof Date);
  assert.equal(restored.servers[0]?.serverId, "mini");
  assert.equal(restored.projects[0]?.name, "corporate-site");
  assert.equal(restored.goals[0]?.goalId, "goal-1");
  assert.equal(restored.runs[0]?.runSlug, "run-aaa");
});

test("必須の値が空の proto 初期状態は変換せず拒否する", () => {
  const invalid = DashboardState.create({ servers: [{ serverId: "" }] });
  assert.throws(() => dashboardStateFromProto(invalid), isStage("size"));
});

test("承認待ちの revision と任意ゴール ID を含む実送信形式を復元する", () => {
  const restored = restoreDashboardState(wireOf(createActionableDashboardState()));
  const registration = restored.pendingDecisions.find((decision) => decision.revision === 7);

  assert.ok(registration, "revision 7 の承認待ちがある");
  assert.equal(registration.goalId, undefined);
  assert.equal(registration.inquiryId, undefined);
  assert.equal(
    restored.pendingDecisions.find((decision) => decision.inquiryId === "inquiry-options")?.goalId,
    "goal-2",
  );
  assert.equal(
    restored.pendingDecisions.find((decision) => decision.body === "revision の無い承認待ちです。")?.revision,
    undefined,
  );
});

test("ゴール詳細の JSON を内部型へ復元し、作業記録・質問・タスクを保つ", () => {
  const wire = GetGoalDetailResponse.toJSON(createGoalDetailResponse());
  const restored = restoreGoalDetail(wire);
  assert.equal(restored.goalId, "goal-1");
  assert.equal(restored.status, 2);
  assert.equal(restored.executionStatus, 1);
  assert.equal(restored.objective, "採用ページのレイアウトを直す目的です。");
  assert.deepEqual(restored.outOfScope, ["管理画面の変更は対象外です。"]);
  assert.deepEqual(restored.acceptanceCriteria, [
    "採用ページのレイアウトが崩れないこと。",
    "スマートフォンで読めること。",
  ]);
  assert.deepEqual(restored.constraints, ["既存の配色を変えないこと。"]);
  assert.equal(restored.branch, "goal/45-frontend");
  assert.equal(restored.integrationBranch, "main");
  assert.ok(restored.workLog[0]!.recordedAt instanceof Date);
  assert.equal(restored.workLog[0]!.summary, "最初の作業記録です。");
  assert.equal(restored.questions[0]!.answerText, "変えないでください。");
  assert.equal(restored.questions[1]!.answerText, undefined);
  assert.equal(restored.tasks[0]!.status, 3);
  assert.equal(restored.tasks[1]!.runSlug, "run-bbb");
});

test("タスク詳細の JSON を内部型へ復元し、開始時刻と列挙型と任意項目を保つ", () => {
  const wire = GetTaskDetailResponse.toJSON(createTaskDetailResponse());
  const restored = restoreTaskDetail(wire);
  assert.equal(restored.project, "corporate-site");
  assert.equal(restored.taskName, "採用ページのレイアウトを直す");
  assert.equal(restored.goalId, "goal-1");
  assert.equal(restored.status, 3);
  assert.ok(restored.startedAt instanceof Date);
  assert.equal(restored.pullRequestNumber, 418);
  assert.equal(restored.branch, "goal/45-frontend");
  assert.equal(restored.pullRequestUrl, "https://example.test/pr/418");
  assert.deepEqual(restored.linkedRunSlugs, ["run-aaa", "run-bbb"]);
});

test("タスク詳細の JSON を復元するとき、作成時刻と完了時刻を Date として保つ", () => {
  const createdAt = new Date("2026-11-13T06:00:00Z");
  const completedAt = new Date("2026-11-13T07:00:00Z");
  const wire = GetTaskDetailResponse.toJSON(createTaskDetailResponse({ createdAt, completedAt }));
  const restored = restoreTaskDetail(wire);
  assert.ok(restored.createdAt instanceof Date, "作成時刻を Date として復元する");
  assert.equal(restored.createdAt!.toISOString(), createdAt.toISOString());
  assert.ok(restored.completedAt instanceof Date, "完了時刻を Date として復元する");
  assert.equal(restored.completedAt!.toISOString(), completedAt.toISOString());
});

test("タスク詳細の JSON で作成時刻と完了時刻が省略されたときは undefined のまま復元する", () => {
  const wire = GetTaskDetailResponse.toJSON(
    createTaskDetailResponse({ createdAt: undefined, completedAt: undefined }),
  );
  const restored = restoreTaskDetail(wire);
  assert.equal(restored.createdAt, undefined);
  assert.equal(restored.completedAt, undefined);
});

test("タスク詳細の任意項目が省略された JSON は undefined のまま復元し、undefined 文字列を作らない", () => {
  const wire = GetTaskDetailResponse.toJSON(
    createTaskDetailResponse({
      startedAt: undefined,
      branch: undefined,
      pullRequestNumber: undefined,
      pullRequestUrl: undefined,
      goalId: undefined,
      runSlug: undefined,
      linkedRunSlugs: [],
    }),
  );
  const restored = restoreTaskDetail(wire);
  assert.equal(restored.startedAt, undefined);
  assert.equal(restored.branch, undefined);
  assert.equal(restored.pullRequestNumber, undefined);
  assert.equal(restored.pullRequestUrl, undefined);
  assert.equal(restored.goalId, undefined);
  assert.equal(restored.runSlug, undefined);
  assert.deepEqual(restored.linkedRunSlugs, []);
});
// 判断待ちの操作を組み立てる層の契約。
//
// 一覧の項目から、送信先の種類・対象の識別・回答の内容を決める。画面（Route Handler と
// ブラウザ）はこの層が作った値をそのまま使い、同じ判定を各所で作り直さない。
import test from "node:test";
import assert from "node:assert/strict";
import { PendingDecisionType, type PendingDecision } from "../../src/proto/taktdash.ts";
import { createActionableDashboardState } from "../support/fixtures.ts";
import {
  buildAnswerAction,
  buildGoalRegistrationAction,
  buildMergeAction,
  decisionActionState,
  decisionKey,
  decisionLabel,
  decisionTargetLabel,
  decisionTypeLabel,
  resolveDecisionIntent,
  submitDecisionAction,
  toDecisionResult,
  upsertDecisionResult,
  type DecisionAction,
} from "../../src/state/decision-actions.ts";

const decisions = createActionableDashboardState().pendingDecisions;

const findDecision = (predicate: (decision: PendingDecision) => boolean): PendingDecision => {
  const found = decisions.find(predicate);
  assert.ok(found, "フィクスチャに対象の判断待ちがある");
  return found;
};

const optionsQuestion = (): PendingDecision => findDecision((decision) => decision.inquiryId === "inquiry-options");
const textQuestion = (): PendingDecision => findDecision((decision) => decision.inquiryId === "inquiry-text");
const operation = (): PendingDecision => findDecision((decision) => decision.inquiryId === "inquiry-operation");
const merge = (): PendingDecision =>
  findDecision((decision) => decision.type === PendingDecisionType.PENDING_DECISION_TYPE_AWAITING_MERGE);
const registration = (): PendingDecision =>
  findDecision(
    (decision) =>
      decision.type === PendingDecisionType.PENDING_DECISION_TYPE_GOAL_REGISTRATION && decision.revision === 7,
  );
const registrationWithoutRevision = (): PendingDecision =>
  findDecision(
    (decision) =>
      decision.type === PendingDecisionType.PENDING_DECISION_TYPE_GOAL_REGISTRATION && decision.revision === undefined,
  );
const inquiryWithoutGoal = (): PendingDecision =>
  findDecision((decision) => decision.inquiryId === "inquiry-without-goal");

test("判断待ちの識別子は3種別で重複しない", () => {
  const keys = [decisionKey(optionsQuestion()), decisionKey(merge()), decisionKey(registration())];
  assert.equal(new Set(keys).size, 3);
  assert.match(decisionKey(optionsQuestion()), /inquiry-options/);
  assert.match(decisionKey(merge()), /goal-1/);
  assert.match(decisionKey(registration()), /7/);
});

test("種別の表示名は3種別を判別できる", () => {
  assert.equal(decisionTypeLabel(merge()), "人のマージ待ち");

  const inquiryLabel = decisionTypeLabel(optionsQuestion());
  assert.match(inquiryLabel, /問い合わせ/);

  const registrationLabel = decisionTypeLabel(registration());
  assert.match(registrationLabel, /ゴール登録/);
  assert.match(registrationLabel, /承認/);
  assert.notEqual(registrationLabel, inquiryLabel);

  assert.equal(decisionTypeLabel({ ...optionsQuestion(), type: 99 }), "判断待ち");
});

test("操作の結果に出す項目名はプロジェクトと種別を含む", () => {
  const label = decisionLabel(merge());
  assert.match(label, /corporate-site/);
  assert.match(label, /人のマージ待ち/);
});

test("操作要素の対象名は種別ごとの識別子を含み、同じ種別でも区別できる", () => {
  assert.match(decisionTargetLabel(merge()), /PR #418/);

  const registrationLabel = decisionTargetLabel(registration());
  assert.match(registrationLabel, /revision 7/);

  const optionsLabel = decisionTargetLabel(optionsQuestion());
  assert.match(optionsLabel, /inquiry-options/);
  assert.match(optionsLabel, /リリースの日取りはどうしますか。/);
  assert.notEqual(optionsLabel, decisionTargetLabel(textQuestion()));
});

test("操作の形は問い合わせの選択肢・自由記述・作業依頼とマージ・ゴール登録で決まる", () => {
  assert.equal(decisionActionState(optionsQuestion()).kind, "question-options");
  assert.equal(decisionActionState(textQuestion()).kind, "question-text");
  assert.equal(decisionActionState(operation()).kind, "operation");
  assert.equal(decisionActionState(merge()).kind, "merge");
  assert.equal(decisionActionState(registration()).kind, "goal-registration");
});

test("作業の依頼は選択肢があっても回答ボタンより優先する", () => {
  const withOptions: PendingDecision = { ...operation(), options: ["済み", "未対応"] };
  assert.equal(decisionActionState(withOptions).kind, "operation");
});

test("操作に必要な値が欠けた項目は操作不可として理由を返す", () => {
  const cases: Array<[string, PendingDecision]> = [
    ["ゴールに属さない問い合わせ", inquiryWithoutGoal()],
    ["問い合わせの識別子が空", { ...optionsQuestion(), inquiryId: "" }],
    ["プルリクエストが無いマージ待ち", { ...merge(), pullRequest: undefined }],
    ["ゴール ID が無いマージ待ち", { ...merge(), goalId: undefined }],
    ["revision が無いゴール登録", registrationWithoutRevision()],
  ];
  for (const [label, decision] of cases) {
    const state = decisionActionState(decision);
    assert.equal(state.kind, "unavailable", label);
    assert.ok(typeof state.reason === "string" && state.reason.length > 0, `${label} の理由`);
  }
});

test("自由記述の回答は問い合わせの識別子と本文を持つ", () => {
  const action = buildAnswerAction(textQuestion(), { text: " 補足します " });
  assert.ok(action);
  assert.equal(action.kind, "answer");
  assert.equal(action.payload.serverId, "mini");
  assert.equal(action.payload.project, "esa");
  assert.equal(action.payload.goalId, "goal-2");
  assert.equal(action.payload.inquiryId, "inquiry-text");
  // 前後の空白を落とすのは受け付ける Route Handler の責務。ここでは本文が失われないことだけを見る。
  assert.equal(action.payload.text?.trim(), "補足します");
  assert.equal(action.payload.report, undefined);
});

test("作業の依頼の回答は report だけを持つ", () => {
  const completed = buildAnswerAction(operation(), { report: 1 });
  assert.ok(completed);
  assert.equal(completed.payload.report, 1);
  assert.equal(completed.payload.text, undefined);

  const unable = buildAnswerAction(operation(), { report: 2 });
  assert.ok(unable);
  assert.equal(unable.payload.report, 2);
  assert.equal(unable.payload.text, undefined);
});

test("マージ承認の payload は until を含めず、画面から prepare を選べない", () => {
  const action = buildMergeAction(merge());
  assert.ok(action);
  assert.equal(action.kind, "merge");
  assert.deepEqual(action.payload, {
    serverId: "mini",
    project: "corporate-site",
    goalId: "goal-1",
    prNumber: 418,
  });
});

test("ゴール登録の承認と却下は revision を同じ対象として持つ", () => {
  const approve = buildGoalRegistrationAction(registration(), "approve");
  const reject = buildGoalRegistrationAction(registration(), "reject");
  assert.ok(approve);
  assert.ok(reject);
  assert.equal(approve.kind, "approve-registration");
  assert.equal(reject.kind, "reject-registration");
  assert.deepEqual(approve.payload, { serverId: "mini", project: "takt", revision: 7 });
  assert.deepEqual(reject.payload, { serverId: "mini", project: "takt", revision: 7 });
});

test("意図の解決は種別ごとに正しいダイアログを開き、開くだけでは送信しない", () => {
  const cases: Array<["merge" | "approve-registration" | "reject-registration", PendingDecision]> = [
    ["merge", merge()],
    ["approve-registration", registration()],
    ["reject-registration", registration()],
  ];
  for (const [dialog, decision] of cases) {
    const opened = resolveDecisionIntent(decision, { type: "open", dialog }, { text: "" });
    assert.deepEqual(opened, { kind: "open", dialog }, `${dialog} を開く`);
    assert.equal("action" in opened, false, `${dialog} を開くだけでは送信しない`);
  }

  assert.deepEqual(resolveDecisionIntent(merge(), { type: "cancel" }, { text: "" }), { kind: "close" });
});

test("確定の意図はダイアログに対応する操作を選び、承認と却下を入れ替えない", () => {
  const mergeResolution = resolveDecisionIntent(merge(), { type: "confirm", dialog: "merge" }, { text: "" });
  assert.equal(mergeResolution.kind, "submit");
  assert.equal(mergeResolution.kind === "submit" ? mergeResolution.action?.kind : undefined, "merge");

  const approveResolution = resolveDecisionIntent(
    registration(),
    { type: "confirm", dialog: "approve-registration" },
    { text: "" },
  );
  assert.equal(approveResolution.kind, "submit");
  assert.equal(approveResolution.kind === "submit" ? approveResolution.action?.kind : undefined, "approve-registration");

  const rejectResolution = resolveDecisionIntent(
    registration(),
    { type: "confirm", dialog: "reject-registration" },
    { text: "" },
  );
  assert.equal(rejectResolution.kind, "submit");
  assert.equal(rejectResolution.kind === "submit" ? rejectResolution.action?.kind : undefined, "reject-registration");
});

test("選択肢の回答は選んだ文字列を text として一覧の識別子とともに送る", () => {
  const decision = optionsQuestion();
  const resolution = resolveDecisionIntent(decision, { type: "answer-option", option: "今週" }, { text: "" });
  assert.equal(resolution.kind, "submit");
  const action = resolution.kind === "submit" ? resolution.action : undefined;
  assert.ok(action);
  assert.equal(action.kind, "answer");
  assert.equal(action.payload.text, "今週");
  assert.equal(action.payload.report, undefined);
  assert.equal(action.payload.serverId, decision.serverId);
  assert.equal(action.payload.project, decision.project);
  assert.equal(action.payload.goalId, decision.goalId);
  assert.equal(action.payload.inquiryId, decision.inquiryId);
});

test("自由記述と作業の依頼の意図も従来どおりの回答を組み立てる", () => {
  const textResolution = resolveDecisionIntent(textQuestion(), { type: "answer-text" }, { text: " 補足します " });
  assert.equal(textResolution.kind, "submit");
  const textAction = textResolution.kind === "submit" ? textResolution.action : undefined;
  assert.ok(textAction);
  assert.equal(textAction.payload.text, " 補足します ");

  const reportResolution = resolveDecisionIntent(operation(), { type: "answer-report", report: 2 }, { text: "" });
  assert.equal(reportResolution.kind, "submit");
  const reportAction = reportResolution.kind === "submit" ? reportResolution.action : undefined;
  assert.ok(reportAction);
  assert.equal(reportAction.payload.report, 2);
  assert.equal(reportAction.payload.text, undefined);
});

test("操作結果は一覧と同じ key を持ち、同じ key の結果は置換して別 key は保持する", () => {
  const first = toDecisionResult(merge(), { ok: true, message: "マージを承認しました。" });
  assert.equal(first.key, decisionKey(merge()));
  assert.equal(first.label, decisionLabel(merge()));
  assert.equal(first.ok, true);
  assert.equal(first.message, "マージを承認しました。");

  const single = upsertDecisionResult([], first);
  assert.deepEqual(single, [first]);

  const replaced = toDecisionResult(merge(), { ok: false, message: "操作に失敗しました。" });
  const afterReplace = upsertDecisionResult(single, replaced);
  assert.equal(afterReplace.length, 1);
  assert.equal(afterReplace[0]!.key, first.key);
  assert.equal(afterReplace[0]!.ok, false);
  assert.equal(afterReplace[0]!.message, "操作に失敗しました。");

  const other = toDecisionResult(registration(), { ok: true, message: "ゴール登録を承認しました。" });
  const mixed = upsertDecisionResult(afterReplace, other);
  assert.equal(mixed.length, 2);
  assert.equal(mixed[0]!.key, other.key);
  assert.equal(mixed[1]!.key, first.key);
});

test("送信できない入力では操作を組み立てない", () => {
  assert.equal(buildAnswerAction(textQuestion(), { text: "   " }), undefined);
  assert.equal(buildAnswerAction({ ...textQuestion(), inquiryId: "" }, { text: "回答" }), undefined);
  assert.equal(buildAnswerAction(inquiryWithoutGoal(), { text: "回答" }), undefined);
  assert.equal(buildAnswerAction(operation(), { text: "対応しました" }), undefined);
  assert.equal(buildAnswerAction(textQuestion(), { report: 1 }), undefined);
  assert.equal(buildMergeAction({ ...merge(), pullRequest: undefined }), undefined);
  assert.equal(buildMergeAction({ ...merge(), goalId: undefined }), undefined);
  assert.equal(buildGoalRegistrationAction(registrationWithoutRevision(), "approve"), undefined);
});

type FetchCall = { url: string; init: RequestInit | undefined };

const fetchReturning = (
  response: { ok: boolean; json: () => Promise<unknown> },
): { fetchImpl: typeof fetch; calls: FetchCall[] } => {
  const calls: FetchCall[] = [];
  const fetchImpl = (async (input: RequestInfo | URL, init?: RequestInit) => {
    calls.push({ url: String(input), init });
    return response as Response;
  }) as typeof fetch;
  return { fetchImpl, calls };
};

const answerAction = (): DecisionAction => {
  const action = buildAnswerAction(textQuestion(), { text: "補足します" });
  assert.ok(action);
  return action;
};

test("回答の送信は回答の Route Handler へ payload を POST する", async () => {
  const { fetchImpl, calls } = fetchReturning({ ok: true, json: async () => ({ ok: true }) });
  const action = answerAction();

  const outcome = await submitDecisionAction(action, fetchImpl);

  assert.equal(calls.length, 1);
  assert.match(calls[0]!.url, /\/api\/decisions\/answer$/);
  assert.equal(calls[0]!.init?.method, "POST");
  assert.deepEqual(JSON.parse(String(calls[0]!.init?.body)), action.payload);
  assert.equal(outcome.ok, true);
  assert.equal(outcome.message, "回答を送信しました。");
});

test("マージ承認・ゴール登録の承認と却下も種類ごとの Route Handler へ送る", async () => {
  const cases: Array<[DecisionAction, string, string]> = [
    [buildMergeAction(merge())!, "/api/decisions/merge", "マージを承認しました。"],
    [
      buildGoalRegistrationAction(registration(), "approve")!,
      "/api/decisions/goal-registration/approve",
      "ゴール登録を承認しました。",
    ],
    [
      buildGoalRegistrationAction(registration(), "reject")!,
      "/api/decisions/goal-registration/reject",
      "ゴール登録を却下しました。",
    ],
  ];
  for (const [action, path, message] of cases) {
    const { fetchImpl, calls } = fetchReturning({ ok: true, json: async () => ({ ok: true }) });
    const outcome = await submitDecisionAction(action, fetchImpl);
    assert.ok(calls[0]!.url.endsWith(path), `${action.kind} は ${path} へ送る`);
    assert.deepEqual(JSON.parse(String(calls[0]!.init?.body)), action.payload);
    assert.equal(outcome.ok, true);
    assert.equal(outcome.message, message);
  }
});

test("backend が失敗を返すと画面に出す失敗の文言へ変換する", async () => {
  const { fetchImpl } = fetchReturning({
    ok: false,
    json: async () => ({ ok: false, error: "問い合わせはゴールに属していません。" }),
  });
  const outcome = await submitDecisionAction(answerAction(), fetchImpl);
  assert.equal(outcome.ok, false);
  assert.equal(outcome.message, "問い合わせはゴールに属していません。");
});

test("失敗の応答に文言が無い場合は既定の失敗文言を使う", async () => {
  const { fetchImpl } = fetchReturning({ ok: false, json: async () => ({ ok: false }) });
  const outcome = await submitDecisionAction(answerAction(), fetchImpl);
  assert.equal(outcome.ok, false);
  assert.equal(outcome.message, "操作に失敗しました。");
});

test("通信できない場合は通信の確認を促す文言へ変換する", async () => {
  const fetchImpl = (async () => {
    throw new TypeError("fetch failed");
  }) as typeof fetch;
  const outcome = await submitDecisionAction(answerAction(), fetchImpl);
  assert.equal(outcome.ok, false);
  assert.equal(outcome.message, "操作を送信できませんでした。通信を確認してください。");
});

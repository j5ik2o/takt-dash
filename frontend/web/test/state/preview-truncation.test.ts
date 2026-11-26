// 切り詰めの印（Goal.nameTruncated、PendingDecision.bodyTruncated）の復元と表示の契約（issue #230）。
//
// backend は takt の応答の印から切り詰めを判定し、文字列には省略記号を混ぜずに印を別の項目で
// 送る。画面は印だけを見て省略記号を1つ付け、文の形やバイト長から推定しない。
import test from "node:test";
import assert from "node:assert/strict";
import { createElement, type ComponentType, type ReactElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { DashboardState, PendingDecisionType, type PendingDecision } from "../../src/proto/taktdash.ts";
import { createDashboardState, NOW } from "../support/fixtures.ts";
import { dashboardStateFromProto, restoreDashboardState } from "../../src/state/restore-state.ts";
import {
  decisionDisplayBody,
  decisionTargetDescription,
  decisionTargetLabel,
} from "../../src/state/decision-actions.ts";
import DecisionsPage from "../../app/decisions/page.tsx";
import ProjectsPage from "../../app/projects/page.tsx";

const QA_CUT = "土台と完成申請の宛先は、プルリク";

const renderPage = (Page: ComponentType<never>, props: unknown): string =>
  renderToStaticMarkup(createElement(Page as ComponentType<never>, props as never) as ReactElement);

const countOf = (text: string, part: string): number => text.split(part).length - 1;

const wire = (mutate: (json: Record<string, unknown>) => void): Record<string, unknown> => {
  const json = DashboardState.toJSON(createDashboardState()) as Record<string, unknown>;
  mutate(json);
  return json;
};

test("印は true のときだけ立て、欠落（古い backend）・false・ほかの値は切っていないとして復元する", () => {
  for (const [value, expected] of [
    [true, true],
    [false, false],
    [undefined, false],
    ["true", false],
    [1, false],
    [null, false],
  ] as const) {
    const restored = restoreDashboardState(
      wire((json) => {
        const goals = json.goals as Record<string, unknown>[];
        const decisions = json.pendingDecisions as Record<string, unknown>[];
        for (const item of [...goals, ...decisions]) {
          delete item.nameTruncated;
          delete item.bodyTruncated;
        }
        if (value !== undefined) {
          goals[0].nameTruncated = value;
          decisions[0].bodyTruncated = value;
        }
      }),
    );
    assert.equal(restored.goals[0].nameTruncated, expected, `nameTruncated=${String(value)}`);
    assert.equal(restored.pendingDecisions[0].bodyTruncated, expected, `bodyTruncated=${String(value)}`);
  }
});

test("proto 経路でも印を同じに復元する", () => {
  const state = createDashboardState();
  state.goals[0].nameTruncated = true;
  state.pendingDecisions[1].bodyTruncated = true;
  const restored = dashboardStateFromProto(state);
  assert.equal(restored.goals[0].nameTruncated, true);
  assert.equal(restored.pendingDecisions[1].bodyTruncated, true);
});

const decision = (body: string, bodyTruncated: boolean): PendingDecision => ({
  ...createDashboardState().pendingDecisions[0],
  body,
  bodyTruncated,
});

test("判断待ちの本文は印のあるときだけ、末尾の空白を除いて省略記号を1つ足す（生成した本文）", () => {
  const parts = ["段落", "\n", "\n\n", " ", "…", "https://example.test/x", "。"];
  let seed = 14;
  for (let index = 0; index < 400; index += 1) {
    const body = Array.from({ length: index % 12 }, () => {
      seed = (seed * 48_271) % 2_147_483_647;
      return parts[seed % parts.length];
    }).join("");
    assert.equal(decisionDisplayBody(decision(body, false)), body, JSON.stringify(body));
    const shown = decisionDisplayBody(decision(body, true));
    assert.equal(shown, `${body.trimEnd()}…`, JSON.stringify(body));
    assert.equal(countOf(shown, "…"), countOf(body, "…") + 1, "省略記号は1つだけ足す");
  }
});

test("問い合わせの読み上げ用の文にも、切ってある本文の省略記号を付ける", () => {
  const cut = decision(QA_CUT, true);
  assert.match(decisionTargetLabel(cut), new RegExp(`（${QA_CUT}…）`));
  assert.match(decisionTargetDescription(cut), new RegExp(`（${QA_CUT}…）`));
  assert.doesNotMatch(decisionTargetLabel(decision(QA_CUT, false)), /…/);
});

test("判断待ちのマージ待ちのカードは、切ってある目的の末尾に省略記号を1つ出す", () => {
  const state = createDashboardState();
  const merge = state.pendingDecisions.find(
    (item) => item.type === PendingDecisionType.PENDING_DECISION_TYPE_AWAITING_MERGE,
  );
  assert.ok(merge);
  merge.body = QA_CUT;
  merge.bodyTruncated = true;

  const html = renderPage(DecisionsPage, { state, now: NOW });

  assert.match(html, new RegExp(`${QA_CUT}…`));
  assert.equal(countOf(html, `${QA_CUT}…`), countOf(html, QA_CUT), "本文はいつも省略記号付きで出す");
  assert.doesNotMatch(html, /……/);
});

test("印の無い判断待ちの本文は、文の途中で終わっていても省略記号を付けない", () => {
  const state = createDashboardState();
  state.pendingDecisions[1].body = QA_CUT;
  state.pendingDecisions[1].bodyTruncated = false;

  const html = renderPage(DecisionsPage, { state, now: NOW });

  assert.match(html, new RegExp(QA_CUT));
  assert.doesNotMatch(html, new RegExp(`${QA_CUT}…`));
});

test("プロジェクトのゴールは、印のある名前の見出しと全文に省略記号を1つ出す", () => {
  const state = createDashboardState();
  state.goals[0].name = QA_CUT;
  state.goals[0].nameTruncated = true;

  const html = renderPage(ProjectsPage, { state, now: NOW });

  assert.ok(countOf(html, `${QA_CUT}…`) >= 1, html);
  assert.equal(countOf(html, `${QA_CUT}…`), countOf(html, QA_CUT), "名前はいつも省略記号付きで出す");
  assert.doesNotMatch(html, /……/);
});

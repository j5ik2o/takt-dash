// プロジェクトページのゴール表示の契約（issue #78）。
//
// ゴールの目的の全文と完了済みゴールでカードが埋まらず、いま動いているものが
// 最初の画面で分かるようにする。見出しは1行、全文は折りたたみ、既定は進行中のみ、
// 状態は1つのバッジ、進捗と PR リンクを添え、run・進行中・完了の順に並べる。
// 描画結果の HTML から、カード単位の要素と文字列を観測する。
import test from "node:test";
import assert from "node:assert/strict";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { createDashboardState, NOW } from "../support/fixtures.ts";
import {
  DashboardState,
  ExecutionStatus,
  GoalStatus,
  InquiryKind,
  PendingDecisionType,
  type Goal,
} from "../../src/proto/taktdash.ts";
import ProjectsPage from "../../app/projects/page.tsx";

const PROJECT = "corporate-site";

const goal = (overrides: Partial<Goal> = {}): Goal => ({
  goalId: "goal-active",
  nameTruncated: false,
  serverId: "mini",
  project: PROJECT,
  name: "アクティブな目的の見出しです。補足の文が続きます。",
  status: GoalStatus.GOAL_STATUS_CREATED,
  executionStatus: ExecutionStatus.EXECUTION_STATUS_ACTIVE,
  taskCounts: { pending: 0, running: 0, completed: 6, failed: 0, exceeded: 0, prFailed: 0 },
  pullRequest: undefined,
  ...overrides,
});

const renderProjects = (goals: Goal[], state: Partial<DashboardState> = {}): string => {
  const base = createDashboardState();
  const next = DashboardState.create({ ...base, goals, ...state });
  return renderToStaticMarkup(
    createElement(ProjectsPage, { state: next, now: NOW }),
  );
};

// 1枚のプロジェクトカードを取り出す。カードは入れ子の article を持たない。
const cardFor = (html: string, project = PROJECT): string => {
  const idIndex = html.indexOf(`id="project-${project}"`);
  assert.ok(idIndex >= 0, `${project} のカードが描画されていない`);
  const start = html.lastIndexOf("<article", idIndex);
  const end = html.indexOf("</article>", idIndex);
  return html.slice(start, end + "</article>".length);
};

// 各ゴール行は h4.goal-head から始まる。次の見出しまでを1行として切り出す。
const goalRows = (card: string): string[] => {
  const starts = Array.from(card.matchAll(/<h4 class="goal-head">/g), (match) => match.index ?? 0);
  return starts.map((start, index) => card.slice(start, starts[index + 1] ?? card.length));
};

const goalRowFor = (card: string, needle: string): string => {
  const row = goalRows(card).find((candidate) => candidate.includes(needle));
  if (row === undefined) {
    assert.fail(`見出しに ${needle} を含むゴール行が描画されていない`);
  }
  return row;
};

const headingTexts = (card: string): string[] =>
  Array.from(card.matchAll(/<h4 class="goal-head">([\s\S]*?)<\/h4>/g), (match) => match[1] ?? "");

const badgeClasses = (row: string): string[] =>
  Array.from(
    row.matchAll(/<span\b[^>]*class="([^"]*\bgoal-badge\b[^"]*)"[^>]*>[\s\S]*?<\/span>/g),
    (match) => match[1] ?? "",
  );

const badgeTexts = (row: string): string[] =>
  Array.from(
    row.matchAll(/<span\b[^>]*class="[^"]*\bgoal-badge\b[^"]*"[^>]*>([\s\S]*?)<\/span>/g),
    (match) => (match[1] ?? "").trim(),
  );

const detailsTag = (html: string, className: string): string | undefined =>
  new RegExp(`<details\\b[^>]*class="[^"]*\\b${className}\\b[^"]*"[^>]*>`).exec(html)?.[0];

// 指定クラスの details を、入れ子の details を数えて閉じタグまで切り出す。
const detailsBlock = (html: string, className: string): string | undefined => {
  const open = new RegExp(
    `<details\\b[^>]*class="[^"]*\\b${className}\\b[^"]*"[^>]*>`,
  ).exec(html);
  if (open === null) {
    return undefined;
  }
  const openEnd = open.index + open[0].length;
  const tags = /<details\b[^>]*>|<\/details>/g;
  tags.lastIndex = openEnd;
  let depth = 1;
  let tag = tags.exec(html);
  while (tag !== null) {
    depth += tag[0].startsWith("</") ? -1 : 1;
    if (depth === 0) {
      return html.slice(open.index, tag.index + tag[0].length);
    }
    tag = tags.exec(html);
  }
  return undefined;
};

const summaryText = (html: string): string => {
  const match = /<summary>([\s\S]*?)<\/summary>/.exec(html);
  if (match === null) {
    assert.fail("summary が描画されていない");
  }
  return (match[1] ?? "").replace(/<[^>]+>/g, "").trim();
};

const progressText = (html: string): string => {
  const match = /<p class="goal-progress">([\s\S]*?)<\/p>/.exec(html);
  if (match === null) {
    assert.fail("タスクの進み具合が描画されていない");
  }
  return (match[1] ?? "").replace(/<[^>]+>/g, "").trim();
};

// 進行中一覧は run 区画の後、区画ごとの折りたたみ（goal-done 系のクラス）の前にある。
const inProgressSection = (card: string): string => {
  const start = card.indexOf("進行中のゴール");
  assert.ok(start >= 0, "進行中のゴールの区画が描画されていない");
  const end = card.indexOf('class="goal-done');
  return card.slice(start, end < 0 ? card.length : end);
};

const completedSection = (card: string): string => {
  const start = card.indexOf('class="goal-done"');
  return start < 0 ? "" : card.slice(start);
};

test("ゴールの見出しは目的の最初の1文を1行で出し、全文を見出しに出さない", () => {
  const name = "採用ページのレイアウトを直す。対象はトップページだけである。";
  const card = cardFor(renderProjects([goal({ name })]));

  assert.deepEqual(headingTexts(card), ["採用ページのレイアウトを直す。"]);
  assert.doesNotMatch(card, /<h4 class="goal-head">[^<]*対象はトップページだけである。<\/h4>/);
});

test("引用と括弧の内側に句点がある目的でも、外側の最初の文までを見出しにする", () => {
  const name =
    "オーナーの指示（2026-11-16「E2Eないだろ？テストピラミッドを健全化してほしい。カバレッジも80%以上」）に基づき、土台を作る。対象は issue #106 である。";
  const card = cardFor(renderProjects([goal({ name })]));

  assert.deepEqual(headingTexts(card), [
    "オーナーの指示（2026-11-16「E2Eないだろ？テストピラミッドを健全化してほしい。カバレッジも80%以上」）に基づき、土台を作る。",
  ]);
});

test("目的の全文は既定で閉じた折りたたみの中に入れる", () => {
  const name = "採用ページのレイアウトを直す。対象はトップページだけである。";
  const card = cardFor(renderProjects([goal({ name })]));
  const row = goalRowFor(card, "採用ページのレイアウトを直す。");

  const tag = detailsTag(row, "goal-full");
  assert.ok(tag !== undefined, "全文の折りたたみが描画されていない");
  assert.doesNotMatch(tag, /\bopen\b/, "全文の折りたたみは既定で閉じる");
  const block = detailsBlock(row, "goal-full");
  assert.ok(block !== undefined, "全文の折りたたみが描画されていない");
  assert.ok(summaryText(block).length > 0, "全文の折りたたみは summary を持つ");
  assert.ok(block.includes(name), "全文の折りたたみに目的の全文が入る");
});

test("ゴールごとに状態のバッジを1つだけ出し、完了に稼働中を出さない", () => {
  const cases: Array<{ name: string; status: GoalStatus; executionStatus: ExecutionStatus; label: string; tone: string }> = [
    {
      name: "進行中の目的です。",
      status: GoalStatus.GOAL_STATUS_CREATED,
      executionStatus: ExecutionStatus.EXECUTION_STATUS_ACTIVE,
      label: "進行中",
      tone: "live",
    },
    {
      name: "マージ待ちの目的です。",
      status: GoalStatus.GOAL_STATUS_AWAITING_MERGE,
      executionStatus: ExecutionStatus.EXECUTION_STATUS_ACTIVE,
      label: "マージ待ち",
      tone: "warn",
    },
    {
      name: "一時停止の目的です。",
      status: GoalStatus.GOAL_STATUS_CREATED,
      executionStatus: ExecutionStatus.EXECUTION_STATUS_PAUSED,
      label: "一時停止",
      tone: "idle",
    },
    {
      name: "完了した目的です。",
      status: GoalStatus.GOAL_STATUS_COMPLETED,
      executionStatus: ExecutionStatus.EXECUTION_STATUS_ACTIVE,
      label: "完了",
      tone: "idle",
    },
    {
      name: "中止した目的です。",
      status: GoalStatus.GOAL_STATUS_CREATED,
      executionStatus: ExecutionStatus.EXECUTION_STATUS_ABORTED,
      label: "中止",
      tone: "warn",
    },
  ];
  const goals = cases.map((item, index) =>
    goal({ goalId: `goal-${index}`, name: item.name, status: item.status, executionStatus: item.executionStatus }),
  );
  const card = cardFor(renderProjects(goals));

  assert.doesNotMatch(card, /稼働中/, "完了の組み合わせを含め、稼働中と出さない");
  for (const item of cases) {
    const row = goalRowFor(card, item.name);
    assert.equal(badgeClasses(row).length, 1, `${item.label} の行のバッジは1つ`);
    assert.deepEqual(badgeTexts(row), [item.label]);
    assert.match(badgeClasses(row)[0]!, new RegExp(`\\b${item.tone}\\b`), `${item.label} の色クラス`);
  }
});

test("一時停止のゴールのバッジには一時停止の理由を添える", () => {
  const card = cardFor(
    renderProjects([
      goal({
        name: "公開を待つ目的です。",
        executionStatus: ExecutionStatus.EXECUTION_STATUS_PAUSED,
        pausedReason: "公開の日取りが決まるまで止めています。",
      }),
    ]),
  );

  const row = goalRowFor(inProgressSection(card), "公開を待つ目的です。");
  const why = /<p class="goal-why">([\s\S]*?)<\/p>/.exec(row);
  assert.ok(why !== null, "一時停止の理由を進行中区画の行に出す");
  assert.match(why[1] ?? "", /公開の日取りが決まるまで止めています。/, "理由の本文を省略せず出す");
});

test("既定では進行中のゴールだけを出し、完了と中止は進行中の一覧に出さない", () => {
  const active = goal({ goalId: "goal-active", name: "進行中の目的です。" });
  const completed = goal({
    goalId: "goal-completed",
    name: "完了した目的です。",
    status: GoalStatus.GOAL_STATUS_COMPLETED,
  });
  const aborted = goal({
    goalId: "goal-aborted",
    name: "中止した目的です。",
    executionStatus: ExecutionStatus.EXECUTION_STATUS_ABORTED,
  });
  const awaitingMerge = goal({
    goalId: "goal-awaiting-merge",
    name: "マージ待ちの目的です。",
    status: GoalStatus.GOAL_STATUS_AWAITING_MERGE,
  });
  const paused = goal({
    goalId: "goal-paused",
    name: "一時停止した目的です。",
    executionStatus: ExecutionStatus.EXECUTION_STATUS_PAUSED,
  });
  const card = cardFor(renderProjects([active, completed, aborted, awaitingMerge, paused]));

  const visible = inProgressSection(card);
  assert.ok(visible.includes("進行中の目的です。"), "進行中のゴールは既定で出す");
  assert.equal(visible.includes("完了した目的です。"), false, "完了したゴールは進行中の一覧に出さない");
  assert.equal(visible.includes("中止した目的です。"), false, "中止したゴールは進行中の一覧に出さない");

  const done = completedSection(card);
  for (const name of ["マージ待ちの目的です。", "一時停止した目的です。"]) {
    assert.ok(visible.includes(name), `${name} を進行中の一覧に残す`);
    assert.equal(done.includes(name), false, `${name} を完了区画に入れない`);
  }
});

test("質問のある進行中のゴールも既定の一覧に残す", () => {
  const card = cardFor(
    renderProjects(
      [goal({ goalId: "goal-question", name: "質問待ちの目的です。" })],
      {
        pendingDecisions: [
          {
            type: PendingDecisionType.PENDING_DECISION_TYPE_INQUIRY,
            serverId: "mini",
            project: PROJECT,
            goalId: "goal-question",
            inquiryKind: InquiryKind.INQUIRY_KIND_QUESTION,
            inquiryId: "inquiry-question",
            createdAt: new Date(NOW.getTime() - 10 * 60_000),
            body: "この方針で進めますか。",
            options: ["進める", "止める"],
            pullRequest: undefined,
            stale: false,
          },
        ],
      },
    ),
  );

  assert.match(inProgressSection(card), /質問待ちの目的です。/);
});

test("完了と中止は別の折りたたみに分け、完了の件数に中止を含めない（issue #243）", () => {
  const completedA = goal({
    goalId: "goal-completed-a",
    name: "完了した目的Aです。",
    status: GoalStatus.GOAL_STATUS_COMPLETED,
  });
  const completedB = goal({
    goalId: "goal-completed-b",
    name: "完了した目的Bです。",
    status: GoalStatus.GOAL_STATUS_COMPLETED,
  });
  const aborted = goal({
    goalId: "goal-aborted",
    name: "中止した目的です。",
    executionStatus: ExecutionStatus.EXECUTION_STATUS_ABORTED,
  });
  const card = cardFor(renderProjects([completedA, completedB, aborted]));

  const tag = detailsTag(card, "goal-done");
  assert.ok(tag !== undefined, "完了したゴールの折りたたみが描画されていない");
  assert.doesNotMatch(tag, /\bopen\b/, "完了したゴールの折りたたみは既定で閉じる");
  const doneBlock = detailsBlock(card, "goal-done");
  assert.ok(doneBlock !== undefined, "完了したゴールの折りたたみが描画されていない");
  const doneSummary = summaryText(doneBlock);
  assert.match(doneSummary, /完了したゴール/, "完了したゴールの折りたたみは見出しを持つ");
  assert.match(doneSummary, /2件/, "完了したゴールだけの件数を出す");
  assert.equal(doneBlock.includes("中止した目的です。"), false, "中止したゴールを完了の折りたたみへ入れない");
  assert.equal(badgeTexts(goalRowFor(doneBlock, "完了した目的Aです。"))[0], "完了");

  const abortedBlock = detailsBlock(card, "goal-aborted");
  assert.ok(abortedBlock !== undefined, "中止したゴールの折りたたみが描画されていない");
  const abortedSummary = summaryText(abortedBlock);
  assert.match(abortedSummary, /中止したゴール/, "中止したゴールの折りたたみは見出しを持つ");
  assert.match(abortedSummary, /1件/, "中止したゴールだけの件数を出す");
  assert.equal(badgeTexts(goalRowFor(abortedBlock, "中止した目的です。"))[0], "中止");
});

test("完了と中止が0件なら折りたたみを描画しない", () => {
  const card = cardFor(renderProjects([goal({ name: "進行中の目的です。" })]));

  assert.equal(detailsTag(card, "goal-done"), undefined);
  assert.equal(detailsBlock(card, "goal-done"), undefined, "0件なら完了の折りたたみを描画しない");
  assert.equal(card.includes("完了したゴール"), false);
});

test("進行中のゴールが0件なら、その旨を進行中の区画に出す", () => {
  const card = cardFor(
    renderProjects([
      goal({ name: "完了した目的です。", status: GoalStatus.GOAL_STATUS_COMPLETED }),
    ]),
  );

  const visible = inProgressSection(card);
  assert.match(visible, /<li class="empty">[^<]+<\/li>/, "進行中が0件である旨を区画に出す");
  assert.deepEqual(goalRows(visible), [], "進行中の区画にゴール行を出さない");
});

test("タスクの進み具合は完了 n / 全体 m で出し、失敗がある場合は警告色を付ける", () => {
  const failingCounts = { pending: 1, running: 1, completed: 3, failed: 1, exceeded: 1, prFailed: 1 };
  const failing = goal({
    goalId: "goal-failing",
    name: "失敗のある目的です。",
    taskCounts: failingCounts,
  });
  const healthy = goal({
    goalId: "goal-healthy",
    name: "失敗の無い目的です。",
    taskCounts: { pending: 0, running: 0, completed: 6, failed: 0, exceeded: 0, prFailed: 0 },
  });
  const card = cardFor(renderProjects([failing, healthy]));

  const failingRow = goalRowFor(card, "失敗のある目的です。");
  const failingTotal = Object.values(failingCounts).reduce((sum, value) => sum + value, 0);
  assert.match(
    progressText(failingRow),
    new RegExp(`完了\\s*${failingCounts.completed}\\s*/\\s*全体\\s*${failingTotal}`),
    "6項目の合計を全体として出し、失敗や超過や PR 失敗も数える",
  );
  const failingWarning = /<p class="goal-failed warn">([\s\S]*?)<\/p>/.exec(failingRow);
  assert.ok(failingWarning !== null, "失敗のあるゴールは警告色の段落を出す");
  assert.match(failingWarning[1] ?? "", /3件/, "失敗・超過・PR 失敗を合わせた件数を出す");

  const healthyRow = goalRowFor(card, "失敗の無い目的です。");
  assert.match(progressText(healthyRow), /完了\s*6\s*\/\s*全体\s*6/);
  assert.equal(healthyRow.includes('class="goal-failed warn"'), false, "失敗が無ければ警告を出さない");
});

test("タスクの件数が無いゴールは、タスクがまだ登録されていないと出す", () => {
  const card = cardFor(renderProjects([goal({ name: "件数の無い目的です。", taskCounts: undefined })]));

  const row = goalRowFor(card, "件数の無い目的です。");
  assert.equal(progressText(row), "タスクはまだ登録されていません。");
  assert.doesNotMatch(row, /全体\s*0/, "全体 0 という意味の取れない進捗を出さない");
});

test("タスクが0件のゴールは、タスクがまだ登録されていないと出す", () => {
  const empty = { pending: 0, running: 0, completed: 0, failed: 0, exceeded: 0, prFailed: 0 };
  const card = cardFor(renderProjects([goal({ name: "タスクの無い目的です。", taskCounts: empty })]));

  assert.equal(progressText(goalRowFor(card, "タスクの無い目的です。")), "タスクはまだ登録されていません。");
});

test("ゴールのバッジは、タスクの登録と実行中の有無で未着手・実行中・進行中を区別する", () => {
  const untouched = goal({ goalId: "goal-untouched", name: "未着手の目的です。", taskCounts: undefined });
  const running = goal({
    goalId: "goal-running",
    name: "実行中の目的です。",
    taskCounts: { pending: 0, running: 1, completed: 2, failed: 0, exceeded: 0, prFailed: 0 },
  });
  const active = goal({
    goalId: "goal-active",
    name: "進行中の目的です。",
    taskCounts: { pending: 1, running: 0, completed: 2, failed: 0, exceeded: 0, prFailed: 0 },
  });
  const card = cardFor(renderProjects([untouched, running, active]));
  const visible = inProgressSection(card);

  assert.deepEqual(badgeTexts(goalRowFor(visible, "実行中の目的です。")), ["実行中"]);
  assert.deepEqual(badgeTexts(goalRowFor(visible, "進行中の目的です。")), ["進行中"]);
  assert.equal(visible.includes("未着手の目的です。"), false, "未着手のゴールは進行中の一覧に出さない");

  const untouchedBlock = detailsBlock(card, "goal-untouched");
  assert.ok(untouchedBlock !== undefined, "未着手のゴールの折りたたみが描画されていない");
  assert.deepEqual(badgeTexts(goalRowFor(untouchedBlock, "未着手の目的です。")), ["未着手"]);
});

test("未着手のゴールは進行中の件数へ入れず、未着手の折りたたみに分ける（issue #242）", () => {
  const active = goal({ goalId: "goal-active", name: "進行中の目的です。" });
  const untouched = goal({ goalId: "goal-untouched", name: "未着手の目的です。", taskCounts: undefined });
  const card = cardFor(renderProjects([active, untouched]));

  const text = card.replace(/<!-- -->/g, "");
  assert.match(text, /進行中のゴール（1件）/, "進行中の件数に未着手を含めない");

  const untouchedBlock = detailsBlock(card, "goal-untouched");
  assert.ok(untouchedBlock !== undefined, "未着手のゴールの折りたたみが描画されていない");
  const untouchedSummary = summaryText(untouchedBlock);
  assert.match(untouchedSummary, /未着手のゴール/, "未着手のゴールの折りたたみは見出しを持つ");
  assert.match(untouchedSummary, /1件/, "未着手だけの件数を出す");
});

test("プルリクエストの URL があるゴールだけリンクを出す", () => {
  const withPr = goal({
    goalId: "goal-pr",
    name: "PR のある目的です。",
    pullRequest: { number: 418, url: "https://example.test/pr/418" },
  });
  const withoutPr = goal({
    goalId: "goal-no-pr",
    name: "PR の無い目的です。",
    pullRequest: undefined,
  });
  const emptyUrl = goal({
    goalId: "goal-empty-pr",
    name: "URL の空の目的です。",
    pullRequest: { number: 0, url: "" },
  });
  const card = cardFor(renderProjects([withPr, withoutPr, emptyUrl]));

  const prRow = goalRowFor(card, "PR のある目的です。");
  const prBlock = /<p class="goal-pr">([\s\S]*?)<\/p>/.exec(prRow);
  assert.ok(prBlock !== null, "PR の欄を出す");
  assert.match(prBlock[0], /href="https:\/\/example\.test\/pr\/418"/, "PR の URL を href に出す");
  assert.match(prBlock[1] ?? "", /#418/, "PR の番号をリンクに出す");

  const noPrRow = goalRowFor(card, "PR の無い目的です。");
  assert.equal(noPrRow.includes('class="goal-pr"'), false, "URL の無いゴールに PR の欄を出さない");
  assert.equal(/href="https:\/\/example\.test\/pr\/0"/.test(noPrRow), false);

  const emptyRow = goalRowFor(card, "URL の空の目的です。");
  assert.equal(emptyRow.includes('class="goal-pr"'), false, "URL が空なら PR のリンクを出さない");
  assert.equal(emptyRow.includes('href=""'), false);
});

test("カードは走っている run・進行中のゴール・完了したゴールの順に並べる", () => {
  const card = cardFor(
    renderProjects([
      goal({ goalId: "goal-active", name: "進行中の目的です。" }),
      goal({ goalId: "goal-completed", name: "完了した目的です。", status: GoalStatus.GOAL_STATUS_COMPLETED }),
    ]),
  );

  const runIndex = card.indexOf("走っている run");
  const inProgressIndex = card.indexOf("進行中のゴール");
  const completedIndex = card.indexOf('class="goal-done"');
  assert.ok(runIndex >= 0, "走っている run の区画がある");
  assert.ok(inProgressIndex >= 0, "進行中のゴールの区画がある");
  assert.ok(completedIndex >= 0, "完了したゴールの区画がある");
  assert.ok(runIndex < inProgressIndex, "run を進行中のゴールより先に置く");
  assert.ok(inProgressIndex < completedIndex, "進行中のゴールを完了したゴールより先に置く");
});

test("見出しからゴール詳細へのリンクを作らない", () => {
  const card = cardFor(
    renderProjects([
      goal({ goalId: "goal-active", name: "進行中の目的です。" }),
      goal({ goalId: "goal-completed", name: "完了した目的です。", status: GoalStatus.GOAL_STATUS_COMPLETED }),
    ]),
  );

  assert.doesNotMatch(card, /href="\/goals\//, "行き先の無いゴール詳細リンクを作らない");
  assert.doesNotMatch(card, /<a\b[^>]*>\s*<h4\b/, "見出しをリンクにしない");
});

// issue #315 D7：操作の結果に「閉じる」を付け、利用者が押すまで消さない（元帥の決定、案 A）。
//
// 設計は docs/design/issue-315.md の D7。判断待ちのページ（DecisionsContent）を jsdom の DOM へ
// マウントし、選択肢の回答（押すとすぐに送る）で本物の結果を出して、本物の「閉じる」を押す。
// 書き込み（fetch）は差し替えて数え、成功・失敗の応答をテストが決める。
import test, { mock } from "node:test";
import assert from "node:assert/strict";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { JSDOM } from "jsdom";
import type { DashboardState, PendingDecision } from "../../src/proto/taktdash.ts";
import { createActionableDashboardState, NOW } from "../support/fixtures.ts";
import { DecisionsContent } from "../../src/components/decisions-content.tsx";
import { decisionLabel } from "../../src/state/decision-actions.ts";

const SUCCESS_MESSAGE = "回答を送信しました。";

// 選択肢つきの問い合わせ（押すとすぐに送る）を、別の項目として count 件作る。
// 本文と問い合わせの ID を変え、結果のラベル（decisionLabel）が項目ごとに違うようにする。
const questionDecisions = (count: number): PendingDecision[] => {
  const base = createActionableDashboardState().pendingDecisions.find(
    (decision) => decision.inquiryId === "inquiry-options",
  );
  assert.ok(base, "フィクスチャに選択肢つきの問い合わせがある");
  return Array.from({ length: count }, (_, index) => ({
    ...base,
    inquiryId: `inquiry-options-${index}`,
    body: `選択肢の質問 ${index} です。`,
    options: ["今週", "来週"],
  }));
};

// react-dom/client のマウントに要る DOM を jsdom で用意し、グローバルへ公開する
// （issue-308-operation-confirm.test.ts と同じ手順）。
let domInstalled = false;
const installDomEnvironment = (): void => {
  if (domInstalled) {
    return;
  }
  domInstalled = true;
  const dom = new JSDOM("<!doctype html><html><body></body></html>", { url: "http://localhost/" });
  const scope = globalThis as unknown as Record<string, unknown>;
  const win = dom.window as unknown as Record<string, unknown>;
  for (const key of Object.getOwnPropertyNames(dom.window)) {
    if (key in scope) {
      continue;
    }
    try {
      scope[key] = win[key];
    } catch {
      // 読み出せないプロパティはマウントに不要として飛ばす。
    }
  }
  for (const [key, value] of [
    ["window", dom.window],
    ["document", dom.window.document],
    ["navigator", dom.window.navigator],
  ] as const) {
    Object.defineProperty(scope, key, { value, configurable: true, writable: true });
  }
  scope.IS_REACT_ACT_ENVIRONMENT = true;
};

type ResultRow = { label: string; ok: boolean; message: string };

// 待っている非同期の続き（fetch の応答、response.json、setState）を流しきる。
const flush = async (): Promise<void> => {
  await act(async () => {
    for (let index = 0; index < 20; index += 1) {
      await Promise.resolve();
    }
  });
};

const normalize = (text: string | null | undefined): string => (text ?? "").replace(/\s+/g, " ").trim();

const mountDecisionsPage = async (decisions: PendingDecision[]) => {
  installDomEnvironment();
  let state: DashboardState = { ...createActionableDashboardState(), pendingDecisions: decisions };
  let disconnected = false;
  // 次の書き込みへの応答。true なら成功、文字列ならその文の失敗にする。
  let nextResponse: true | string = true;
  const sent: string[] = [];
  (globalThis as unknown as { fetch: unknown }).fetch = async (url: string) => {
    sent.push(url);
    const response = nextResponse;
    return (response === true
      ? { ok: true, json: async () => ({ ok: true }) }
      : { ok: false, json: async () => ({ ok: false, error: response }) }) as unknown as Response;
  };
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root: Root = createRoot(container);
  const render = async (): Promise<void> => {
    await act(async () => {
      root.render(createElement(DecisionsContent, { snapshotLoaded: true, state, now: NOW, disconnected }));
    });
  };
  await render();

  const rows = (): HTMLLIElement[] => Array.from(container.querySelectorAll<HTMLLIElement>(".decision-results li"));
  const rowOf = (row: HTMLLIElement): ResultRow => ({
    label: normalize(row.querySelector(".result-label")?.textContent),
    ok: row.getAttribute("role") === "status",
    message: normalize(row.querySelector(".result-message")?.textContent),
  });
  const closeButtonOf = (row: HTMLLIElement): HTMLButtonElement => {
    const found = Array.from(row.querySelectorAll<HTMLButtonElement>("button")).filter(
      (button) => normalize(button.textContent) === "閉じる",
    );
    assert.equal(found.length, 1, `結果の行「${rowOf(row).label}」に「閉じる」が1つある`);
    return found[0]!;
  };
  const click = async (element: HTMLElement): Promise<void> => {
    await act(async () => {
      element.click();
    });
    await flush();
  };

  return {
    container,
    sent,
    rows,
    shown: (): ResultRow[] => rows().map(rowOf),
    closeButtonOf,
    heading: (): HTMLHeadingElement | null => container.querySelector("h1"),
    // index 番目の項目の「今週」を押して回答し、結果を出す。response で成功か失敗かを決める。
    answer: async (index: number, response: true | string): Promise<void> => {
      nextResponse = response;
      const item = Array.from(container.querySelectorAll<HTMLElement>(".tray-item")).find((element) =>
        (element.textContent ?? "").includes(`選択肢の質問 ${index} です。`),
      );
      assert.ok(item, `質問 ${index} の項目がある`);
      const option = Array.from(item.querySelectorAll<HTMLButtonElement>("button")).find(
        (button) => button.getAttribute("aria-label") === "今週",
      );
      assert.ok(option, `質問 ${index} に「今週」のボタンがある`);
      assert.equal(option.disabled, false, `質問 ${index} の「今週」は押せる`);
      await click(option);
    },
    close: async (rowIndex: number): Promise<void> => {
      const row = rows()[rowIndex];
      assert.ok(row, `${rowIndex} 行目の結果がある`);
      await click(closeButtonOf(row));
    },
    setDisconnected: async (next: boolean): Promise<void> => {
      disconnected = next;
      await render();
    },
    // SSE の更新をまねて、新しい状態で描画し直す。
    replaceState: async (next: DashboardState): Promise<void> => {
      state = next;
      await render();
    },
    unmount: async (): Promise<void> => {
      await act(async () => {
        root.unmount();
      });
      container.remove();
    },
  };
};

test("D7: 成功の結果と失敗の結果のどちらの行にも「閉じる」があり、説明の先にその行の結果のラベルがある", async () => {
  const decisions = questionDecisions(2);
  const page = await mountDecisionsPage(decisions);
  try {
    await page.answer(0, true);
    await page.answer(1, "takt に届きませんでした。");
    assert.deepEqual(page.shown(), [
      { label: decisionLabel(decisions[1]!), ok: false, message: "takt に届きませんでした。" },
      { label: decisionLabel(decisions[0]!), ok: true, message: SUCCESS_MESSAGE },
    ]);
    const ids = new Set<string>();
    for (const row of page.rows()) {
      const button = page.closeButtonOf(row);
      assert.equal(button.getAttribute("type"), "button");
      // 読み上げ名は操作の文言だけにし、対象は説明で補う（issue #324 の決まり）。
      const name = button.getAttribute("aria-label");
      assert.ok(name === null || name === "閉じる", `読み上げ名は「閉じる」だけ（実際: ${name}）`);
      const describedBy = button.getAttribute("aria-describedby");
      assert.ok(describedBy, "「閉じる」は aria-describedby で対象を指す");
      assert.equal(ids.has(describedBy), false, "説明の id は行ごとに違う");
      ids.add(describedBy);
      const description = describedBy
        .split(/\s+/)
        .map((id) => document.getElementById(id))
        .filter((element): element is HTMLElement => element !== null);
      assert.ok(description.length > 0, `aria-describedby の先（${describedBy}）がある`);
      assert.ok(
        description.every((element) => row.contains(element)),
        "説明の先は同じ結果の行の中にある",
      );
      const label = normalize(row.querySelector(".result-label")?.textContent);
      assert.ok(
        description.some((element) => normalize(element.textContent).includes(label)),
        `説明の先の文に、その行の結果のラベル「${label}」が入る`,
      );
    }
  } finally {
    await page.unmount();
  }
});

test("D7: 成功の結果は、時間がたっても、一覧が更新されても、「閉じる」を押すまで消えない", async () => {
  mock.timers.enable({ apis: ["Date", "setTimeout", "setInterval"], now: NOW.getTime() });
  const decisions = questionDecisions(2);
  const page = await mountDecisionsPage(decisions);
  try {
    await page.answer(0, true);
    assert.equal(page.rows().length, 1, "結果が出る");
    await act(async () => {
      mock.timers.tick(30 * 60 * 1000);
    });
    await flush();
    assert.deepEqual(page.shown(), [{ label: decisionLabel(decisions[0]!), ok: true, message: SUCCESS_MESSAGE }]);
    // 回答した項目が一覧から消えた更新（SSE）を受けても、結果は残る。
    await page.replaceState({ ...createActionableDashboardState(), pendingDecisions: [decisions[1]!] });
    assert.deepEqual(page.shown(), [{ label: decisionLabel(decisions[0]!), ok: true, message: SUCCESS_MESSAGE }]);
  } finally {
    await page.unmount();
    mock.timers.reset();
  }
});

test("D7: 「閉じる」は押した行だけを消し、ほかの行は順も中身も残し、書き込みを出さない", async () => {
  const decisions = questionDecisions(3);
  const page = await mountDecisionsPage(decisions);
  try {
    await page.answer(0, true);
    await page.answer(1, "操作に失敗しました。");
    await page.answer(2, true);
    assert.equal(page.sent.length, 3);
    const before = page.shown();
    await page.close(1);
    assert.deepEqual(page.shown(), [before[0], before[2]]);
    assert.equal(page.sent.length, 3, "閉じても書き込みは出ない");
  } finally {
    await page.unmount();
  }
});

test("D7: 切断中も「閉じる」は押せて、押した行が消える", async () => {
  const decisions = questionDecisions(2);
  const page = await mountDecisionsPage(decisions);
  try {
    await page.answer(0, true);
    await page.answer(1, true);
    await page.setDisconnected(true);
    const rows = page.rows();
    assert.equal(rows.length, 2, "切断しても結果は残る");
    for (const row of rows) {
      assert.equal(page.closeButtonOf(row).disabled, false, "切断中も「閉じる」は押せる");
    }
    const before = page.shown();
    await page.close(0);
    assert.deepEqual(page.shown(), [before[1]]);
    assert.equal(page.sent.length, 2, "閉じても書き込みは出ない");
  } finally {
    await page.unmount();
  }
});

test("D7: 閉じた後のフォーカスは、同じ位置に来た行の「閉じる」、無ければ1つ前の行、0件ならページの h1 へ移る", async () => {
  const decisions = questionDecisions(3);
  const page = await mountDecisionsPage(decisions);
  try {
    await page.answer(0, true);
    await page.answer(1, true);
    await page.answer(2, true);
    // 3 行のうち真ん中を閉じる → 同じ位置（1 行目）に来た行。
    await page.close(1);
    assert.equal(document.activeElement, page.closeButtonOf(page.rows()[1]!), "同じ位置に来た行の「閉じる」");
    // 最後の行を閉じる → 1 つ前の行。
    await page.close(1);
    assert.equal(document.activeElement, page.closeButtonOf(page.rows()[0]!), "1 つ前の行の「閉じる」");
    // 最後の1件を閉じる → h1。
    await page.close(0);
    assert.equal(page.rows().length, 0);
    const heading = page.heading();
    assert.ok(heading, "ページに h1 がある");
    assert.equal(normalize(heading.textContent), "判断待ち");
    assert.equal(heading.getAttribute("tabindex"), "-1", "h1 はフォーカスを受けられる");
    assert.equal(document.activeElement, heading, "結果が無くなったら h1");
  } finally {
    await page.unmount();
  }
});

// ---- 性質のテスト：回答と「閉じる」の操作の列で、画面の結果の並びとフォーカスがモデルと一致する ----

// 種を決めた疑似乱数（mulberry32）。失敗したときに、種から同じ操作の列を作り直せる。
const random = (seed: number): (() => number) => {
  let value = seed >>> 0;
  return () => {
    value = (value + 0x6d2b79f5) >>> 0;
    let t = value;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
};

type Operation = { kind: "answer"; index: number; ok: boolean } | { kind: "close"; row: number };
type ModelRow = ResultRow & { index: number };
type FocusTarget = { kind: "close"; row: number } | { kind: "heading" };

test("D7（性質）: 回答と「閉じる」をどう並べても、結果の行の並び・書き込みの数・閉じた後のフォーカスがモデルと同じ", async () => {
  const ITEMS = 4;
  const decisions = questionDecisions(ITEMS);
  for (let seed = 1; seed <= 40; seed += 1) {
    const next = random(seed);
    const page = await mountDecisionsPage(decisions);
    let model: ModelRow[] = [];
    let writes = 0;
    const done: Operation[] = [];
    try {
      const steps = 1 + Math.floor(next() * 12);
      for (let step = 0; step < steps; step += 1) {
        const operation: Operation =
          model.length > 0 && next() < 0.4
            ? { kind: "close", row: Math.floor(next() * model.length) }
            : { kind: "answer", index: Math.floor(next() * ITEMS), ok: next() < 0.6 };
        done.push(operation);
        const where = `種 ${seed} の操作 ${JSON.stringify(done)}`;
        if (operation.kind === "answer") {
          const failure = `質問 ${operation.index} の失敗です。`;
          await page.answer(operation.index, operation.ok ? true : failure);
          writes += 1;
          model = [
            {
              index: operation.index,
              label: decisionLabel(decisions[operation.index]!),
              ok: operation.ok,
              message: operation.ok ? SUCCESS_MESSAGE : failure,
            },
            ...model.filter((row) => row.index !== operation.index),
          ];
        } else {
          await page.close(operation.row);
          model = model.filter((_, rowIndex) => rowIndex !== operation.row);
          const focus: FocusTarget =
            model.length === 0
              ? { kind: "heading" }
              : { kind: "close", row: Math.min(operation.row, model.length - 1) };
          const expected =
            focus.kind === "heading" ? page.heading() : page.closeButtonOf(page.rows()[focus.row]!);
          assert.ok(expected, `${where}: フォーカスの先がある`);
          assert.equal(document.activeElement, expected, `${where}: 閉じた後のフォーカスが ${JSON.stringify(focus)}`);
        }
        assert.deepEqual(
          page.shown(),
          model.map(({ label, ok, message }) => ({ label, ok, message })),
          `${where}: 結果の行の並び`,
        );
        assert.equal(page.sent.length, writes, `${where}: 書き込みの数`);
      }
    } finally {
      await page.unmount();
    }
  }
});

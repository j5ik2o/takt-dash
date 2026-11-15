// クライアント側の状態受信の契約（C15）。
//
// EventSource を差し替え可能な関数にし、受信で状態が置き換わること、
// エラー時に再接続すること、初回の RSC の内容が受信後の状態で置き換わることを観測する。
import test from "node:test";
import assert from "node:assert/strict";
import { renderToStaticMarkup } from "react-dom/server";
import { createElement } from "react";
import { DashboardState } from "../../src/proto/taktdash.ts";
import { createDashboardState } from "../support/fixtures.ts";
import { openStateStream, type EventSourceLike } from "../../src/components/state-stream.ts";
import { buildOverviewView } from "../../src/state/dashboard.ts";
import { restoreDashboardState } from "../../src/state/restore-state.ts";
import { DecisionsContent } from "../../src/components/decisions-content.tsx";
import { OverviewContent } from "../../src/components/overview-content.tsx";

class FakeEventSource implements EventSourceLike {
  static instances: FakeEventSource[] = [];
  onmessage: ((event: { data: string }) => void) | null = null;
  onerror: ((event: unknown) => void) | null = null;
  closed = false;
  constructor(public readonly url: string) {
    FakeEventSource.instances.push(this);
  }
  close() {
    this.closed = true;
  }
}

const freshFakes = (): typeof FakeEventSource => {
  FakeEventSource.instances = [];
  return FakeEventSource;
};

type StreamHandlers = {
  onState?: (state: DashboardState) => void;
  onError?: () => void;
};

const open = (Ctor: typeof FakeEventSource, handlers: StreamHandlers) =>
  openStateStream({
    url: "/api/state",
    createEventSource: (url: string) => new Ctor(url),
    onState: handlers.onState ?? (() => {}),
    onError: handlers.onError ?? (() => {}),
  });

test("受信した状態で画面の状態を置き換える", () => {
  const Ctor = freshFakes();
  const received: DashboardState[] = [];
  const stream = open(Ctor, { onState: (state) => received.push(state) });
  const source = Ctor.instances[0]!;
  assert.equal(source.url, "/api/state");
  source.onmessage?.({
    data: JSON.stringify({ readAt: "2026-11-13T07:30:00.000Z", servers: [{ serverId: "mini", name: "Mac mini" }] }),
  });
  assert.equal(received.length, 1);
  stream.close();
  assert.equal(source.closed, true);
});

test("proto の JSON（時刻は ISO 文字列、列挙型は名称文字列）を受信すると内部型へ復元され、描画の件数と表示が送信値に対応する", () => {
  const Ctor = freshFakes();
  const received: DashboardState[] = [];
  const stream = open(Ctor, { onState: (state) => received.push(state) });

  // 実際の送信形式で観測するため、送信側と同じ DashboardState.toJSON を使う。
  // toJSON は時刻を ISO 文字列、列挙型を名称文字列（例 "RUN_STATUS_RUNNING"）で出す。
  const wire = DashboardState.toJSON(createDashboardState());
  Ctor.instances[0]!.onmessage?.({ data: JSON.stringify(wire) });

  assert.equal(received.length, 1);
  const state = received[0]!;

  // 時刻は Date として復元される。
  assert.ok(state.readAt instanceof Date);
  assert.ok(state.servers[0]!.lastReadAt instanceof Date);
  assert.ok(state.runs[0]!.startedAt instanceof Date);
  assert.ok(state.pendingDecisions[0]!.createdAt instanceof Date);
  // 列挙型は名称文字列から数値へ戻る（NaN にならない）。
  assert.equal(state.runs[0]!.status, 1); // RUN_STATUS_RUNNING
  assert.equal(state.pendingDecisions[1]!.type, 2); // PENDING_DECISION_TYPE_AWAITING_MERGE

  const now = new Date("2026-11-13T07:30:30.000Z");
  // 概要: 実行中の run が件数に入り、判断待ちの内訳が送信値に対応する。
  const overview = buildOverviewView(state, now);
  assert.equal(overview.runningRuns, 1, "実行中の run が1件として数えられる");
  assert.equal(overview.pendingCounts.total, 2);
  assert.equal(overview.pendingCounts.inquiries, 1, "問い合わせが1件として数えられる");
  assert.equal(overview.pendingCounts.awaitingMerge, 1, "人のマージ待ちが1件として数えられる");

  // 判断待ち: 種類表示が送信値に対応する（マージ待ちは「人のマージ待ち」）。
  const decisionsHtml = renderToStaticMarkup(createElement(DecisionsContent, { state, now }));
  assert.match(decisionsHtml, /人のマージ待ち/);
  assert.match(decisionsHtml, /問い合わせ（質問）/);

  // 概要: 実行中の run 件数が描画に現れる。
  const overviewHtml = renderToStaticMarkup(createElement(OverviewContent, { view: overview }));
  assert.match(overviewHtml, /1/);

  stream.close();
});

test("切断（エラー）時は接続を捨てて再接続する", () => {
  const Ctor = freshFakes();
  let state = "initial";
  const stream = open(Ctor, {
    onState: () => {
      state = "updated";
    },
    onError: () => {
      state = "disconnected";
    },
  });
  const first = Ctor.instances[0]!;
  first.onerror?.({});
  assert.equal(state, "disconnected");
  assert.equal(first.closed, true, "古い接続を閉じる");
  assert.equal(Ctor.instances.length, 2, "新しい接続を作って再接続する");
  assert.equal(Ctor.instances[1]!.url, "/api/state");
  stream.close();
});

test("受信前の状態は上書きされず、受信後に置き換わる", () => {
  const Ctor = freshFakes();
  let current: DashboardState = createDashboardState();
  const stream = open(Ctor, {
    onState: (state) => {
      current = state;
    },
  });
  assert.equal(current.servers.length, 2);
  Ctor.instances[0]!.onmessage?.({
    data: JSON.stringify({ servers: [{ serverId: "solo", name: "Mac Studio" }] }),
  });
  assert.equal(current.servers.length, 1);
  assert.equal(current.servers[0]?.name, "Mac Studio");
  stream.close();
});

test("close の後はエラーでも再接続しない", () => {
  const Ctor = freshFakes();
  const stream = open(Ctor, {});
  stream.close();
  Ctor.instances[0]!.onerror?.({});
  assert.equal(Ctor.instances.length, 1);
});

test("ステップ位置と反復回数は proto の JSON を経て復元後も同じ意味で保持される", () => {
  const wire = DashboardState.toJSON(createDashboardState());
  const restored = restoreDashboardState(JSON.parse(JSON.stringify(wire)));
  const run = restored.runs[0]!;

  assert.equal(run.currentStepNumber, 2, "現在ステップの位置を保持する");
  assert.equal(run.totalSteps, 5, "全ステップ数を保持する");
  assert.equal(run.currentIteration, 2, "反復回数を保持する");
  assert.equal(run.currentStepName, "implement", "今のステップ名を保持する");
});

test("位置と反復回数とステップ名を持たない run を復元しても 0 や空文字を捏造しない", () => {
  const restored = restoreDashboardState({ runs: [{ runSlug: "run-x" }] });
  const run = restored.runs[0]!;

  assert.equal(run.currentStepNumber, undefined, "未設定は undefined のままにする");
  assert.equal(run.totalSteps, undefined, "未設定は undefined のままにする");
  assert.equal(run.currentIteration, undefined, "未設定は undefined のままにする");
  assert.equal(run.currentStepName, undefined, "未設定は undefined のままにする");
});

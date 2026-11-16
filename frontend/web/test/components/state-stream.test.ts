// クライアント側の状態受信の契約（C15）。
//
// EventSource を差し替え可能な関数にし、受信で状態が置き換わること、
// エラー時に再接続すること、初回の RSC の内容が受信後の状態で置き換わることを観測する。
import test from "node:test";
import assert from "node:assert/strict";
import { renderToStaticMarkup } from "react-dom/server";
import { createElement } from "react";
import { ConnectionState, DashboardState } from "../../src/proto/taktdash.ts";
import { createDashboardState, BACKEND_BRANCH, BACKEND_COMMIT, NOW } from "../support/fixtures.ts";
import { openStateStream, type EventSourceLike } from "../../src/components/state-stream.ts";
import { reduceLiveState } from "../../src/components/state-provider.tsx";
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

type ScheduledReconnect = {
  run: () => void;
  delayMs: number;
  cancelled: boolean;
};

type ReconnectScheduler = (run: () => void, delayMs: number) => () => void;

/** 再接続の予約を記録し、テストが任意の時点で実行する。 */
class FakeScheduler {
  readonly scheduled: ScheduledReconnect[] = [];
  readonly schedule: ReconnectScheduler = (run, delayMs) => {
    const entry: ScheduledReconnect = { run, delayMs, cancelled: false };
    this.scheduled.push(entry);
    return () => {
      entry.cancelled = true;
    };
  };
  latest(): ScheduledReconnect {
    const entry = this.scheduled[this.scheduled.length - 1];
    assert.ok(entry, "再接続が予約されています");
    return entry;
  }
  runLatest() {
    const entry = this.latest();
    if (!entry.cancelled) {
      entry.run();
    }
  }
}

const open = (Ctor: typeof FakeEventSource, handlers: StreamHandlers, scheduler?: FakeScheduler) =>
  openStateStream({
    url: "/api/state",
    createEventSource: (url: string) => new Ctor(url),
    onState: handlers.onState ?? (() => {}),
    onError: handlers.onError ?? (() => {}),
    ...(scheduler === undefined ? {} : { scheduleReconnect: scheduler.schedule }),
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
  // プロジェクト単位の接続状態と最後に読めた時刻も実送信形式から復元される。
  assert.equal(state.projects[0]!.connected, true);
  assert.ok(state.projects[0]!.lastReadAt instanceof Date);
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
  const decisionsHtml = renderToStaticMarkup(createElement(DecisionsContent, { state, now, disconnected: false }));
  assert.match(decisionsHtml, /人のマージ待ち/);
  assert.match(decisionsHtml, /問い合わせ（質問）/);

  // 概要: 実行中の run 件数が描画に現れる。
  const overviewHtml = renderToStaticMarkup(createElement(OverviewContent, { view: overview, disconnected: false }));
  assert.match(overviewHtml, /1/);

  stream.close();
});

test("切断（エラー）時は接続を捨て、待機してから同じ URL へ再接続する", () => {
  const Ctor = freshFakes();
  const scheduler = new FakeScheduler();
  let state = "initial";
  const stream = open(
    Ctor,
    {
      onState: () => {
        state = "updated";
      },
      onError: () => {
        state = "disconnected";
      },
    },
    scheduler,
  );
  const first = Ctor.instances[0]!;
  first.onerror?.({});
  assert.equal(state, "disconnected");
  assert.equal(first.closed, true, "古い接続を閉じる");
  assert.equal(Ctor.instances.length, 1, "エラーの直後には新しい接続を作らない");
  assert.equal(scheduler.latest().delayMs, 1000, "最初の再接続は1秒待つ");

  scheduler.runLatest();
  assert.equal(Ctor.instances.length, 2, "予約した待機の後に新しい接続を作る");
  assert.equal(Ctor.instances[1]!.url, "/api/state");
  stream.close();
});

test("再接続の待機は連続失敗で倍になり、受信に成功すると初期値へ戻る", () => {
  const Ctor = freshFakes();
  const scheduler = new FakeScheduler();
  const stream = open(Ctor, {}, scheduler);

  Ctor.instances[0]!.onerror?.({});
  assert.equal(scheduler.latest().delayMs, 1000);
  scheduler.runLatest();

  Ctor.instances[1]!.onerror?.({});
  assert.equal(scheduler.latest().delayMs, 2000, "連続失敗では待機を倍にする");
  scheduler.runLatest();

  Ctor.instances[2]!.onmessage?.({ data: JSON.stringify({ servers: [] }) });
  Ctor.instances[2]!.onerror?.({});
  assert.equal(scheduler.latest().delayMs, 1000, "受信に成功したら待機を初期値へ戻す");
  stream.close();
});

test("close は予約済みの再接続を取り消す", () => {
  const Ctor = freshFakes();
  const scheduler = new FakeScheduler();
  const stream = open(Ctor, {}, scheduler);
  Ctor.instances[0]!.onerror?.({});
  const scheduled = scheduler.latest();

  stream.close();
  assert.equal(scheduled.cancelled, true, "予約済みの再接続を取り消す");

  // 取り消しの連絡が届く前にタイマーが発火しても、閉じた接続を再開しない。
  scheduled.run();
  assert.equal(Ctor.instances.length, 1);
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

test("プロジェクトの接続状態と最終読み取り時刻は proto の JSON を経て復元後も保持される", () => {
  const wire = DashboardState.toJSON(createDashboardState());
  const restored = restoreDashboardState(JSON.parse(JSON.stringify(wire)));
  const project = restored.projects.find((item) => item.name === "corporate-site");

  assert.ok(project !== undefined, "corporate-site のプロジェクトを復元する");
  assert.equal(project.connectionState, ConnectionState.CONNECTION_STATE_CONNECTED, "接続状態を数値へ戻す");
  assert.equal(
    project.lastReadAt?.getTime(),
    new Date(NOW.getTime() - 60_000).getTime(),
    "最終読み取り時刻を Date へ戻す",
  );
});

test("位置と反復回数とステップ名を持たない run を復元しても 0 や空文字を捏造しない", () => {
  const restored = restoreDashboardState({ runs: [{ runSlug: "run-x" }] });
  const run = restored.runs[0]!;

  assert.equal(run.currentStepNumber, undefined, "未設定は undefined のままにする");
  assert.equal(run.totalSteps, undefined, "未設定は undefined のままにする");
  assert.equal(run.currentIteration, undefined, "未設定は undefined のままにする");
  assert.equal(run.currentStepName, undefined, "未設定は undefined のままにする");
});

test("受信したプロジェクトの最後に読めた時刻を復元する", () => {
  // 実送信形式（Project.toJSON）は connected が false のときフィールドを省略する。
  // 省略された connected を切断として復元し、最後に読めた時刻を Date として保持することを観測する。
  const restored = restoreDashboardState({
    projects: [
      {
        name: "esa",
        about: "イベントストアの実装",
        serverId: "mini",
        completedTasks24h: 1,
        lastReadAt: "2026-11-13T07:09:00.000Z",
      },
    ],
  });

  assert.equal(restored.projects[0]?.connected, false);
  assert.ok(restored.projects[0]?.lastReadAt instanceof Date);
  assert.equal(restored.projects[0]?.lastReadAt?.toISOString(), "2026-11-13T07:09:00.000Z");
});

test("SSE を受信して状態が置き換わっても backend のビルド情報を保持する", () => {
  const Ctor = freshFakes();
  const before = createDashboardState();
  // 受信前は切断中としておき、受信で接続中へ戻ることも同じ実体で観測する。
  let live: { state: DashboardState; disconnected: boolean } = { state: before, disconnected: true };
  const stream = open(Ctor, {
    onState: (next) => {
      live = reduceLiveState(live, { type: "state", state: next });
    },
    onError: () => {
      live = reduceLiveState(live, { type: "error" });
    },
  });

  // 送信側と同じ DashboardState.toJSON の JSON を受信する。
  Ctor.instances[0]!.onmessage?.({ data: JSON.stringify(DashboardState.toJSON(before)) });

  assert.equal(live.disconnected, false, "受信で接続中へ戻る");
  assert.equal(live.state.buildInfo?.commit, BACKEND_COMMIT, "受信後も backend の短縮 SHA を保持する");
  assert.equal(live.state.buildInfo?.branch, BACKEND_BRANCH, "受信後も backend のブランチ名を保持する");

  stream.close();
});


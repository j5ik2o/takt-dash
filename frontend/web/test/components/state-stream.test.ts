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
import {
  openStateStream,
  STATE_STREAM_STALE_TIMEOUT_MS,
  type EventSourceLike,
  type OpenStateStreamOptions,
} from "../../src/components/state-stream.ts";
import { reduceLiveState, type StateStreamContextValue } from "../../src/components/state-provider.tsx";
import { buildOverviewView } from "../../src/state/dashboard.ts";
import {
  dashboardStateFromProto,
  restoreDashboardState,
  type DashboardState as RestoredDashboardState,
} from "../../src/state/restore-state.ts";
import { DomainPrimitiveError } from "../../src/state/domain-primitives.ts";
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

// 受信した状態は復元後の内部型で届く。proto の DashboardState は送信側の
// JSON 化（DashboardState.toJSON）にだけ使う。
type StreamHandlers = {
  onState?: (state: RestoredDashboardState) => void;
  onError?: () => void;
};

type ScheduledReconnect = {
  run: () => void;
  delayMs: number;
  cancelled: boolean;
  fired: boolean;
};

type ReconnectScheduler = (run: () => void, delayMs: number) => () => void;

/** 再接続の予約を記録し、テストが任意の時点で実行する。 */
class FakeScheduler {
  readonly scheduled: ScheduledReconnect[] = [];
  readonly schedule: ReconnectScheduler = (run, delayMs) => {
    const entry: ScheduledReconnect = { run, delayMs, cancelled: false, fired: false };
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
      entry.fired = true;
      entry.run();
    }
  }
}

// 見張りタイマーは既定で偽のスケジューラへ差し替え、本物の 40 秒タイマーを残さない。
const open = (
  Ctor: typeof FakeEventSource,
  handlers: StreamHandlers,
  scheduler?: FakeScheduler,
  extra: Partial<OpenStateStreamOptions> = {},
) =>
  openStateStream({
    url: "/api/state",
    createEventSource: (url: string) => new Ctor(url),
    onState: handlers.onState ?? (() => {}),
    onError: handlers.onError ?? (() => {}),
    scheduleWatchdog: new FakeScheduler().schedule,
    ...(scheduler === undefined ? {} : { scheduleReconnect: scheduler.schedule }),
    ...extra,
  });

test("受信した状態で画面の状態を置き換える", () => {
  const Ctor = freshFakes();
  const received: RestoredDashboardState[] = [];
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
  const received: RestoredDashboardState[] = [];
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
  const overview = buildOverviewView(state, now, true);
  assert.equal(overview.runningRuns, 1, "実行中の run が1件として数えられる");
  assert.equal(overview.pendingCounts.total, 2);
  assert.equal(overview.pendingCounts.inquiries, 1, "問い合わせが1件として数えられる");
  assert.equal(overview.pendingCounts.awaitingMerge, 1, "人のマージ待ちが1件として数えられる");

  // 判断待ち: 種類表示が送信値に対応する（マージ待ちは「人のマージ待ち」）。
  const decisionsHtml = renderToStaticMarkup(createElement(DecisionsContent, { snapshotLoaded: true, state, now, disconnected: false }));
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

test("同じ接続でエラーが複数回発火しても、再接続の予約は1つだけ残り、接続は増えない", () => {
  const Ctor = freshFakes();
  const scheduler = new FakeScheduler();
  let errors = 0;
  const stream = open(Ctor, { onError: () => (errors += 1) }, scheduler);
  const first = Ctor.instances[0]!;
  // 切断でハンドラは外れるため、外す前のハンドラを握って、閉じた接続から重ねて届くエラーを再現する（issue #383）。
  const handler = first.onerror;
  assert.ok(handler);
  handler({});
  handler({});
  handler({});

  assert.equal(scheduler.scheduled.length, 1, "最初のエラーで接続を切り離し、2回目以降は予約しない");
  assert.equal(scheduler.scheduled.filter((entry) => !entry.cancelled).length, 1, "有効な予約は1つだけ");
  assert.equal(scheduler.latest().delayMs, 1000, "待機は1秒のまま");
  assert.equal(errors, 1, "切断の通知は1回だけ");

  scheduler.runLatest();
  assert.equal(Ctor.instances.length, 2, "再接続で作る新しい接続は1つだけ");
  stream.close();
});

test("受信前の状態は上書きされず、受信後に置き換わる", () => {
  const Ctor = freshFakes();
  let current: RestoredDashboardState = dashboardStateFromProto(createDashboardState());
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
  const restored = restoreDashboardState({
    runs: [{ runSlug: "run-x", serverId: "mini", project: "esa", taskName: "implement" }],
  });
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

test("不正な値を受信したときは状態を置き換えず、onState を呼ばない", () => {
  const Ctor = freshFakes();
  const received: RestoredDashboardState[] = [];
  const stream = open(Ctor, { onState: (state) => received.push(state) });

  assert.throws(
    () => Ctor.instances[0]!.onmessage?.({ data: JSON.stringify({ servers: [{ serverId: "" }] }) }),
    (error) => error instanceof DomainPrimitiveError && error.stage === "size",
  );
  assert.equal(received.length, 0, "検証に失敗した状態は画面へ渡さない");

  stream.close();
});

test("SSE を受信して状態が置き換わっても backend のビルド情報を保持する", () => {
  const Ctor = freshFakes();
  const before = createDashboardState();
  // 受信前は切断中としておき、受信で接続中へ戻ることも同じ実体で観測する。
  // Context 値は内部型の状態を持つため、初期値も proto から変換してそろえる。
  let live: StateStreamContextValue = { state: dashboardStateFromProto(before), snapshotLoaded: true, disconnected: true };
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


// ===== 半開き接続の無音見張り（issue #383） =====
//
// backend は 15 秒ごとに状態を送り直す。最後の受信（または接続の開始）から壁時計で
// 40 秒何も届かなければ、onerror が来なくても切断として扱い、接続を開き直す。
// 見張りタイマーと再接続の予約はどちらも偽のスケジューラで、壁時計は now で差し替える。

const VALID_STATE = JSON.stringify({ servers: [] });
const INVALID_STATE = JSON.stringify({ servers: [{ serverId: "" }] });

type WatchdogRig = {
  Ctor: typeof FakeEventSource;
  watchdog: FakeScheduler;
  reconnect: FakeScheduler;
  clock: { now: number };
  states: number;
  errors: number;
  stream: ReturnType<typeof openStateStream>;
};

const openWatched = (extra: Partial<OpenStateStreamOptions> = {}, handlers: StreamHandlers = {}): WatchdogRig => {
  const Ctor = freshFakes();
  const watchdog = new FakeScheduler();
  const reconnect = new FakeScheduler();
  const clock = { now: 1_000_000 };
  const rig = { Ctor, watchdog, reconnect, clock, states: 0, errors: 0 } as WatchdogRig;
  rig.stream = open(
    Ctor,
    {
      onState: (state) => {
        rig.states += 1;
        handlers.onState?.(state);
      },
      onError: () => {
        rig.errors += 1;
        handlers.onError?.();
      },
    },
    reconnect,
    { scheduleWatchdog: watchdog.schedule, now: () => clock.now, ...extra },
  );
  return rig;
};

const pendingOf = (scheduler: FakeScheduler): ScheduledReconnect[] =>
  scheduler.scheduled.filter((entry) => !entry.cancelled && !entry.fired);

test("見張り時間は backend の送り直し 15 秒の 2 回分より長く、長時間切断の文面しきい値 60 秒より短い", () => {
  assert.equal(STATE_STREAM_STALE_TIMEOUT_MS, 40_000);
  assert.ok(STATE_STREAM_STALE_TIMEOUT_MS > 2 * 15_000);
  assert.ok(STATE_STREAM_STALE_TIMEOUT_MS < 60_000);
});

test("接続直後から見張りを 40 秒で仕掛け、1 件も届かないまま期限が来たら切断して再接続を 1 本予約する", () => {
  const rig = openWatched();
  const first = rig.Ctor.instances[0]!;
  assert.equal(rig.watchdog.latest().delayMs, 40_000, "接続の開始で 40 秒の見張りを仕掛ける");

  rig.clock.now += 40_000;
  rig.watchdog.runLatest();

  assert.equal(rig.errors, 1, "onerror が来なくても切断として通知する");
  assert.equal(first.closed, true, "半開きの接続を閉じる");
  assert.equal(first.onmessage, null, "閉じた接続のハンドラを外す");
  assert.equal(pendingOf(rig.reconnect).length, 1, "再接続を 1 本だけ予約する");
  assert.equal(rig.reconnect.latest().delayMs, 1000);

  rig.reconnect.runLatest();
  assert.equal(rig.Ctor.instances.length, 2, "予約の後に接続を開き直す");
  assert.equal(pendingOf(rig.watchdog).length, 1, "開き直した接続にだけ見張りを仕掛ける");
  rig.Ctor.instances[1]!.onmessage?.({ data: VALID_STATE });
  assert.equal(rig.states, 1, "開き直した接続の受信で状態が戻る");
  rig.stream.close();
});

test("受信が 39,999ms 間隔で続く間は切断せず、受信のたびに見張りを 40 秒で張り直す", () => {
  const rig = openWatched();
  const source = rig.Ctor.instances[0]!;
  for (let i = 0; i < 5; i += 1) {
    rig.clock.now += 39_999;
    source.onmessage?.({ data: VALID_STATE });
    assert.equal(rig.watchdog.latest().delayMs, 40_000);
    assert.equal(pendingOf(rig.watchdog).length, 1, "有効な見張りは 1 本だけ");
  }
  assert.equal(rig.errors, 0);
  assert.equal(source.closed, false);
  rig.stream.close();
});

test("ちょうど 40,000ms 無音で切断し、39,999ms では切らずに残り 1ms で張り直す", () => {
  const rig = openWatched();
  const source = rig.Ctor.instances[0]!;
  rig.clock.now += 39_999;
  rig.watchdog.runLatest();
  assert.equal(rig.errors, 0, "期限より前の発火では切らない");
  assert.equal(rig.watchdog.latest().delayMs, 1, "残りの時間で張り直す");

  rig.clock.now += 1;
  rig.watchdog.runLatest();
  assert.equal(rig.errors, 1);
  assert.equal(source.closed, true);
});

test("見張りが早く発火しても切らず、壁時計の残り時間で張り直す", () => {
  const rig = openWatched();
  rig.clock.now += 12_345;
  rig.watchdog.runLatest();
  assert.equal(rig.errors, 0);
  assert.equal(rig.watchdog.latest().delayMs, 40_000 - 12_345);
  assert.equal(pendingOf(rig.watchdog).length, 1);
  rig.stream.close();
});

test("不正なペイロードは見張りも連続失敗の回数も更新しない", () => {
  const rig = openWatched();
  const source = rig.Ctor.instances[0]!;
  rig.clock.now += 30_000;
  assert.throws(() => source.onmessage?.({ data: INVALID_STATE }), DomainPrimitiveError);
  assert.equal(rig.watchdog.scheduled.length, 1, "見張りを張り直さない");
  rig.clock.now += 10_000;
  rig.watchdog.runLatest();
  assert.equal(rig.errors, 1, "接続開始から 40 秒で切断する");
});

test("再接続した後の不正なペイロードは連続失敗の回数を戻さず、次の待機は 2 秒のまま", () => {
  const rig = openWatched();
  rig.Ctor.instances[0]!.onerror?.({});
  assert.equal(rig.reconnect.latest().delayMs, 1000);
  rig.reconnect.runLatest();
  const second = rig.Ctor.instances[1]!;
  assert.throws(() => second.onmessage?.({ data: INVALID_STATE }), DomainPrimitiveError);
  second.onerror?.({});
  assert.equal(rig.reconnect.latest().delayMs, 2000, "不正な受信は成功として数えない");
  rig.stream.close();
});

test("見張りで切った後に古い接続から遅れて届いた受信とエラーは無視し、切断のまま再接続も増やさない", () => {
  const rig = openWatched();
  const first = rig.Ctor.instances[0]!;
  const lateMessage = first.onmessage!;
  const lateError = first.onerror!;
  rig.clock.now += 40_000;
  rig.watchdog.runLatest();
  assert.equal(rig.errors, 1);

  lateMessage({ data: VALID_STATE });
  lateError({});
  assert.equal(rig.states, 0, "古い接続の受信で状態を戻さない");
  assert.equal(rig.errors, 1, "古い接続のエラーで切断を重ねない");
  assert.equal(rig.reconnect.scheduled.length, 1, "再接続の予約を増やさない");

  rig.reconnect.runLatest();
  const second = rig.Ctor.instances[1]!;
  lateMessage({ data: VALID_STATE });
  lateError({});
  assert.equal(rig.states, 0, "開き直した後も古い接続の受信で状態を戻さない");
  assert.equal(second.closed, false, "古い接続のエラーで新しい接続を閉じない");
  assert.equal(rig.reconnect.scheduled.length, 1, "二重に再接続しない");
  assert.equal(rig.Ctor.instances.length, 2);
  // 古い接続の受信で、新しい接続の見張りは張り直されない。
  assert.equal(rig.watchdog.scheduled.length, 2);
  rig.stream.close();
});

test("onerror で切った後に古い接続から遅れて届いた受信も無視する", () => {
  const rig = openWatched();
  const first = rig.Ctor.instances[0]!;
  const lateMessage = first.onmessage!;
  first.onerror?.({});
  lateMessage({ data: VALID_STATE });
  assert.equal(rig.states, 0);
  assert.equal(pendingOf(rig.watchdog).length, 0, "切断した接続の見張りは残さない");
  rig.stream.close();
});

test("スリープ復帰の checkStale は、壁時計で 40 秒以上経っていれば即座に切断する", () => {
  const rig = openWatched();
  const source = rig.Ctor.instances[0]!;
  source.onmessage?.({ data: VALID_STATE });
  rig.clock.now += 60_000;
  rig.stream.checkStale();
  assert.equal(rig.errors, 1);
  assert.equal(source.closed, true);
  assert.equal(pendingOf(rig.reconnect).length, 1);
  assert.equal(pendingOf(rig.watchdog).length, 0);
});

test("期限未満の部分スリープ（25 秒）から復帰した checkStale は切らず、残り 15 秒で張り直す", () => {
  const rig = openWatched();
  rig.clock.now += 25_000;
  rig.stream.checkStale();
  assert.equal(rig.errors, 0);
  assert.equal(rig.watchdog.latest().delayMs, 15_000);
  assert.equal(pendingOf(rig.watchdog).length, 1);

  rig.clock.now += 15_000;
  rig.watchdog.runLatest();
  assert.equal(rig.errors, 1, "壁時計の期限どおりに切る");
});

test("壁時計が逆行しても切らず、基準を現在へ詰めて 40 秒で張り直す", () => {
  const rig = openWatched();
  rig.clock.now -= 10_000;
  rig.stream.checkStale();
  assert.equal(rig.errors, 0);
  assert.equal(rig.watchdog.latest().delayMs, 40_000);
  rig.clock.now += 39_999;
  rig.watchdog.runLatest();
  assert.equal(rig.errors, 0, "詰めた基準から 40 秒経つまで切らない");
  rig.clock.now += 1;
  rig.watchdog.runLatest();
  assert.equal(rig.errors, 1);
});

test("切断中（再接続待ち）と close の後の checkStale は何もしない", () => {
  const rig = openWatched();
  rig.Ctor.instances[0]!.onerror?.({});
  rig.clock.now += 100_000;
  rig.stream.checkStale();
  assert.equal(rig.errors, 1);
  assert.equal(rig.reconnect.scheduled.length, 1);
  rig.stream.close();
  rig.stream.checkStale();
  assert.equal(rig.errors, 1);
  assert.equal(rig.Ctor.instances.length, 1);
});

test("close は見張りも再接続の予約も取り消し、取消の後に発火しても何も起きない", () => {
  const rig = openWatched();
  const watch = rig.watchdog.latest();
  rig.stream.close();
  assert.equal(watch.cancelled, true);
  rig.clock.now += 100_000;
  watch.run();
  assert.equal(rig.errors, 0);
  assert.equal(rig.Ctor.instances.length, 1);
});

test("onState の中で close しても見張りを残さない", () => {
  const rig = openWatched({}, { onState: () => rig.stream.close() });
  rig.Ctor.instances[0]!.onmessage?.({ data: VALID_STATE });
  assert.equal(pendingOf(rig.watchdog).length, 0);
  assert.equal(rig.Ctor.instances[0]!.closed, true);
});

test("onError の中で close したら再接続を予約しない", () => {
  const rig = openWatched({}, { onError: () => rig.stream.close() });
  rig.clock.now += 40_000;
  rig.watchdog.runLatest();
  assert.equal(rig.errors, 1);
  assert.equal(rig.reconnect.scheduled.length, 0);
  assert.equal(pendingOf(rig.watchdog).length, 0);
});

test("同期で実行するスケジューラでも、完了した予約で次の予約を上書きしない", () => {
  const Ctor = freshFakes();
  const clock = { now: 0 };
  const cancelled: string[] = [];
  let reconnects = 0;
  // 再接続の予約はその場で実行する。2 本目の接続は見張りの期限切れで切れ、その後の予約は残す。
  const watchdog = new FakeScheduler();
  const stream = openStateStream({
    url: "/api/state",
    createEventSource: (url) => new Ctor(url),
    onState: () => {},
    now: () => clock.now,
    scheduleWatchdog: watchdog.schedule,
    scheduleReconnect: (run) => {
      reconnects += 1;
      if (reconnects === 1) {
        run();
        return () => cancelled.push("sync");
      }
      return () => cancelled.push("async");
    },
  });
  Ctor.instances[0]!.onerror?.({});
  assert.equal(Ctor.instances.length, 2, "同期の予約で接続を開き直す");
  clock.now += 40_000;
  watchdog.runLatest();
  assert.equal(reconnects, 2);
  stream.close();
  assert.deepEqual(cancelled, ["async"], "close は生きている予約だけを取り消す");
});

test("同期で 1 回だけ発火する見張りスケジューラでも、張り直した見張りを失わない", () => {
  const Ctor = freshFakes();
  const clock = { now: 0 };
  const later = new FakeScheduler();
  let calls = 0;
  const stream = openStateStream({
    url: "/api/state",
    createEventSource: (url) => new Ctor(url),
    onState: () => {},
    now: () => clock.now,
    scheduleReconnect: new FakeScheduler().schedule,
    scheduleWatchdog: (run, delayMs) => {
      calls += 1;
      if (calls === 1) {
        // 最初の予約はその場で発火する（期限前なので張り直しになる）。
        run();
        return () => {};
      }
      return later.schedule(run, delayMs);
    },
  });
  assert.equal(pendingOf(later).length, 1, "張り直した見張りが残る");
  clock.now += 40_000;
  later.runLatest();
  assert.equal(Ctor.instances[0]!.closed, true, "張り直した見張りで切断する");
  stream.close();
});

for (const bad of [0, -1, Number.NaN, Number.POSITIVE_INFINITY, 2_147_483_648]) {
  test(`staleTimeoutMs に ${bad} を渡すと RangeError で拒み、接続を開かない`, () => {
    const Ctor = freshFakes();
    assert.throws(() => open(Ctor, {}, undefined, { staleTimeoutMs: bad }), RangeError);
    assert.equal(Ctor.instances.length, 0);
  });
}

// ===== 状態機械の性質テスト（PBT、issue #383） =====
//
// 単調時計（タイマー）と壁時計（now）を別々に持ち、時刻の進行、スリープ（壁時計だけ進む）、
// 壁時計の逆行、見張りの早期発火、受信・不正受信・エラー、古い接続からの遅延イベント、
// 再接続の発火、checkStale、close をシード固定の乱数で並べ、各手のあとに不変条件を確かめる。

const mulberry32 = (seed: number): (() => number) => {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
};

type Timer = { kind: "watchdog" | "reconnect"; due: number; run: () => void; cancelled: boolean; fired: boolean };

class PbtSource implements EventSourceLike {
  messageHandler: ((event: { data: string }) => void) | null = null;
  errorHandler: ((event: unknown) => void) | null = null;
  private message: ((event: { data: string }) => void) | null = null;
  private error: ((event: unknown) => void) | null = null;
  closed = false;
  get onmessage() {
    return this.message;
  }
  set onmessage(handler) {
    this.message = handler;
    if (handler !== null) this.messageHandler = handler;
  }
  get onerror() {
    return this.error;
  }
  set onerror(handler) {
    this.error = handler;
    if (handler !== null) this.errorHandler = handler;
  }
  close() {
    this.closed = true;
  }
}

const runStateMachine = (seed: number, steps: number): void => {
  const rand = mulberry32(seed);
  const int = (min: number, max: number) => min + Math.floor(rand() * (max - min + 1));
  const TIMEOUT = STATE_STREAM_STALE_TIMEOUT_MS;
  const where = (step: number, op: string) => `seed=${seed} step=${step} op=${op}`;

  let mono = 0;
  let wall = 1_000_000;
  const timers: Timer[] = [];
  const conns: PbtSource[] = [];
  const received = new Set<PbtSource>();
  let states = 0;
  let errors = 0;
  let disconnected = false;
  let closed = false;
  let expectedLast = wall;
  // 現在の見張りを仕掛けた後に、壁時計だけが動いたか（スリープ・逆行）。
  let wallSkewSinceArm = false;
  let currentStep = 0;
  let currentOp = "init";

  const live = (kind: Timer["kind"]) => timers.filter((t) => t.kind === kind && !t.cancelled && !t.fired);
  const openConns = () => conns.filter((c) => !c.closed);

  // 見張りの評価（発火・checkStale）の前後で、性質 5・6 を確かめる。
  const evaluate = (fire: () => void) => {
    const open = openConns()[0];
    const elapsed = wall - expectedLast;
    const errorsBefore = errors;
    fire();
    const msg = where(currentStep, currentOp);
    if (open === undefined || closed) {
      assert.equal(errors, errorsBefore, msg);
      return;
    }
    if (elapsed < 0) {
      assert.equal(errors, errorsBefore, `逆行では切らない ${msg}`);
      expectedLast = wall;
      assert.equal(live("watchdog")[0]?.due, mono + TIMEOUT, `逆行では 40 秒で張り直す ${msg}`);
    } else if (elapsed < TIMEOUT) {
      assert.equal(errors, errorsBefore, `期限前は切らない ${msg}`);
      assert.equal(live("watchdog")[0]?.due, mono + (TIMEOUT - elapsed), `残り時間で張り直す ${msg}`);
    } else {
      assert.equal(errors, errorsBefore + 1, `期限を過ぎたら切る ${msg}`);
      assert.equal(open.closed, true, msg);
      assert.equal(live("reconnect").length, 1, msg);
    }
  };

  const schedule =
    (kind: Timer["kind"]) =>
    (run: () => void, delayMs: number): (() => void) => {
      if (kind === "watchdog") wallSkewSinceArm = false;
      const timer: Timer = { kind, due: mono + delayMs, run, cancelled: false, fired: false };
      timers.push(timer);
      return () => {
        timer.cancelled = true;
      };
    };

  const fireTimer = (timer: Timer) => {
    timer.fired = true;
    if (timer.kind === "watchdog") evaluate(timer.run);
    else timer.run();
  };

  const stream = openStateStream({
    url: "/api/state",
    createEventSource: () => {
      assert.equal(closed, false, "close の後に接続を作らない");
      const source = new PbtSource();
      conns.push(source);
      expectedLast = wall;
      return source;
    },
    onState: () => {
      assert.equal(closed, false, "close の後に onState を呼ばない");
      states += 1;
      disconnected = false;
      expectedLast = wall;
    },
    onError: () => {
      assert.equal(closed, false, "close の後に onError を呼ばない");
      errors += 1;
      disconnected = true;
    },
    now: () => wall,
    scheduleWatchdog: schedule("watchdog"),
    scheduleReconnect: schedule("reconnect"),
  });

  const ignored = (fire: () => void) => {
    const before = { states, errors, conns: conns.length, timers: timers.length };
    fire();
    assert.deepEqual({ states, errors, conns: conns.length, timers: timers.length }, before, where(currentStep, currentOp));
  };

  const ops: Array<[string, number, () => void]> = [
    ["advanceBoth", 6, () => {
      const target = mono + int(1, 20_000);
      for (;;) {
        const next = timers.filter((t) => !t.cancelled && !t.fired && t.due <= target).sort((a, b) => a.due - b.due)[0];
        if (next === undefined) break;
        wall += next.due - mono;
        mono = next.due;
        fireTimer(next);
      }
      wall += target - mono;
      mono = target;
      if (!wallSkewSinceArm && openConns().length === 1 && !closed) {
        assert.ok(wall - expectedLast < TIMEOUT, `時刻が進んで期限に達した接続は切れている ${where(currentStep, currentOp)}`);
      }
    }],
    ["sleep", 2, () => {
      wall += int(1, 70_000);
      wallSkewSinceArm = true;
    }],
    ["rewind", 1, () => {
      wall -= int(1, 10_000);
      wallSkewSinceArm = true;
    }],
    ["earlyWatchdog", 2, () => {
      const timer = live("watchdog")[0];
      if (timer !== undefined) fireTimer(timer);
    }],
    ["message", 5, () => {
      const source = openConns()[0];
      if (source === undefined) return;
      const before = states;
      source.onmessage?.({ data: VALID_STATE });
      assert.equal(states, before + 1);
      received.add(source);
      assert.equal(live("watchdog")[0]?.due, mono + TIMEOUT, "受信で 40 秒の見張りを張り直す");
    }],
    ["invalid", 1, () => {
      const source = openConns()[0];
      if (source === undefined) return;
      const dueBefore = live("watchdog")[0]?.due;
      assert.throws(() => source.onmessage?.({ data: INVALID_STATE }));
      assert.equal(live("watchdog")[0]?.due, dueBefore, "不正な受信で見張りを張り直さない");
    }],
    ["error", 2, () => {
      const source = openConns()[0];
      if (source === undefined) return;
      source.onerror?.({});
      assert.equal(source.closed, true);
    }],
    ["staleMessage", 2, () => {
      const old = conns.filter((c) => c.closed);
      const source = old[int(0, old.length - 1)];
      if (source?.messageHandler) ignored(() => source.messageHandler?.({ data: VALID_STATE }));
    }],
    ["staleError", 2, () => {
      const old = conns.filter((c) => c.closed);
      const source = old[int(0, old.length - 1)];
      if (source?.errorHandler) ignored(() => source.errorHandler?.({}));
    }],
    ["reconnect", 3, () => {
      const timer = live("reconnect")[0];
      if (timer !== undefined) fireTimer(timer);
    }],
    ["checkStale", 3, () => evaluate(() => stream.checkStale())],
    // 取り消した予約がタイマーの都合で遅れて発火しても、何も起こさない。
    ["cancelledTimer", 2, () => {
      const cancelled = timers.filter((t) => t.cancelled && !t.fired);
      const timer = cancelled[int(0, cancelled.length - 1)];
      if (timer !== undefined) ignored(() => timer.run());
    }],
    ["close", 1, () => {
      if (rand() < 0.3) {
        stream.close();
        closed = true;
      }
    }],
  ];
  const totalWeight = ops.reduce((sum, [, weight]) => sum + weight, 0);
  const pick = () => {
    let roll = rand() * totalWeight;
    for (const op of ops) {
      roll -= op[1];
      if (roll < 0) return op;
    }
    return ops[0]!;
  };

  for (currentStep = 0; currentStep < steps; currentStep += 1) {
    const [name, , run] = pick();
    currentOp = name;
    const before = { states, errors };
    run();
    const msg = where(currentStep, currentOp);
    if (closed) {
      if (name !== "close") assert.deepEqual({ states, errors }, before, `close の後は通知しない ${msg}`);
      assert.equal(openConns().length, 0, msg);
      assert.equal(live("watchdog").length, 0, msg);
      assert.equal(live("reconnect").length, 0, msg);
      continue;
    }
    const opened = openConns();
    assert.ok(opened.length <= 1, `開いた接続は 1 本まで ${msg}`);
    assert.ok(live("reconnect").length <= 1, `再接続の予約は 1 本まで ${msg}`);
    if (opened.length === 1) assert.equal(live("reconnect").length, 0, `接続中は再接続を予約しない ${msg}`);
    assert.equal(live("watchdog").length, opened.length, `見張りは開いた接続 1 本にだけ付く ${msg}`);
    if (!disconnected) {
      const current = opened[0];
      assert.ok(current !== undefined, `接続中なら現在の接続がある ${msg}`);
      assert.ok(current === conns[0] || received.has(current), `開き直した接続は自身の受信で戻る ${msg}`);
    }
    if (opened.length === 0) assert.ok(disconnected, `接続が無いなら切断の扱い ${msg}`);
  }
  stream.close();
  closed = true;
  assert.equal(live("watchdog").length + live("reconnect").length + openConns().length, 0);
};

test("性質: 200 系列 × 60 手のどの並びでも、接続・見張り・再接続の不変条件を守る", () => {
  for (let seed = 1; seed <= 200; seed += 1) {
    runStateMachine(0x383 * 1000 + seed, 60);
  }
});

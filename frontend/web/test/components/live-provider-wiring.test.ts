// 概要と判断待ちとゴール一覧の画面が、StateStreamProvider の Context 値を useLiveState 経由で消費する契約と、
// SSE の受信・切断を本番と同じ結線で Context 値へ反映する結線の契約（issue #20 の配線層）。
//
// 本番のハンドラ結線は stateStreamHandlers に一本化されている。ここでは本番と同じ
// stateStreamHandlers → reduceLiveState を通して遷移を検証し、接続状態の LiveOverview／LiveDecisions を
// StateStreamProvider 配下で描画して観測する。切断時に通知が出ることは、内容コンポーネントの
// prop 契約を固定する live-disconnected.test.ts が担保する。StateStreamProvider は初期値を
// initialState から決めるため、初期読み込みの成否が渡されない限り SSR では切断状態の
// Live* を描画しない（未確認として扱う）。初期読み込みが失敗した場合は
// initialFailure から、その失敗理由を持って切断状態で始まり、起動時の不通でも
// 画面側の通知が失敗理由に合った文面で1回だけ出る（issue #92）。ゴール一覧の通知も
// 同じ表示元の形であり、起動時の不通で1回だけ出ることをここで固定する（issue #252）。
//
// ファイルの後半では、StateStreamProvider を jsdom の DOM へ実際にマウントして結線を
// 検証する（issue #89）。SSR は useEffect を実行しないため、Provider の useEffect が
// openStateStream へ接続し、SSE の受信と切断が Context と画面へ反映されることと、
// router.refresh 相当の再描画でも購読が維持されることは、マウントしなければ検証できない。
import test from "node:test";
import assert from "node:assert/strict";
import { act, createElement, StrictMode, type ComponentProps, type ReactElement, type ReactNode } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { createRoot, type Root } from "react-dom/client";
import { JSDOM } from "jsdom";
import { ConnectionState, DashboardState } from "../../src/proto/taktdash.ts";
import { createDashboardState, NOW } from "../support/fixtures.ts";
import { parseStreamUrl } from "../support/stream-url.ts";
import { assertUnreadRunList, assertRejectsUnreadEmptyClaim } from "../support/run-list.ts";
import { dashboardStateFromProto } from "../../src/state/restore-state.ts";
import { formatClock } from "../../src/state/format.ts";
import { LiveOverview } from "../../src/components/live-overview.tsx";
import { LiveDecisions } from "../../src/components/live-decisions.tsx";
import { LiveGoals } from "../../src/components/live-goals.tsx";
import { LiveSidebar } from "../../src/components/live-sidebar.tsx";
import {
  reduceLiveState,
  stateStreamHandlers,
  StateStreamProvider,
  useLiveState,
  type StateStreamContextValue,
} from "../../src/components/state-provider.tsx";
import {
  BACKEND_NO_RESPONSE_MESSAGE,
  BACKEND_UNAVAILABLE_MESSAGE,
  BACKEND_UNREACHABLE_LONG_MESSAGE,
  BACKEND_UNREACHABLE_MESSAGE,
  backendUnavailableMessage,
} from "../../src/components/backend-unavailable-notice.tsx";
import type { BackendFailure, BackendUnavailableSituation } from "../../src/grpc/client.ts";
import { DomainPrimitiveError } from "../../src/state/domain-primitives.ts";

// 本番と同じ結線（stateStreamHandlers → reduceLiveState）を通して、現在の Context 値を
// 進めるハンドラを作る。テスト内の値はこの関数だけで更新し、結線の取り違えを検出する。
const drive = (initial: StateStreamContextValue) => {
  const holder: { current: StateStreamContextValue } = { current: initial };
  const handlers = stateStreamHandlers((event) => {
    holder.current = reduceLiveState(holder.current, event);
  });
  return { holder, handlers };
};

// 本番の唯一の入口（app/layout.tsx）と同じ形で StateStreamProvider を描画する。
// initialState だけを渡し、追加の供給口は使わない。initialState は RSC が渡す
// proto の状態であり、Context 値の内部型とは別であることに注意する。
// children は props の必須項目のため、ComponentProps で型付けした props ごと渡す
// （createElement の残余引数形式は React 19 の型では必須 children と両立せず、
// props へ直接書く形式は lint の react/no-children-prop と衝突するため）。
const renderLive = (
  page: typeof LiveOverview | typeof LiveDecisions,
  state: DashboardState,
): string => {
  const props: ComponentProps<typeof StateStreamProvider> = { initialSnapshotLoaded: true,
    initialState: state,
    children: createElement(page),
  };
  return renderToStaticMarkup(createElement(StateStreamProvider, props));
};

const connected = (): StateStreamContextValue => ({
  state: dashboardStateFromProto(createDashboardState()),
  snapshotLoaded: true,
  disconnected: false,
});

test("SSE の受信で切断フラグが消え、切断で立ち、切断中も直前の state を保持する", () => {
  const initial = connected();
  const { holder, handlers } = drive(initial);

  handlers.onError();
  assert.equal(holder.current.disconnected, true, "エラーで切断フラグが立つ");
  assert.equal(holder.current.state, initial.state, "切断中も直前の state を保持する");

  const next = dashboardStateFromProto(createDashboardState());
  handlers.onState(next);
  assert.equal(holder.current.disconnected, false, "受信で切断フラグが消える");
  assert.equal(holder.current.state, next, "受信で state を置き換える");
});

test("切断の開始時刻と接続履歴を Context 値に保持する", () => {
  const initial: StateStreamContextValue = {
    state: dashboardStateFromProto(createDashboardState()),
    snapshotLoaded: true,
    disconnected: false,
    disconnectedSince: null,
    respondedBefore: true,
  };
  const { holder, handlers } = drive(initial);

  handlers.onError();
  assert.equal(holder.current.disconnected, true, "エラーで切断フラグが立つ");
  assert.ok(holder.current.disconnectedSince instanceof Date, "切断の開始時刻を保持する");
  assert.equal(holder.current.respondedBefore, true, "接続履歴は切断でも消えない");
  const since = holder.current.disconnectedSince;

  handlers.onError();
  assert.equal(holder.current.disconnectedSince, since, "切断が続く間は開始時刻を保つ");

  handlers.onState(dashboardStateFromProto(createDashboardState()));
  assert.equal(holder.current.disconnected, false, "受信で切断フラグが消える");
  assert.equal(holder.current.disconnectedSince, null, "受信で切断の開始時刻を消す");
  assert.equal(holder.current.respondedBefore, true, "受信で接続履歴が残る");
});

test("概要は Provider の Context が接続中なら useLiveState 経由で通知を描画しない", () => {
  const html = renderLive(LiveOverview, createDashboardState());
  assert.doesNotMatch(html, /backend に接続できません/);
});

test("概要は Provider の Context が接続中なら最終読み取り時刻のタイルを持たない", () => {
  const html = renderLive(LiveOverview, createDashboardState());
  assert.doesNotMatch(html, /最終読み取り時刻/, "最終読み取り時刻のタイルはサイドバーへ移した");
  assert.match(html, /Mac mini/, "サーバーの表示を残す");
});

test("判断待ちは Provider の Context が接続中なら useLiveState 経由で通知を描画しない", () => {
  const html = renderLive(LiveDecisions, createDashboardState());
  assert.doesNotMatch(html, /backend に接続できません/);
});

test("判断待ちは Provider の Context が接続中でも最後に読めた状態の表示を残す", () => {
  const html = renderLive(LiveDecisions, createDashboardState());
  assert.match(html, /判断待ち/, "最後に読めた判断待ちの表示を残す");
  // 見出しや脚注にも「判断待ち」が含まれるため、状態に由来する一覧の値で
  // Context の state が一覧へ届いていることを確かめる。
  assert.match(html, /corporate-site/, "最後に読めた判断待ちの対象プロジェクトを残します");
  assert.match(html, /プルリクエスト #418/, "最後に読めた判断待ちの本文を残します");
});

test("RSC の初期状態は検証を通してから Context に載せ、必須の値が空なら描画しない", () => {
  const invalid = DashboardState.create({ servers: [{ serverId: "" }] });

  const props: ComponentProps<typeof StateStreamProvider> = { initialSnapshotLoaded: true,
    initialState: invalid,
    children: createElement(LiveOverview),
  };
  assert.throws(
    () => renderToStaticMarkup(createElement(StateStreamProvider, props)),
    (error) => error instanceof DomainPrimitiveError && error.stage === "size",
  );
});

// Provider の初期 Context 値を、子が useLiveState で受け取った値として観測する。
// 本番の入口（app/layout.tsx）が渡す prop の組み合わせをそのまま与え、初期値の
// 決め方そのものを検証する。
const renderInitialValue = (props: {
  initialFailure?: BackendFailure;
  initialRespondedBefore?: boolean;
}): StateStreamContextValue => {
  const holder: { current?: StateStreamContextValue } = {};
  const Probe = (): null => {
    holder.current = useLiveState();
    return null;
  };
  const providerProps: ComponentProps<typeof StateStreamProvider> = { initialSnapshotLoaded: true,
    initialState: createDashboardState(),
    ...props,
    children: createElement(Probe),
  };
  renderToStaticMarkup(createElement(StateStreamProvider, providerProps));
  const current = holder.current;
  if (current === undefined) {
    throw new Error("Provider の初期値を観測できませんでした。");
  }
  return current;
};

// LiveOverview／LiveDecisions と同じ組み立てで、切断表示の状況を作る。
const disconnectedSituation = (value: StateStreamContextValue, now: Date): BackendUnavailableSituation => ({
  failure: value.failure ?? "unreachable",
  respondedBefore: value.respondedBefore ?? true,
  ...(value.disconnectedSince !== undefined && value.disconnectedSince !== null
    ? { durationMs: Math.max(0, now.getTime() - value.disconnectedSince.getTime()) }
    : {}),
});

test("再読み込みで今回の読み込みが失敗しても、サーバー側の接続履歴があれば履歴を落とさない", () => {
  // 取得成功 → backend 停止 → キャッシュを使った再読み込み、という経路を prop で表す。
  // 今回の読み込みは失敗（initialFailure）でも、過去の応答履歴は true で渡る。
  const initial = renderInitialValue({ initialFailure: "unreachable", initialRespondedBefore: true });
  assert.equal(initial.respondedBefore, true, "過去の応答履歴が初期値に残る");

  // SSE 接続も失敗する。接続履歴は消えず、切断の開始時刻を持つ。
  const disconnected = reduceLiveState(initial, { type: "error", at: new Date() });
  assert.equal(disconnected.disconnected, true, "SSE の失敗で切断フラグが立つ");
  assert.equal(disconnected.respondedBefore, true, "SSE の失敗でも接続履歴は消えない");

  // 切断直後は設定の確認を出さず、短い文面にする。
  const shortMessage = backendUnavailableMessage(disconnectedSituation(disconnected, new Date()));
  assert.equal(shortMessage, BACKEND_UNREACHABLE_MESSAGE, "設定の確認を出さない");
  assert.doesNotMatch(shortMessage, /TAKTDASH_BACKEND_ADDR/);

  // 切断が60秒を超えたら、起動の確認へ切り替わる。
  const longDisconnected = reduceLiveState(initial, { type: "error", at: new Date(Date.now() - 120_000) });
  const longMessage = backendUnavailableMessage(disconnectedSituation(longDisconnected, new Date()));
  assert.equal(longMessage, BACKEND_UNREACHABLE_LONG_MESSAGE, "継続時間で起動の確認へ切り替わる");
});

test("一度も応答を受けていない場合は、読み込みの失敗と SSE の失敗で設定の確認を出し続ける", () => {
  const initial = renderInitialValue({ initialFailure: "unreachable", initialRespondedBefore: false });
  assert.equal(initial.respondedBefore, false, "応答履歴が無ければ初期値も false になる");

  const longDisconnected = reduceLiveState(initial, { type: "error", at: new Date(Date.now() - 120_000) });
  const message = backendUnavailableMessage(disconnectedSituation(longDisconnected, new Date()));
  assert.equal(message, BACKEND_UNAVAILABLE_MESSAGE, "継続時間が長くても設定の確認を出す");
});

test("接続履歴の prop が無い場合は、今回の読み込みの成否から接続履歴の初期値を決める", () => {
  assert.equal(renderInitialValue({ initialFailure: "unreachable" }).respondedBefore, false);
  assert.equal(renderInitialValue({}).respondedBefore, true);
});

// 初期読み込みが失敗したときの本番の入口（app/layout.tsx）と同じ prop の組み合わせで
// StateStreamProvider を描画する。レイアウトは通知を描画せず、初期読み込みの成否と
// 失敗理由を Provider へ渡す（issue #92）。
const renderLiveWithInitialFailure = (
  page: typeof LiveOverview | typeof LiveDecisions,
  state: DashboardState,
  failure: BackendFailure,
  respondedBefore: boolean,
): string => {
  const props: ComponentProps<typeof StateStreamProvider> = { initialSnapshotLoaded: false,
    initialState: state,
    initialFailure: failure,
    initialRespondedBefore: respondedBefore,
    children: createElement(page),
  };
  return renderToStaticMarkup(createElement(StateStreamProvider, props));
};

// ゴール一覧は props の形が LiveOverview と共通項を持たず、createElement の
// オーバーロード解決で共用ヘルパーの合併型へ混ぜられないため、専用の入口で描画する。
const renderGoalsWithInitialFailure = (
  state: DashboardState,
  failure: BackendFailure,
  respondedBefore: boolean,
): string => {
  const props: ComponentProps<typeof StateStreamProvider> = { initialSnapshotLoaded: false,
    initialState: state,
    initialFailure: failure,
    initialRespondedBefore: respondedBefore,
    children: createElement(LiveGoals),
  };
  return renderToStaticMarkup(createElement(StateStreamProvider, props));
};

const countOccurrences = (html: string, text: string): number => html.split(text).length - 1;

test("初期読み込みが失敗した場合は Provider が切断状態で始まる", () => {
  const initial = renderInitialValue({ initialFailure: "unreachable", initialRespondedBefore: false });
  assert.equal(initial.disconnected, true, "初期読み込みの失敗を切断の初期値へ反映する");
  assert.equal(initial.failure, "unreachable", "失敗理由も初期値に残る");
  assert.equal(renderInitialValue({}).disconnected, false);
});

test("初期読み込みの失敗で切断状態で始まっても、SSE の受信で切断が消える", () => {
  const initial = renderInitialValue({ initialFailure: "unreachable", initialRespondedBefore: false });
  assert.equal(initial.disconnected, true, "起動時は切断状態で始まる");
  const { holder, handlers } = drive(initial);
  handlers.onState(dashboardStateFromProto(createDashboardState()));
  assert.equal(holder.current.disconnected, false, "受信で切断フラグが消える");
});

test("初期読み込みの失敗理由は SSE の切断でも保持され、状態の受信で消える", () => {
  const initial = renderInitialValue({ initialFailure: "no-response", initialRespondedBefore: false });
  assert.equal(initial.failure, "no-response", "初期読み込みの失敗理由を初期値に残す");
  const { holder, handlers } = drive(initial);

  handlers.onError();
  assert.equal(holder.current.disconnected, true, "SSE の失敗で切断フラグが立つ");
  assert.equal(holder.current.failure, "no-response", "切断が続く間は初期の失敗理由を引き継ぐ");
  assert.equal(
    backendUnavailableMessage(disconnectedSituation(holder.current, new Date())),
    BACKEND_NO_RESPONSE_MESSAGE,
    "応答なしの文面のままになる",
  );

  handlers.onState(dashboardStateFromProto(createDashboardState()));
  assert.equal(holder.current.disconnected, false, "受信で切断フラグが消える");
  assert.equal(holder.current.failure, undefined, "受信で失敗理由を消す");

  handlers.onError();
  assert.equal(holder.current.failure, "unreachable", "状態を受けてからの切断は接続不可として扱う");
});

test("概要は初期読み込みが接続不可で失敗した場合に通知を1回だけ描画する", () => {
  const html = renderLiveWithInitialFailure(LiveOverview, createDashboardState(), "unreachable", false);
  assert.equal(
    countOccurrences(html, BACKEND_UNAVAILABLE_MESSAGE),
    1,
    "起動時の不通でも通知は画面側の1回だけにする",
  );
});

test("判断待ちは初期読み込みが接続不可で失敗した場合に通知を1回だけ描画する", () => {
  const html = renderLiveWithInitialFailure(LiveDecisions, createDashboardState(), "unreachable", false);
  assert.equal(
    countOccurrences(html, BACKEND_UNAVAILABLE_MESSAGE),
    1,
    "起動時の不通でも通知は画面側の1回だけにする",
  );
});

test("概要は初期読み込みが応答なしで失敗した場合に、応答なしの文面の通知を1回だけ描画する", () => {
  const html = renderLiveWithInitialFailure(LiveOverview, createDashboardState(), "no-response", false);
  assert.equal(
    countOccurrences(html, BACKEND_NO_RESPONSE_MESSAGE),
    1,
    "応答なし専用の文面が画面側の1回だけ出る",
  );
  assert.equal(countOccurrences(html, BACKEND_UNAVAILABLE_MESSAGE), 0, "接続不可の文面は出ない");
});

test("判断待ちは初期読み込みが応答なしで失敗した場合に、応答なしの文面の通知を1回だけ描画する", () => {
  const html = renderLiveWithInitialFailure(LiveDecisions, createDashboardState(), "no-response", false);
  assert.equal(
    countOccurrences(html, BACKEND_NO_RESPONSE_MESSAGE),
    1,
    "応答なし専用の文面が画面側の1回だけ出る",
  );
  assert.equal(countOccurrences(html, BACKEND_UNAVAILABLE_MESSAGE), 0, "接続不可の文面は出ない");
});

test("ゴール一覧は初期読み込みが接続不可で失敗した場合に通知を1回だけ描画する", () => {
  const html = renderGoalsWithInitialFailure(createDashboardState(), "unreachable", false);
  assert.equal(
    countOccurrences(html, BACKEND_UNAVAILABLE_MESSAGE),
    1,
    "起動時の不通でも通知は画面側の1回だけにする",
  );
});

test("ゴール一覧は初期読み込みが応答なしで失敗した場合に、応答なしの文面の通知を1回だけ描画する", () => {
  const html = renderGoalsWithInitialFailure(createDashboardState(), "no-response", false);
  assert.equal(
    countOccurrences(html, BACKEND_NO_RESPONSE_MESSAGE),
    1,
    "応答なし専用の文面が画面側の1回だけ出る",
  );
  assert.equal(countOccurrences(html, BACKEND_UNAVAILABLE_MESSAGE), 0, "接続不可の文面は出ない");
});

// ===== 実際のマウントによる結線の検証（issue #89） =====
//
// ここまでの検証は、stateStreamHandlers → reduceLiveState の純粋な遷移と、
// renderToStaticMarkup による初期描画が対象だった。SSR は useEffect を実行しないため、
// StateStreamProvider の useEffect が openStateStream へ正しく接続し、受信と切断が
// Context と画面へ反映されることは未検証だった。ここでは jsdom の DOM へ実際に
// マウントし、ブラウザの代わりに差し替えた EventSource を通して結線全体を検証する。
//
// StateStreamProviderWithRefresh の useRouter 呼び出し自体は Next のランタイムが
// 要るため対象外とする。router.refresh の影響は Provider の層では initialState prop の
// 差し替えと revalidate prop の同一性として現れるため、購読の契約は Provider の層で
// 固定する（ラッパーは useRouter を閉じ込めるだけの薄い部品である）。

// ブラウザの EventSource の代わり。Provider の useEffect はグローバルの EventSource を
// new するため、テストではこのクラスをグローバルへ差し替えて接続を観測する。
class FakeEventSource {
  static instances: FakeEventSource[] = [];
  onmessage: ((event: { data: string }) => void) | null = null;
  onerror: ((event: unknown) => void) | null = null;
  closed = false;
  constructor(public readonly url: string) {
    FakeEventSource.instances.push(this);
  }
  close(): void {
    this.closed = true;
  }
}

let domInstalled = false;

// react-dom/client のマウントに要る DOM を jsdom で用意し、グローバルへ公開する。
// Node がすでに持つグローバル（setTimeout など）は上書きしない。window と document と
// navigator だけは、Node 側の有無や getter の有無によらず jsdom のものへ差し替える。
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

type ProviderProps = Omit<ComponentProps<typeof StateStreamProvider>, "children">;

const renderProvider = async (root: Root, props: ProviderProps, children: ReactNode): Promise<void> => {
  const providerProps: ComponentProps<typeof StateStreamProvider> = { ...props, children };
  await act(async () => {
    root.render(createElement(StateStreamProvider, providerProps));
  });
};

// StateStreamProvider を jsdom の DOM へマウントする。EventSource はテスト用の
// クラスへ差し替え、作られた接続を FakeEventSource.instances で観測する。
const mountLive = async (
  props: ProviderProps,
  children: ReactNode,
): Promise<{ container: HTMLDivElement; root: Root; unmount: () => Promise<void> }> => {
  installDomEnvironment();
  FakeEventSource.instances = [];
  (globalThis as unknown as { EventSource: typeof FakeEventSource }).EventSource = FakeEventSource;
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  await renderProvider(root, props, children);
  return {
    container,
    root,
    unmount: async () => {
      await act(async () => {
        root.unmount();
      });
      container.remove();
    },
  };
};

// Context 値を子から観測するプローブ。現在の値を保持し、接続状態を文字でも描く。
const createProbe = (): { holder: { current?: StateStreamContextValue }; Probe: () => ReactElement } => {
  const holder: { current?: StateStreamContextValue } = {};
  const Probe = (): ReactElement => {
    const value = useLiveState();
    holder.current = value;
    return createElement("output", null, value.disconnected ? "disconnected" : "connected");
  };
  return { holder, Probe };
};

const fireMessage = async (source: FakeEventSource, data: string): Promise<void> => {
  await act(async () => {
    source.onmessage?.({ data });
  });
};

const fireError = async (source: FakeEventSource): Promise<void> => {
  await act(async () => {
    source.onerror?.({});
  });
};

for (const respondedBefore of [false, true]) {
  test(`概要未取得なら他RPCの応答履歴${respondedBefore}でも両画面のサーバー台数は不明`, () => {
    const props: ComponentProps<typeof StateStreamProvider> = {
      initialState: DashboardState.create({}), initialSnapshotLoaded: false,
      initialFailure: "unreachable", initialRespondedBefore: respondedBefore,
      children: [createElement(LiveOverview, { key: "overview", initialNowMs: NOW.getTime() }),
        createElement(LiveSidebar, { key: "sidebar", frontendBuild: { commit: "test", branch: "test" } })],
    };
    const page = new JSDOM(renderToStaticMarkup(createElement(StateStreamProvider, props))).window.document;
    for (const selector of [".page-head > div > p", ".side-foot > p:nth-of-type(2)"]) {
      const count = page.querySelector(selector);
      assert.ok(count);
      assert.match(count.textContent ?? "", /サーバー.*不明/);
      assert.doesNotMatch(count.textContent ?? "", /0\s*台/);
    }
  });
}

test("同じ概要とサイドバーは台数不明から取得、切断中の保持、正常な0台の再受信へ追従する", async () => {
  const base = createDashboardState();
  const loaded = DashboardState.create({ ...base,
    servers: base.servers.filter((server) => server.connected),
  });
  const { container, unmount } = await mountLive({
    initialState: DashboardState.create({}), initialSnapshotLoaded: false, initialFailure: "unreachable",
  }, [createElement(LiveOverview, { key: "overview", initialNowMs: NOW.getTime() }),
    createElement(LiveSidebar, { key: "sidebar", frontendBuild: { commit: "test", branch: "test" } })]);
  const counts = (): string[] => [".page-head > div > p", ".side-foot > p:nth-of-type(2)"].map((selector) => {
    const element = container.querySelector(selector);
    assert.ok(element);
    return element.textContent ?? "";
  });
  try {
    for (const count of counts()) assert.match(count, /サーバー.*不明/);
    const source = FakeEventSource.instances[0];
    assert.ok(source);
    await fireMessage(source, JSON.stringify(DashboardState.toJSON(loaded)));
    assert.match(counts()[0] ?? "", /1台のサーバー/);
    assert.match(counts()[1] ?? "", /1台中1台/);
    await fireError(source);
    assert.match(counts()[0] ?? "", /1台のサーバー/);
    assert.match(counts()[1] ?? "", /1台中1台/);
    await waitForConnections(2);
    const reconnected = FakeEventSource.instances[1];
    assert.ok(reconnected);
    await fireMessage(reconnected, JSON.stringify(DashboardState.toJSON(DashboardState.create({}))));
    assert.match(counts()[0] ?? "", /0台のサーバー/);
    assert.match(counts()[1] ?? "", /0台中0台/);
    assert.equal(container.querySelectorAll('[role="alert"]').length, 0);
    assert.equal(FakeEventSource.instances.length, 2);
  } finally {
    await unmount();
  }
});

test("同じライブ画面とサイドバーは未読から空、非空への読み取り結果を反映する", async () => {
  const populated = createDashboardState();
  const empty = DashboardState.create({
    ...populated, goals: [], runs: [], pendingDecisions: [],
    servers: populated.servers.map((server) => ({ ...server, connected: true, lastReadAt: NOW })),
    projects: populated.projects.map((project) => ({ ...project, completedTasks24h: 0, connected: true, connectionState: ConnectionState.CONNECTION_STATE_CONNECTED, lastReadAt: NOW })),
  });
  const unread = DashboardState.create({
    ...empty, readAt: undefined,
    servers: empty.servers.map((server) => ({ ...server, connected: false, lastReadAt: undefined })),
    projects: empty.projects.map((project) => ({
      ...project, connected: false, lastReadAt: undefined,
      connectionState: ConnectionState.CONNECTION_STATE_UNSPECIFIED,
    })),
  });
  const children = [
    createElement("div", { id: "reading-overview", key: "overview" }, createElement(LiveOverview, { initialNowMs: NOW.getTime() })),
    createElement("div", { id: "reading-decisions", key: "decisions" }, createElement(LiveDecisions)),
    createElement("div", { id: "reading-goals", key: "goals" }, createElement(LiveGoals)),
    createElement("div", { id: "reading-sidebar", key: "sidebar" }, createElement(LiveSidebar, { frontendBuild: { commit: "abcdef0", branch: "test" } })),
  ];
  const { container, unmount } = await mountLive({ initialSnapshotLoaded: true, initialState: unread }, children);
  const snapshot = (): Element => {
    const copy = document.createElement("div");
    copy.innerHTML = container.innerHTML;
    return copy;
  };
  try {
    const initial = snapshot();
    const source = FakeEventSource.instances[0];
    assert.ok(source);
    await fireMessage(source, JSON.stringify(DashboardState.toJSON(empty)));
    const afterEmpty = snapshot();
    await fireMessage(source, JSON.stringify(DashboardState.toJSON(populated)));
    const afterPopulated = snapshot();
    assert.equal(FakeEventSource.instances.length, 1, "同じ購読とマウントで更新する");
    for (const id of ["reading-overview", "reading-decisions", "reading-goals"]) {
      const before = initial.querySelector(`#${id}`);
      const zero = afterEmpty.querySelector(`#${id}`);
      const filled = afterPopulated.querySelector(`#${id}`);
      assert.ok(before && zero && filled);
      assert.doesNotMatch(before.textContent ?? "", /(?:^|[^\d])0\s*件|0\s*\/|(?:run|ゴール|判断待ち)はありません/, id);
      assert.match(before.textContent ?? "", /未読|まだ.*読|不明|分かりません|確認中/, id);
      assert.match(zero.textContent ?? "", /0\s*件|ありません/, id);
      assert.doesNotMatch(zero.textContent ?? "", /あるかどうかは分かりません|まだ一度も読め/, id);
      assert.match(filled.textContent ?? "", /corporate-site/, id);
      if (id === "reading-overview") {
        assertUnreadRunList(before);
        assertRejectsUnreadEmptyClaim(before);
        assert.match(before.querySelector('[role="status"]')?.textContent ?? "", /応答がありません/);
        const emptyRunScopes = [zero.querySelector(".page-head"), zero.querySelector(".tile"), zero.querySelector('[aria-labelledby="h-runs"]')];
        const emptyRunText = emptyRunScopes.map((scope) => `${scope?.textContent} ${Array.from(scope?.querySelectorAll('[aria-label]') ?? []).map((item) => item.getAttribute('aria-label')).join(' ')}`).join(' ');
        assert.equal((emptyRunText.match(/(?:^|[^\d])0\s*件|0\s*\/|走っている run はありません/g) ?? []).length, 1, emptyRunText);
        assert.ok(filled.querySelector('a[href="/runs/run-aaa"]'));
        assert.match(filled.querySelector(".page-head")?.textContent ?? "", /1件の run/);
      } else if (id === "reading-decisions") {
        assert.equal(filled.querySelectorAll(".tray-item").length, 2);
        assert.match(filled.querySelector(".page-head")?.textContent ?? "", /2件/);
      } else {
        assert.equal(filled.querySelectorAll(".cards > li").length, 2);
        assert.match(filled.querySelector(".page-head")?.textContent ?? "", /2件/);
      }
    }
    const beforeServer = initial.querySelector("#reading-sidebar .srv-list li");
    const emptyServer = afterEmpty.querySelector("#reading-sidebar .srv-list li");
    const populatedServer = afterPopulated.querySelector("#reading-sidebar .srv-list li");
    assert.ok(beforeServer && emptyServer && populatedServer);
    assert.doesNotMatch(beforeServer.textContent ?? "", /run\s*0件/);
    assert.match(emptyServer.textContent ?? "", /run\s*0件/);
    assert.match(populatedServer.textContent ?? "", /run\s*1件/);
  } finally {
    await unmount();
  }
});

// 受信データは送信側と同じ DashboardState.toJSON の形で作る。
test("空の正常受信後にSSEだけが切断しても一覧に空を観測した時刻が残る", async () => {
  const observed = new Date("2026-11-13T12:00:00Z");
  const base = createDashboardState();
  const project = base.projects[0];
  assert.ok(project);
  const empty = DashboardState.create({ ...base, readAt: observed, runs: [], goals: [], pendingDecisions: [],
    servers: base.servers.filter((server) => server.serverId === project.serverId)
      .map((server) => ({ ...server, connected: true, lastReadAt: observed })),
    projects: [{ ...project, connected: true, connectionState: ConnectionState.CONNECTION_STATE_CONNECTED, lastReadAt: observed }],
  });
  const { container, unmount } = await mountLive({ initialState: DashboardState.create({}), initialSnapshotLoaded: false },
    createElement(LiveOverview, { initialNowMs: observed.getTime() }));
  const listText = (): string => {
    const list = container.querySelector('[aria-labelledby="h-runs"]');
    assert.ok(list);
    return list.textContent ?? "";
  };
  try {
    const source = FakeEventSource.instances[0];
    assert.ok(source);
    await fireMessage(source, JSON.stringify(DashboardState.toJSON(empty)));
    assert.match(listText(), /走っている run はありません。/);
    assert.doesNotMatch(listText(), /ありませんでした/);
    assert.equal(container.querySelectorAll('[role="alert"]').length, 0);
    await fireError(source);
    assert.equal(container.querySelectorAll('[role="alert"]').length, 1);
    assert.ok(listText().includes(formatClock(observed)), listText());
    assert.match(listText(), /走っている run はありませんでした。/);
    assert.equal(listText().match(/走っている run はありません/g)?.length, 1);
  } finally {
    await unmount();
  }
});

test("概要の未取得と取得済みの供給元0件は3画面で区別され、切断と再受信でも取得実績を保つ", async () => {
  const empty = DashboardState.create({});
  const children = [
    createElement("div", { id: "empty-overview", key: "overview" }, createElement(LiveOverview, { initialNowMs: NOW.getTime() })),
    createElement("div", { id: "empty-decisions", key: "decisions" }, createElement(LiveDecisions)),
    createElement("div", { id: "empty-goals", key: "goals" }, createElement(LiveGoals)),
  ];
  const { container, unmount } = await mountLive({
    initialState: empty, initialSnapshotLoaded: false,
    initialFailure: "unreachable", initialRespondedBefore: true,
  }, children);
  const lists = (): Element[] => {
    const overview = container.querySelector('#empty-overview [aria-labelledby="h-runs"]');
    const decisions = container.querySelector("#empty-decisions main");
    const goals = container.querySelector("#empty-goals main");
    assert.ok(overview && decisions && goals);
    return [overview, decisions, goals];
  };
  const assertEmpty = (): void => {
    for (const list of lists()) {
      assert.equal((list.textContent ?? "").match(/ありません/g)?.length, 1);
      assert.doesNotMatch(list.textContent ?? "", /分かりません|まだ読めていない/);
    }
    assert.doesNotMatch(lists()[0]?.textContent ?? "", /時点|ありませんでした/);
    for (const selector of ["#empty-overview .page-head", "#empty-overview .tile"]) {
      const scope = container.querySelector(selector);
      assert.ok(scope);
      assert.doesNotMatch(scope.textContent ?? "", /まだ読めていない|不明|まだ読めていません/);
    }
  };
  try {
    for (const list of lists()) {
      assert.match(list.textContent ?? "", /分かりません/);
      assert.doesNotMatch(list.textContent ?? "", /(?:run|ゴール|判断待ち)\s*はありません/);
    }
    const source = FakeEventSource.instances[0];
    assert.ok(source);
    assert.equal(container.querySelectorAll('[role="alert"]').length, 3);
    await fireMessage(source, JSON.stringify(DashboardState.toJSON(empty)));
    assertEmpty();
    assert.equal(container.querySelectorAll('[role="alert"]').length, 0);
    await fireError(source);
    assertEmpty();
    assert.equal(container.querySelectorAll('[role="alert"]').length, 3);
    assert.equal(source.closed, true);
    await waitForConnections(2);
    const reconnected = FakeEventSource.instances[1];
    assert.ok(reconnected);
    await fireMessage(reconnected, JSON.stringify(DashboardState.toJSON(empty)));
    assertEmpty();
    assert.equal(container.querySelectorAll('[role="alert"]').length, 0);
    await fireMessage(reconnected, JSON.stringify(DashboardState.toJSON(createDashboardState())));
    await fireError(reconnected);
    assert.ok(container.querySelector('#empty-overview a[href="/runs/run-aaa"]'));
    assert.equal(container.querySelectorAll("#empty-decisions .tray-item").length, 2);
    assert.ok(container.querySelector('#empty-goals a[href="/goals/goal-1"]'));
    assert.equal(container.querySelectorAll('[role="alert"]').length, 3);
    assert.equal(FakeEventSource.instances.length, 2);
  } finally {
    await unmount();
  }
});

const wireState = (mutate?: (wire: Record<string, unknown>) => void): string => {
  const wire = JSON.parse(JSON.stringify(DashboardState.toJSON(createDashboardState()))) as Record<
    string,
    unknown
  >;
  mutate?.(wire);
  return JSON.stringify(wire);
};

// 再接続は本番のスケジューラ（setTimeout）が初回1秒で走らせる。作られるまで待つ。
const waitForConnections = async (count: number): Promise<void> => {
  const deadline = Date.now() + 5_000;
  while (FakeEventSource.instances.length < count) {
    if (Date.now() > deadline) {
      throw new Error(`再接続が ${count} 件まで増えませんでした。`);
    }
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
};

test("マウントで /api/state へ接続し、SSE の受信が Context と画面へ反映される", async () => {
  const { holder, Probe } = createProbe();
  let revalidateCalls = 0;
  const { container, unmount } = await mountLive(
    { initialSnapshotLoaded: true,
      initialState: createDashboardState(),
      revalidate: () => {
        revalidateCalls += 1;
      },
    },
    [createElement(Probe, { key: "probe" }), createElement(LiveDecisions, { key: "decisions" })],
  );

  assert.equal(FakeEventSource.instances.length, 1, "マウントで接続が1つだけ開く");
  const source = FakeEventSource.instances[0]!;
  assert.equal(parseStreamUrl(source.url).path, "/api/state", "本番と同じ URL へ接続する");
  assert.equal(parseStreamUrl(source.url).ticket?.attempt, 1, "画面の識別子と最初の試行の番号を載せる");
  assert.equal(source.closed, false, "接続は開いたまま");
  assert.equal(holder.current?.disconnected, false, "初期値は接続中");
  assert.match(container.innerHTML, /corporate-site/, "初期状態の判断待ちが画面に出る");

  await fireMessage(
    source,
    wireState((wire) => {
      wire.pendingDecisions = [];
    }),
  );
  assert.equal(holder.current?.state.pendingDecisions.length, 0, "受信で Context の state が置き換わる");
  assert.equal(holder.current?.disconnected, false, "受信後も接続中");
  assert.equal(revalidateCalls, 1, "受信で revalidate が1回呼ばれる");
  assert.doesNotMatch(container.innerHTML, /corporate-site/, "置き換わった状態が画面へ反映される");

  await unmount();
  assert.equal(source.closed, true, "アンマウントで接続が閉じられる");
});

test("SSE の切断が画面の通知へ反映され、再接続後の受信で通知が消える", async () => {
  const { holder, Probe } = createProbe();
  const { container, unmount } = await mountLive({ initialSnapshotLoaded: true, initialState: createDashboardState() }, [
    createElement(Probe, { key: "probe" }),
    createElement(LiveDecisions, { key: "decisions" }),
  ]);
  const source = FakeEventSource.instances[0]!;
  assert.equal(countOccurrences(container.innerHTML, BACKEND_UNREACHABLE_MESSAGE), 0, "接続中は通知が出ない");

  await fireError(source);
  assert.equal(holder.current?.disconnected, true, "切断で Context の切断フラグが立つ");
  assert.equal(
    countOccurrences(container.innerHTML, BACKEND_UNREACHABLE_MESSAGE),
    1,
    "切断が画面の通知へ反映される",
  );
  assert.equal(source.closed, true, "切断した接続は閉じられる");

  await waitForConnections(2);
  const reconnected = FakeEventSource.instances[1]!;
  assert.equal(parseStreamUrl(reconnected.url).path, "/api/state", "再接続も同じ URL へ向かう");
  assert.equal(
    parseStreamUrl(reconnected.url).ticket?.screen,
    parseStreamUrl(source.url).ticket?.screen,
    "再接続も同じ画面の識別子",
  );
  assert.equal(parseStreamUrl(reconnected.url).ticket?.attempt, 2, "再接続は次の番号");
  await fireMessage(reconnected, wireState());
  assert.equal(holder.current?.disconnected, false, "再接続後の受信で切断フラグが消える");
  assert.equal(countOccurrences(container.innerHTML, BACKEND_UNREACHABLE_MESSAGE), 0, "通知が画面から消える");
  assert.match(container.innerHTML, /corporate-site/, "受信した状態が画面へ反映される");

  await unmount();
  assert.equal(reconnected.closed, true, "アンマウントで再接続した接続も閉じられる");
});

test("router.refresh 相当の再描画でも購読は維持され、状態は初期値へ戻らない", async () => {
  const { holder, Probe } = createProbe();
  let revalidateCalls = 0;
  const revalidate = (): void => {
    revalidateCalls += 1;
  };
  const children = [createElement(Probe, { key: "probe" })];
  const { root, unmount } = await mountLive(
    { initialSnapshotLoaded: true, initialState: createDashboardState(), revalidate },
    children,
  );
  const source = FakeEventSource.instances[0]!;

  await fireMessage(
    source,
    wireState((wire) => {
      wire.pendingDecisions = [];
    }),
  );
  assert.equal(holder.current?.state.pendingDecisions.length, 0, "受信した状態が Context にある");

  // router.refresh は RSC を再取得し、Provider が新しい initialState で再描画される。
  // revalidate の同一性は変わらないため、effect は再実行されない。
  await renderProvider(root, { initialSnapshotLoaded: true, initialState: createDashboardState(), revalidate }, children);
  assert.equal(FakeEventSource.instances.length, 1, "再描画で接続は開き直されない");
  assert.equal(source.closed, false, "購読は維持される");
  assert.equal(holder.current?.state.pendingDecisions.length, 0, "状態は新しい初期値へ戻らない");

  await fireMessage(source, wireState());
  assert.equal(holder.current?.state.pendingDecisions.length, 2, "再描画後も同じ購読で受信が届く");
  assert.equal(revalidateCalls, 2, "再描画後も revalidate が呼ばれる");

  await unmount();
});

test("revalidate の同一性が変わると、古い接続を閉じて1つだけ開き直す", async () => {
  const { holder, Probe } = createProbe();
  let callsA = 0;
  let callsB = 0;
  const revalidateA = (): void => {
    callsA += 1;
  };
  const revalidateB = (): void => {
    callsB += 1;
  };
  const children = [createElement(Probe, { key: "probe" })];
  const { root, unmount } = await mountLive(
    { initialSnapshotLoaded: true, initialState: createDashboardState(), revalidate: revalidateA },
    children,
  );
  const first = FakeEventSource.instances[0]!;
  await fireMessage(first, wireState());
  assert.equal(callsA, 1, "最初の revalidate が呼ばれる");

  await renderProvider(root, { initialSnapshotLoaded: true, initialState: createDashboardState(), revalidate: revalidateB }, children);
  assert.equal(FakeEventSource.instances.length, 2, "effect の再実行で接続が1つ開き直される");
  assert.equal(first.closed, true, "古い接続は閉じられる");
  const second = FakeEventSource.instances[1]!;
  assert.equal(second.closed, false, "新しい接続だけが開いている");

  await fireMessage(
    second,
    wireState((wire) => {
      wire.pendingDecisions = [];
    }),
  );
  assert.equal(callsB, 1, "開き直した後は新しい revalidate が呼ばれる");
  assert.equal(callsA, 1, "古い revalidate はもう呼ばれない");
  assert.equal(holder.current?.state.pendingDecisions.length, 0, "新しい接続の受信が Context へ反映される");

  await unmount();
  assert.equal(second.closed, true, "アンマウントで新しい接続も閉じられる");
});

// ===== 復帰時の鮮度確認の結線（issue #383） =====
//
// OS のスリープや裏のタブではタイマーが遅れる。表へ戻った（visibilitychange が visible）、
// フォーカスが戻った、ネットワークが戻った、の各イベントで openStateStream の checkStale を
// 呼び、最後の受信から壁時計で 40 秒以上経っていれば、利用者が操作する前に切断として扱う。

const setVisibility = (state: DocumentVisibilityState): void => {
  Object.defineProperty(document, "visibilityState", { value: state, configurable: true });
};

const wakeEvents: Array<[string, () => void]> = [
  ["visibilitychange（visible）", () => {
    setVisibility("visible");
    document.dispatchEvent(new window.Event("visibilitychange"));
  }],
  ["focus", () => window.dispatchEvent(new window.Event("focus"))],
  ["online", () => window.dispatchEvent(new window.Event("online"))],
];

// Date.now の差し替えはテストごとに 1 回だけ登録し、読む時刻を pinned で切り替える。
// pinned が undefined の間は実時間を返し、waitForConnections の期限も実時間で進む。
const pinWallClock = (t: { mock: { method: typeof import("node:test").mock.method } }) => {
  const realNow = Date.now.bind(Date);
  const clock: { pinned?: number; realNow: () => number } = { realNow };
  t.mock.method(Date, "now", () => clock.pinned ?? realNow());
  return clock;
};

for (const [name, fire] of wakeEvents) {
  test(`復帰の ${name} で、40 秒以上無音の接続を切断として扱い、前の内容を保ったまま開き直す`, async (t) => {
    const { holder, Probe } = createProbe();
    const clock = pinWallClock(t);
    const { unmount } = await mountLive(
      { initialSnapshotLoaded: true, initialState: createDashboardState() },
      [createElement(Probe, { key: "probe" })],
    );
    try {
      const first = FakeEventSource.instances[0]!;
      // 受信の時刻を固定し、その時刻から 39 秒後と 40 秒後で復帰イベントを判定させる。
      const receivedAt = clock.realNow();
      clock.pinned = receivedAt;
      await fireMessage(first, wireState());
      assert.equal(holder.current?.disconnected, false);
      const pending = holder.current?.state.pendingDecisions.length;

      clock.pinned = receivedAt + 39_999;
      await act(async () => fire());
      assert.equal(holder.current?.disconnected, false, "期限前の復帰では切らない");
      assert.equal(first.closed, false);

      clock.pinned = receivedAt + 40_000;
      await act(async () => fire());
      assert.equal(holder.current?.disconnected, true, "期限を過ぎた復帰で切断として扱う");
      assert.equal(first.closed, true, "半開きの接続を閉じる");
      assert.equal(holder.current?.snapshotLoaded, true, "読めた内容は残す");
      assert.equal(holder.current?.state.pendingDecisions.length, pending, "判断待ちの内容も残す");

      clock.pinned = undefined;
      await waitForConnections(2);
      await fireMessage(FakeEventSource.instances[1]!, wireState());
      assert.equal(holder.current?.disconnected, false, "開き直した接続の受信で戻る");
      assert.equal(FakeEventSource.instances.length, 2, "開き直しは 1 本だけ");
    } finally {
      await unmount();
    }
  });
}

test("裏へ回った visibilitychange（hidden）では鮮度を確かめない", async (t) => {
  const { holder, Probe } = createProbe();
  const clock = pinWallClock(t);
  const { unmount } = await mountLive(
    { initialSnapshotLoaded: true, initialState: createDashboardState() },
    [createElement(Probe, { key: "probe" })],
  );
  try {
    const receivedAt = clock.realNow();
    clock.pinned = receivedAt;
    await fireMessage(FakeEventSource.instances[0]!, wireState());
    clock.pinned = receivedAt + 60_000;
    setVisibility("hidden");
    await act(async () => {
      document.dispatchEvent(new window.Event("visibilitychange"));
    });
    assert.equal(holder.current?.disconnected, false);
    assert.equal(FakeEventSource.instances[0]!.closed, false);
  } finally {
    setVisibility("visible");
    await unmount();
  }
});

test("アンマウントで復帰イベントの購読を外す", async (t) => {
  const added = new Map<string, unknown>();
  const removed = new Map<string, unknown>();
  installDomEnvironment();
  for (const target of [window, document] as EventTarget[]) {
    const add = target.addEventListener.bind(target);
    const remove = target.removeEventListener.bind(target);
    t.mock.method(target, "addEventListener", (type: string, listener: EventListener, opts?: unknown) => {
      added.set(type, listener);
      add(type, listener, opts as AddEventListenerOptions);
    });
    t.mock.method(target, "removeEventListener", (type: string, listener: EventListener, opts?: unknown) => {
      removed.set(type, listener);
      remove(type, listener, opts as EventListenerOptions);
    });
  }
  const { Probe } = createProbe();
  const { unmount } = await mountLive(
    { initialSnapshotLoaded: true, initialState: createDashboardState() },
    [createElement(Probe, { key: "probe" })],
  );
  await unmount();
  for (const type of ["visibilitychange", "focus", "online"]) {
    assert.ok(added.has(type), `${type} を購読する`);
    assert.equal(removed.get(type), added.get(type), `${type} の購読を同じ関数で外す`);
  }
});

test("StrictMode の二重の mount でも、開いたままの接続は 1 本だけ残る（issue #385）", async () => {
  installDomEnvironment();
  FakeEventSource.instances = [];
  (globalThis as unknown as { EventSource: typeof FakeEventSource }).EventSource = FakeEventSource;
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  const providerProps: ComponentProps<typeof StateStreamProvider> = {
    initialSnapshotLoaded: true,
    initialState: createDashboardState(),
    children: createElement(LiveDecisions),
  };
  await act(async () => {
    root.render(createElement(StrictMode, null, createElement(StateStreamProvider, providerProps)));
  });
  assert.equal(FakeEventSource.instances.length, 2, "StrictMode は effect を 2 回走らせ、接続を 2 回開く");
  const open = FakeEventSource.instances.filter((source) => !source.closed);
  assert.equal(open.length, 1, "先の mount の接続は close で閉じる");
  const screens = new Set(FakeEventSource.instances.map((source) => parseStreamUrl(source.url).ticket?.screen));
  assert.equal(screens.size, FakeEventSource.instances.length, "mount ごとに別の識別子なので、互いに閉じ合わない");
  await act(async () => {
    root.unmount();
  });
  container.remove();
  assert.equal(FakeEventSource.instances.filter((source) => !source.closed).length, 0);
});

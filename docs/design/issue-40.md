# issue #40 の設計：画面の情報をリフレッシュレート（1秒・2秒・5秒・10秒、既定1秒）で要素単位に自動更新する

対象の issue：[j5ik2o/takt-dash#40](https://github.com/j5ik2o/takt-dash/issues/40)
関連：[j5ik2o/takt-dash#130](https://github.com/j5ik2o/takt-dash/issues/130)（毎秒の全ページ再取得の負荷）、[j5ik2o/takt-dash#383](https://github.com/j5ik2o/takt-dash/issues/383)（SSE の無音の見張り）、[j5ik2o/takt-dash#385](https://github.com/j5ik2o/takt-dash/issues/385)（見捨てられた SSE の後始末）
大きさの基準：[j5ik2o/takt-dash#361](https://github.com/j5ik2o/takt-dash/pull/361) = 1、[j5ik2o/takt-dash#362](https://github.com/j5ik2o/takt-dash/pull/362) = 5

確認したコミット：`110e25f`（この作業ツリーのブランチ `j5ik2o/issue-40-design` の基点）。

## 先頭のまとめ（公開 API・保存データ・秘密・守りの仕組み）

| 項目 | 触れるか | 内容 |
| --- | --- | --- |
| 公開 API（proto） | **はい** | `WatchStateRequest` に省略可能な `refresh_interval_ms` を足す。0 は「指定なし」で、今のクライアントはそのまま動く。 |
| 公開 API（HTTP） | **はい** | `/api/state` に省略可能な `refresh_ms` を足す。BFF に `/api/activity` と `/api/runs/[slug]` を足す。いずれも画面専用で、既存の呼び出しは変えない。 |
| 保存されるデータ | **いいえ** | DB・ファイル・proto の保存データは変えない。画面の選択だけをブラウザの `localStorage` に保存する。 |
| 秘密 | **いいえ** | 秘密・認証・接続先を増やさない。`refresh_ms` は秘密ではない。 |
| 守りの仕組み | **いいえ** | CI・関門・ルールセットの定義は変えない。テストの追加・更新だけで、`make done` を通す。 |

## 目的

- 画面にリフレッシュレート（1秒・2秒・5秒・10秒、既定1秒）の選択を置き、再読み込み後も選択を保つ。
- すべてのページの表示中の情報を、選んだレートで自動更新する。ページ全体を再読み込みせず、情報を持つコンポーネントや要素の単位で差し替える。
- 1秒を選んだときに、takt 側の状態の変化がおおむね1秒程度で画面へ届くよう、backend の読み取りと配信の間隔も選んだレートに追従させる。

## いまの状態（`110e25f` で確かめた）

### できていること

- 概要・判断待ち・ゴール一覧・サイドバーは、`StateStreamProvider` が受ける WatchState の SSE の状態をそのまま描画に使っている。要素単位の更新になっている。
  - `frontend/web/src/components/live-overview.tsx:16-24`
  - `frontend/web/src/components/live-decisions.tsx:8-10`
  - `frontend/web/src/components/live-goals.tsx:14-16`
  - `frontend/web/src/components/live-sidebar.tsx:17-19`
- タスク詳細・ゴール詳細・会話・run 詳細のログは、ページを保ったまま一定間隔で BFF の Route Handler を読み、値だけを差し替えている。間隔は 1 秒で固定されている。
  - `frontend/web/src/components/live-task-detail.tsx:37-38`
  - `frontend/web/src/components/live-goal-detail.tsx:28-29`
  - `frontend/web/src/components/live-conversation.tsx:75-78`
  - `frontend/web/src/components/live-run-log.tsx:55-57`
- SSE の再接続・無音の見張り・見捨てられた接続の後始末は、#383 と #385 で入っている。この設計では変えない。

### 一部だけできていること（ページ単位の再取得になっている）

- プロジェクト・アクティビティ・run 詳細は、SSE の状態を受けるたびに `router.refresh()` を呼び、RSC のページ全体を取り直している。要素単位ではなく、レートも選べない。
  - `frontend/web/src/components/state-stream.ts:71-82`（`createStateHandlers` が状態の受信ごとに `revalidate` を呼ぶ）
  - `frontend/web/src/components/state-provider.tsx:132-164`（SSE の `onState` に `revalidate` を渡す）
  - `frontend/web/src/components/state-provider.tsx:188-198`（`router.refresh()` を `revalidate` にする）
  - `frontend/web/src/components/projects-loader.tsx:12-20`（RSC で `GetOverview` の状態を読む）
  - `frontend/web/src/components/activity-loader.tsx:18-43`（RSC で `GetActivity` を読む）
  - `frontend/web/src/components/run-detail-loader.tsx:29-64`（RSC で `GetRunDetail` を読む）
- そのため、状態が変わったときだけでなく、backend の 15 秒の再送でもページ全体の RSC 再取得が走る。#130 の「毎秒全ページ再取得」の一部は、この `router.refresh()` が作っている。
- 更新のきっかけは backend の変化通知か 15 秒の再送で、画面の選択したレートではない。

### できていないこと

- リフレッシュレートの選択と、選択の保存がない。
- アクティビティと run 詳細に、要素単位のクライアント側の取得経路がない。
- プロジェクトは SSE の状態を使わず、RSC の結果と `router.refresh()` に頼っている。
- backend の読み取り間隔は設定の 30 秒（`backend/src/config.rs:23`、`backend/src/poller.rs:196-200`）で、画面の選択と結び付いていない。 WatchState の再送は 15 秒（`backend/src/grpc/mod.rs:77-80`）。
- 1 秒固定のポーリングが複数あり、選んだレートにできない。

## 受け入れ条件ごとの対応

| # | 受け入れ条件 | 対応 |
| --- | --- | --- |
| 1 | 1秒・2秒・5秒・10秒から選べ、既定が1秒 | サイドバーに「更新間隔」の `select` を置く。既定は 1000ms。`localStorage` の `taktdash.refreshRateMs` に保存し、再読み込み後も復元する。 |
| 2 | どのページでも、クリックや再読み込みなしで表示中の情報が選んだレートで更新される | 概要・判断待ち・ゴール・サイドバー・プロジェクトは SSE の状態、アクティビティ・run 詳細・タスク詳細・ゴール詳細・会話・run ログは各 BFF のポーリングにする。すべて `refreshRateMs` を使う。 |
| 3 | ページ全体が再読み込みされない（スクロール位置・入力途中が失われない） | SSE の受信ごとの `router.refresh()` をやめる。ポーリングは表示コンポーネントの state だけを差し替える。e2e で同じ document・入力値・スクロール位置を確かめる。 |
| 4 | 1秒で takt の変化がおおむね1秒で現れる | backend に「開いている画面が求める間隔の最小値」を持たせ、poller の読み取り間隔と WatchState の再送間隔をその値にする。画面がないときは設定の 30 秒に戻す。 |
| 5 | `make done` が通る | proto の生成物を更新し、単体・PBT・e2e を足す。`proto-check`、`fmt`、`clippy`、`test`、`typecheck`、`lint`、`coverage` を通す。 |

## 方針（決めたこと）

1. **レートは画面ごと（ブラウザごと）の設定にする。** 4 つの値だけを認め、既定は 1 秒。保存先は `localStorage`。サーバーには保存しない。
2. **SSE の状態を要素単位の更新の主経路にする。** 概要・判断待ち・ゴール・サイドバー・プロジェクトは `StateStreamProvider` の状態から描画する。`router.refresh()` は状態更新の経路から外す。
3. **アクティビティと run 詳細は、RSC の初回表示＋BFF のポーリングにする。** 初回は今と同じくサーバーで読んで内容を出し、開いている間は `/api/activity` と `/api/runs/[slug]` を選んだレートで読む。
4. **backend の読み取りと配信を、開いている画面のレートに合わせる。** `WatchStateRequest.refresh_interval_ms` を受け取り、接続中の要求の最小値を poller の読み取り間隔にする。WatchState は接続ごとに、そのレートを超えない頻度で状態を送る（変化は合流させ、最後の状態を送る）。
5. **画面がないときは今の設定に戻す。** 既定 30 秒の `intervalSeconds` は残す。最後の画面が切れたら、poller は設定の間隔へ戻る。
6. **古いクライアントは今のまま動かす。** `refresh_interval_ms` が 0 または未指定なら、読み取りは設定の間隔、WatchState の再送は 15 秒のまま。

## 変える場所

### proto

- `proto/taktdash.proto` の `WatchStateRequest` に `uint32 refresh_interval_ms = 1;` を足す。0 は指定なし。1000〜60000 の範囲だけを受け付け、それ以外は `INVALID_ARGUMENT` にする（画面は 1000・2000・5000・10000 しか送らない）。
- `make proto-gen` で `frontend/web/src/proto/taktdash.ts` を更新し、`make proto-check` が通ることを確かめる。

### frontend：レートの選択と保存

- 新規 `frontend/web/src/state/refresh-rate.ts`
  - `REFRESH_RATE_OPTIONS = [1000, 2000, 5000, 10000] as const`
  - `DEFAULT_REFRESH_RATE_MS = 1000`
  - `parseRefreshRateMs(value: unknown): number`：文字列・数値・`null`・`undefined`・壊れた値をすべて受け、4 つのどれかならその値、それ以外は既定の 1000 を返す。例外を投げない。
  - `parseRefreshIntervalMs(raw: string | null): number | undefined`：`/api/state` の `refresh_ms` 用。`null`・空・0・1000〜60000 の整数だけを通し、壊れた値は `undefined`（指定なし）にする。
  - `refreshRateLabel(ms): string`：`1秒`・`2秒`・`5秒`・`10秒`。
- 新規 `frontend/web/src/components/refresh-rate-provider.tsx`
  - `RefreshRateProvider` と `useRefreshRate()`。値は `refreshRateMs` と `setRefreshRateMs`。
  - 初期値は 1000（SSR とハイドレーションを一致させるため、描画中は `localStorage` を読まない）。`useEffect` で `localStorage.getItem("taktdash.refreshRateMs")` を読み、`parseRefreshRateMs` で検証してから設定する。
  - `setRefreshRateMs` は state を更新し、`localStorage.setItem` を試す。`SecurityError` や容量不足でも state は更新し、画面は動き続ける。
  - `storage` イベントを聴き、別のタブの変更を反映する（同じブラウザの別タブでレートが食い違わないようにする）。
- 新規 `frontend/web/src/components/refresh-rate-select.tsx`（または `Sidebar` の一部）
  - `<label for="refresh-rate">更新間隔</label>` と `<select id="refresh-rate">`。選択肢は `1秒`・`2秒`・`5秒`・`10秒`。既定は `1秒`。
  - 変更時に `setRefreshRateMs` を呼ぶ。現在の値には `aria-current` ではなく `selected` を使う。
- `frontend/web/app/layout.tsx`
  - `RefreshRateProvider` で `StateStreamProvider` を包む。`Sidebar` にレートの選択を渡す。
- `frontend/web/src/components/sidebar.tsx`
  - `refreshRate?: { value: number; onChange: (ms: number) => void }` を受け取り、「ページ」の下に「更新間隔」の節を出す。渡されない既存のテストは今のまま動く（任意 prop）。
- `frontend/web/src/components/live-sidebar.tsx`
  - `useRefreshRate()` の値を `Sidebar` に渡す。

### frontend：SSE とページ単位の再取得の廃止

- `frontend/web/src/components/state-provider.tsx`
  - `useRefreshRate()` を読み、`openStateStream` の URL を `/api/state?refresh_ms=${refreshRateMs}` にする。既存の `withStreamTicket` が `&screen=…&attempt=…` を足す。
  - `revalidate` prop と `useRouter` を消す。`StateStreamProviderWithRefresh` は廃止し、`layout.tsx` は `StateStreamProvider` を直接使う。
  - レートが変わると `useEffect` の依存が変わり、古い EventSource を閉じて新しい URL で 1 本だけ開き直す。状態は Context に残るので、画面は消えない。
- `frontend/web/src/components/state-stream.ts`
  - `createStateHandlers` から `revalidate` を消す。`onState` は状態を適用するだけにする。
  - 再接続・見張り・`checkStale`・試行の URL は変えない。
- `frontend/web/src/grpc/client.ts`
  - `watchState(refreshIntervalMs?: number)` にし、`WatchStateRequest` の `refreshIntervalMs` を渡す。省略時は 0。
- `frontend/web/app/api/state/route.ts`
  - `refresh_ms` を読み、`parseRefreshIntervalMs` で 0 または 1000〜60000 の整数だけを通す。壊れた値は「指定なし」として扱い、SSE を 400 で止めない（EventSource が再接続しなくなるため）。
  - `openStream: () => getBackendClient().watchState(refreshMs)` にする。`screen`・`attempt` の扱いは変えない。

### frontend：プロジェクトを SSE の状態にする

- 新規 `frontend/web/src/components/live-projects.tsx`
  - `useLiveState()` の `state` を `ProjectsContent` に渡す。`now` は `state.readAt ?? new Date()` を基準に `refreshRateMs` で進める（待ち時間の表示のため。取得はしない）。`StateStreamProvider` の内部型は proto の `DashboardState` の部分型なので、`ProjectsContent` の prop の型は変えずに渡せる。
  - `disconnected` のときは既存の `BackendUnavailableNotice` を出し、前に読めた状態は残す（#309 と同じ）。`snapshotLoaded` が false で状態がないときは内容を出さない。
- `frontend/web/app/projects/page.tsx`
  - `state` prop がないときは `ProjectsLoader` ではなく `LiveProjects` を返す。`state` prop があるテスト用の経路は残す。
- `frontend/web/src/components/projects-loader.tsx`
  - 使わなくなるので削除する（または `LiveProjects` を返す薄いラッパーにする）。削除する場合、直接呼んでいるテストを `LiveProjects` のテストへ置き換える。

### frontend：アクティビティをポーリングする

- 新規 `frontend/web/app/api/activity/route.ts`
  - `getBackendClient().getActivity()` を呼び、`GetActivityResponse.toJSON` を `Cache-Control: no-store` で返す。失敗は既存の `toBackendUnavailableResponse` に合わせる。
  - 読めた応答を `lastActivity` に覚える（#309 のキャッシュ）。`lastActivity` は `activity-loader.tsx` の module 変数から `src/state/last-loaded.ts` へ移し、Route Handler と Loader で共有する。
- 新規 `frontend/web/src/components/live-activity.tsx`
  - 初回の `response` と `readAt` を受け取り、`useLiveState()` の `state` と合わせて `ActivityContent` を描く。
  - `startPolling` で `/api/activity` を `refreshRateMs` 間隔で読む。成功したら `response` と `now` を差し替え、失敗したら直前の内容を残して `banner warn` を出す。404 は無い。
  - `ActivityContent` は proto の `GetActivityResponse` を受けるが、内部型は proto 型の部分型なので、復元した値はそのまま渡せる。
- `frontend/web/src/components/activity-loader.tsx`
  - 今の RSC の取得は残し、成功・キャッシュの結果を `LiveActivity` の初期値として渡す。初回の内容を SSR で出すため。
- `frontend/web/app/activity/page.tsx`
  - `response` prop がないときは `ActivityLoader`（初回 RSC）を返し、その中で `LiveActivity` を描く。
- `frontend/web/src/state/restore-detail.ts`（または `restore-state.ts`）
  - `restoreActivityResponse(payload)` を足す。`events`・`dailySummaries`・`todayCountsByServer` の時刻と enum を復元する。`state` は `useLiveState` の状態を使うので、応答の `state` は省略してよい。

### frontend：run 詳細をポーリングする

- 新規 `frontend/web/app/api/runs/[slug]/route.ts`
  - `getBackendClient().getRunDetail(slug)` を呼び、`GetRunDetailResponse.toJSON` を `no-store` で返す。`RunNotFoundError` は 404、`BackendUnavailableError` は既存の 503 に写す。
  - 読めた応答を `lastRunDetails` に覚える。`lastRunDetails` は `run-detail-loader.tsx` の module 変数から `src/state/last-loaded.ts` へ移す。
- 新規 `frontend/web/src/components/live-run-detail.tsx`
  - 初回の `detail` と `serverName` を受け取り、`startPolling` で `/api/runs/${slug}` を `refreshRateMs` 間隔で読む。成功したら `detail` を差し替え、失敗したら直前の内容を残して `banner warn` を出す。404 は「見つかりません」を出してポーリングを止める。
  - `serverName` は `useLiveState()` の `state.servers` から `serverId` で引き直し、見つからなければ初回の値を使う。
  - `RunDetailContent` に新しい `detail` を渡す。`LiveRunLog` は今と同じく `key={detail.runSlug}` で、ログの追記分は自分で取る。
- `frontend/web/src/components/run-detail-loader.tsx`
  - 今の RSC の取得は残し、`LiveRunDetail` に初期値を渡す。404 の `notFound()` も残す。
- `frontend/web/src/state/restore-detail.ts`
  - `restoreRunDetail(payload)` を足す。文字列・時刻・`RunStatus`・`StepLog`・`RunReport`・`RunInstruction` を復元する。ブラウザで `@grpc/grpc-js` を値として読まないよう、生成コードの `fromJSON` は使わない。

### frontend：既存のポーリングをレートに合わせる

- `frontend/web/src/components/polling.ts`
  - `intervalMs` を「前回の取得の完了から次まで」ではなく「前回の開始から次まで」の周期にする。取得が `intervalMs` より長引いたら、完了後に間を置かず次を始める（重ねない）。これで 1 秒のときに取得時間の分だけ周期が伸びるのを防ぐ。
  - `stop` / abort 後は取得も適用もしない、という今の契約は変えない。
- `frontend/web/src/components/live-task-detail.tsx`
- `frontend/web/src/components/live-goal-detail.tsx`
- `frontend/web/src/components/live-conversation.tsx`
- `frontend/web/src/components/live-run-log.tsx`
  - `useRefreshRate()` を読み、`intervalMs` に `refreshRateMs` を渡す。レートが変わったら effect を張り直し、古いタイマーを止める。
  - 会話は `setInterval` を `startPolling` と同じ固定周期の扱いに直す（前回の `poll` が終わるまで次を始めない点は今のまま）。
- `frontend/web/src/components/live-overview.tsx` の 1 秒の時計は、取得ではなく経過時間の表示なので 1 秒のままにする。

### backend：読み取りと配信を画面のレートに合わせる

- 新規 `backend/src/refresh.rs`
  - `RefreshDemand`：接続中の要求（`u64` の番号）と希望間隔の `BTreeMap`、最小値を配る `tokio::sync::watch` を持つ。
  - `DemandGuard`：`acquire(Duration) -> DemandGuard` が返す。`Drop` で自分の番号を外し、最小値を再計算して通知する。二重解放しない。
  - `effective(config: Duration) -> Duration`：要求が無ければ `config`、あれば `min(config, 要求の最小値)`。下限は 1 秒。
  - `subscribe() -> watch::Receiver<Option<Duration>>`：poller が待つための受信。
  - `MIN_REFRESH_INTERVAL = 1s`、`MAX_REFRESH_INTERVAL = 60s`。
- `backend/src/lib.rs`
  - `mod refresh;` を足す。
- `backend/src/grpc/mod.rs`
  - `TaktDashServiceImpl` と `ServiceDeps` に `refresh_demand: RefreshDemand` を足す。
  - `watch_state`：`WatchStateRequest.refresh_interval_ms` を読む。0 なら要求なし、1000〜60000 なら `DemandGuard` を取る。それ以外は `INVALID_ARGUMENT`。`DemandGuard` は送信タスクへ移し、ストリームの終了（`sender.closed()`・停止・購読の終端）で落とす。
  - 再送と変化の送信を、接続ごとのレートに合わせる。初期状態は今すぐ送る。その後は `next_send = now + interval` まで待ち、変化通知は合流させ、期限に最後の状態を 1 回だけ組み立てて送る。`interval` は要求があればその値、無ければ今の `WATCH_RESEND_INTERVAL`（15 秒）。1 回の送信の後は必ず `next_send` を進めるので、同じ接続でレートより速く送らない。
  - 既存の `WATCH_CHANNEL_CAPACITY`・`WATCH_RESEND_INTERVAL`・停止の扱いは残す。
- `backend/src/poller.rs`
  - `Poller` に `configured_interval` と `refresh_demand` を足す。`with_refresh_demand` で共有する。テストは指定しなければ要求なしの `RefreshDemand` を持つ。
  - `read_cycle_once_at` で `configured_interval = config.interval_seconds` を更新し、`CycleReport.interval = refresh_demand.effective(configured_interval)` にする。
  - `effective_interval()` を足し、要求が変わったときに poller が次の待ち時間を計算し直せるようにする。
- `backend/src/poller_stop.rs`
  - `run_until_stopped` の待機に `refresh_demand.subscribe()` の変更を足す。待っている間に新しい画面が 1 秒を要求したら、残りの待ちを切り上げて、新しい間隔で次の周期を始める。要求が減ったら、新しい長い間隔で待ち直す。停止の合図と `CYCLE_GRACE` の扱いは変えない。
- `backend/src/main.rs`
  - `RefreshDemand::new()` を 1 つ作り、`ServiceDeps` と `Poller` の両方へ同じものを渡す。
- `backend/src/config.rs`
  - `DEFAULT_INTERVAL_SECONDS = 30` は変えない。画面がないときの間隔として残す。`intervalSeconds` の検証（0 は不正）も変えない。

### テスト

- 新規・更新する単体テスト（frontend）
  - `test/state/refresh-rate.test.ts`：4 つの値、既定、壊れた値、`localStorage` の往復。
  - `test/components/refresh-rate-provider.test.ts`：保存値の復元、変更の保存、`localStorage` が使えないときの継続、`storage` イベント。
  - `test/components/live-provider-wiring.test.ts`：レート変更で EventSource が 1 本だけになり、URL に `refresh_ms` が載る。`revalidate` のテストは削除・置換する。
  - `test/components/state-revalidation.test.ts`：状態を受けても RSC の再取得を促さない契約に置き換える。
  - `test/components/state-stream.test.ts`：`createStateHandlers` から `revalidate` が消えた契約に直す（再接続・見張りのテストは残す）。
  - `test/api/state-route.test.ts`：`refresh_ms` の有効・無効と、`watchState` へ渡る値。
  - `test/components/polling.test.ts`：固定周期、遅い取得、abort、レート変更。
  - `frontend/web/src/proto-contract.test.ts`：`WatchStateRequest.refreshIntervalMs` の型が生成物にあること。
  - `test/components/live-projects.test.ts`：SSE の状態だけで更新される。
  - `test/api/activity-route.test.ts`、`test/api/run-route.test.ts`：JSON、404、503、キャッシュ。
  - `test/state/restore-detail.test.ts`：`restoreRunDetail`・`restoreActivityResponse`。
- 新規・更新する単体テスト（backend）
  - `backend/tests/refresh_demand_test.rs`（または `backend/src/refresh_tests.rs`）：複数の要求の最小、解放、二重解放、0 件で設定に戻る、範囲外。
  - `backend/tests/watch_state_test.rs`：`refresh_interval_ms = 1000` で、状態が変わらなくても 1 秒程度で再送される。0 のときは 15 秒のまま（短い待ちで何も来ないことを確かめる既存テストを維持）。古い `WatchStateRequest {}` が今のまま動く。
  - `backend/tests/poller_test.rs`：要求があるときの `CycleReport.interval`、要求が消えたら設定に戻る、待機中に要求が増えたら周期を切り上げる。
- e2e
  - `e2e/tests/issue-40-refresh-rate.spec.ts`（新規）：既定 1 秒、4 つの選択、再読み込み後の復元。
  - 同：概要とプロジェクトで、状態ファイルの変更が再読み込みなしで 3 秒以内に現れる。同じ document の印を置いて確かめる。
  - 同：アクティビティ・run 詳細・タスク詳細・ゴール詳細・会話・run ログが、レートを 5 秒にすると 5 秒周期になる（要求の時刻を数える）。1 秒では 3 秒以内に現れる。
  - 同：判断待ちの入力欄に文字を入れ、状態更新の後も値とスクロール位置が残る。
  - 既存の `e2e/tests/flow-9-run-detail-live-log.spec.ts` は `router.refresh` の `rsc=1` を待っているので、run 詳細のポーリング（`/api/runs/[slug]`）を待つ形に直す。
  - `e2e/tests/issue-430-error-reload.spec.ts` の「状態ストリームの受信で router.refresh が走る」という前提のコメントを直す（ボタンの再取得の確認はそのまま）。
  - `e2e/support/stack.ts` に `intervalSeconds` の指定を足し、backend が 30 秒のままでも画面の 1 秒要求で 1 秒に切り替わることを確かめられるようにする（任意）。

## 状態と遷移

### 画面のレート

| 状態 | 値 | 遷移 |
| --- | --- | --- |
| 初期（SSR） | 1000ms | サーバーとハイドレーションを一致させるため、保存値を読まない |
| 保存値の復元 | 1000/2000/5000/10000 | `useEffect` で `localStorage` を読み、`parseRefreshRateMs` を通す。壊れていれば 1000 のまま |
| 利用者の選択 | 4 つのどれか | state を更新し、`localStorage` に書く。SSE を閉じて新しい `refresh_ms` で開き直す。ポーリングの effect を張り直す |
| 別タブの変更 | 4 つのどれか | `storage` イベントで同じ値へ移る |
| 保存できない | 選んだ値（メモリだけ） | `localStorage` の例外を握りつぶし、画面はその値で動く |

- 保存先は `localStorage` のキー `taktdash.refreshRateMs`。値は `"1000"` などの文字列。DB・ファイル・Cookie には保存しない。
- レートはブラウザごと。複数の端末で同じ値にしたい場合は、設定ファイルやサーバー側の保存が要る（この設計の範囲外）。

### backend の要求と読み取り

```
画面の選択 (localStorage)
  → /api/state?refresh_ms=1000
  → WatchStateRequest.refresh_interval_ms = 1000
  → RefreshDemand（接続中の要求の最小値）
  → Poller の読み取り間隔 = min(intervalSeconds, 要求の最小値)
  → 変化通知
  → WatchState が接続ごとのレートで状態を送る（変化は合流）
  → SSE
  → React の状態 / ポーリング
```

- 要求は `WatchState` の接続ごとに 1 つ。接続が切れたら `DemandGuard` が落ち、最小値を計算し直す。Next の Route Handler が gRPC を閉じれば、backend は `sender.closed()` で気づく。
- 画面がないときは要求が 0 件なので、poller は `intervalSeconds`（既定 30 秒）で読む。
- 複数の画面があるときは、いちばん速い要求に合わせる。10 秒の画面と 1 秒の画面が同時にあれば、読み取りは 1 秒。10 秒の画面への送信は 10 秒ごと（変化は合流）で、1 秒の画面への送信は 1 秒ごと。
- 設定の `intervalSeconds` が 1 秒など要求より速い場合は、設定のほうが速い（`min`）。

### 要素単位の更新

| 画面 | 初回 | 更新 |
| --- | --- | --- |
| `/` 概要 | layout の `GetOverview` | SSE の状態を `LiveOverview` が描画 |
| `/decisions` | layout の `GetOverview` | SSE の状態を `LiveDecisions` が描画 |
| `/projects` | layout の `GetOverview` | SSE の状態を `LiveProjects` が描画 |
| `/goals` | layout の `GetOverview` | SSE の状態を `LiveGoals` が描画 |
| サイドバー | layout の `GetOverview` | SSE の状態を `LiveSidebar` が描画 |
| `/activity` | RSC の `GetActivity` | `/api/activity` をレートで読む |
| `/runs/[slug]` | RSC の `GetRunDetail` | `/api/runs/[slug]` をレートで読む。ログは `/api/runs/[slug]/updates` をレートで読む |
| `/tasks/[project]` | RSC の `GetTaskDetail` | `/api/tasks/[project]` をレートで読む（既存） |
| `/goals/[goalId]` | RSC の `GetGoalDetail` | `/api/goals/[goalId]` をレートで読む（既存） |
| `/conversations` | RSC の `GetConversation` | `/api/conversations` をレートで読む（既存） |

- どの更新もページ全体の RSC 再取得をしない。表示コンポーネントの state だけを差し替える。
- 取得に失敗した間は直前の内容を残し、`banner warn` を出す。次の成功で消す（今の詳細画面と同じ）。
- 前回の取得が終わる前に次を重ねない。abort 後は取得も適用もしない。

## 守る不変条件

1. **レートは 4 つのどれか**：`refreshRateMs` は 1000・2000・5000・10000 のどれか。保存値が壊れていても既定の 1000 になる。
2. **既定は 1 秒、再読み込み後も保たれる**：保存値がなければ 1000。保存できたら次の読み込みで同じ値になる。保存できない環境でも画面はその値で動く。
3. **データ更新でページ全体を再取得しない**：SSE の受信で `router.refresh()` を呼ばない。ポーリングは対象の BFF だけを読む。
4. **接続ごとのレートを超えて送らない**：WatchState は初期状態の後、接続の `interval` の間に 1 回までしか送らない。変化が何度来ても、最後の状態を 1 回にまとめる。
5. **要求の最小値を読み取りに使う**：`Poller` の間隔は `min(intervalSeconds, 接続中の要求の最小値)`。要求が無ければ `intervalSeconds`。
6. **要求は接続の終了で必ず外れる**：`DemandGuard` は正常終了・エラー・停止・クライアント切断のどれでも 1 回だけ外れる。外れ忘れて 1 秒のまま残らない。
7. **1 秒のとき、変化がおおむね 1 秒で届く**：読み取り周期 1 秒＋状態の組み立て・送信・描画。LAN の実測で 1〜2 秒程度を目標にし、e2e は 3 秒以内で確かめる。
8. **取得は重ならない**：同じ画面の同じ経路で、前回の取得が終わる前に次を始めない。abort 後は取得も適用もしない。
9. **失敗しても内容を消さない**：取得に失敗したら直前の内容を残し、通知を出す。次の成功で戻す（#309 と同じ）。
10. **古いクライアントは今のまま**：`refresh_interval_ms = 0` のときは、読み取りは設定の間隔、再送は 15 秒。`/api/state` の引数なしも同じ。
11. **LAN・VPN の使い方を壊さない**：Next の待ち受け（`0.0.0.0:3000`）・gRPC の待ち受け・認証・接続元の制限を変えない。新しい経路は同じ origin の HTTP だけで、新しいポートを開けない。
12. **保存データを変えない**：DB の表・列・索引・保存形式を変えない。`localStorage` 以外に選択を保存しない。

## テストにできる性質（例・PBT・e2e）

| ID | 性質 | 確かめ方 | 置き場所 |
| --- | --- | --- | --- |
| E1 | 4 つの値だけを受け、それ以外は 1000 になる | 例 | `test/state/refresh-rate.test.ts` |
| E2 | 保存した値が再読み込みで戻る。保存できない環境でも動く | 例 | `test/components/refresh-rate-provider.test.ts` |
| E3 | `select` の選択肢が 4 つで、既定が 1 秒。変更が state と保存へ伝わる | 例 | 同 |
| E4 | レート変更で EventSource が 1 本だけになり、URL に `refresh_ms` が載る | 例 | `test/components/live-provider-wiring.test.ts` |
| E5 | 状態を受けても `router.refresh` を呼ばない | 例 | `test/components/state-revalidation.test.ts`（置換） |
| E6 | プロジェクトは SSE の状態だけで更新される | 例 | `test/components/live-projects.test.ts` |
| E7 | アクティビティは `/api/activity` だけをレートで読む | 例 | `test/components/live-activity.test.ts`、`test/api/activity-route.test.ts` |
| E8 | run 詳細は `/api/runs/[slug]` だけをレートで読む。ログは updates を読む | 例 | `test/components/live-run-detail.test.ts`、`test/api/run-route.test.ts` |
| E9 | 遅い取得でも重ならず、完了後にすぐ次を始める | 例 | `test/components/polling.test.ts` |
| E10 | 要求の最小値が読み取り間隔になる。解放で設定に戻る | 例 | `backend/tests/refresh_demand_test.rs` |
| E11 | 1 秒の要求で、状態が変わらなくても 1 秒程度で再送される | 例 | `backend/tests/watch_state_test.rs` |
| E12 | 0 の要求（古いクライアント）は今のまま | 例 | 同 |
| E13 | レートの選択が再読み込み後も残る | e2e | `e2e/tests/issue-40-refresh-rate.spec.ts` |
| E14 | 全ページで、状態の変更が再読み込みなしで現れる | e2e | 同、既存の flow |
| E15 | 同じ document のまま、入力値とスクロール位置が残る | e2e | 同 |
| E16 | 1 秒のとき、変更が 3 秒以内に現れる | e2e | 同 |
| E17 | 5 秒にすると、要求の間隔が 5 秒になる | e2e | 同 |
| P1 | `parseRefreshRateMs` はどんな値でも例外を投げず、4 つの値か 1000 を返す | PBT | `test/state/refresh-rate.test.ts` |
| P2 | 任意の acquire/release の列で、`effective` は「要求が無ければ config、あれば min(config, 最小要求)」。解放しても他の要求の値を壊さない | PBT | `backend/tests/refresh_demand_test.rs` |
| P3 | 任意の変化の到着と期限の列で、送信は interval より速くなく、変化を合流させて最後の状態を送る（途中の版を送らなくてよいが、最後の版は落とさない） | PBT | `backend/src/grpc` の純粋な送信スケジュール関数 |
| P4 | 任意の取得完了・abort・レート変更の列で、取得は重ならず、abort 後は 0 回 | PBT | `test/components/polling.test.ts` |
| P5 | 任意のベース URL に `screen`・`attempt` と `refresh_ms` を付けても、両方が保たれる | PBT | `test/support/stream-url.ts` の拡張 |

- PBT は既存の frontend と同じく、シード固定の PRNG で操作列を作る。除外は付けない。
- 送信スケジュール（P3）は、`now`・`last_sent`・`interval`・`dirty` を受けて「送る／いつまで待つか」を返す純粋関数に切り出し、`tokio::time` の実時間に頼らずに確かめる。実時間の 1 秒は E11 と e2e で確かめる。
- `make done` の coverage 80% を満たすため、新規の `refresh-rate.ts`・`RefreshDemand`・`LiveActivity`・`LiveRunDetail`・新しい Route Handler には対応するテストを置く。

## 壊しうる入力

| 入力・状況 | 期待する振る舞い |
| --- | --- |
| `localStorage` が無い・読めない・書き込めない（プライベートモード、容量不足） | 例外を握りつぶし、メモリの値で動く。既定は 1000 |
| 保存値が `""`・`"0"`・`"1001"`・`"1e3"`・`"NaN"`・`"１０秒"`・巨大な文字列・Unicode | 1000 に戻す。例外を投げない |
| `refresh_ms` が無い・空・負・小数・`999`・`60001`・`1e3`・繰り返し | BFF は「指定なし」として扱い、SSE を止めない。backend に直接不正値が来たら `INVALID_ARGUMENT` |
| レートを 1→10→1 と素早く変える | 古い EventSource を閉じ、新しい 1 本だけを開く。古い接続の遅延イベントは捨てる（#383 の契約）。ポーリングは古いタイマーを止めて張り直す |
| 同じブラウザの複数タブで違うレート | 各タブが自分のレートを持つ。backend は最小値で読み、各接続へ自分のレートで送る |
| 画面を裏に回す・スリープから戻る | 既存の `visibilitychange`・`focus`・`online` の `checkStale` はそのまま。ポーリングのタイマーが遅れても、次の取得で最新になる |
| 取得がレートより遅い | 前回の完了まで次を始めない。完了後に間を置かず次を始める（重ならない） |
| 取得が失敗する・backend が落ちる | 直前の内容を残し、通知を出す。次の成功で戻す |
| run が消える（404） | 直前の内容を残して通知を出し、その run のポーリングを止める。初回の 404 は今のとおりページの 404 |
| アクティビティの応答が壊れた JSON | 取得失敗として扱い、直前の内容を残す。例外でページを落とさない |
| 古い画面の JavaScript（`refresh_ms` を付けない） | 今のまま動く。読み取りは設定の間隔、再送は 15 秒 |
| `intervalSeconds` が 1 秒・30 秒・大きい値 | `min` を取るので、要求が速ければ要求、遅ければ設定。0 は今のとおり設定エラー |
| 接続が突然切れる（タブを閉じる、VPN が切れる） | backend は `sender.closed()` で気づき、`DemandGuard` を落とす。Next 側は既存の #385 の後始末で閉じる |
| `WatchState` の初期組み立てが失敗する | エラーを送って終了し、`DemandGuard` を落とす。1 秒の要求が残らない |

## 影響

- **上流（takt、takt-mcp）**：なし。takt の読み取り回数は、画面が開いている間だけ増える。画面がないときは今と同じ 30 秒。
- **保存済みデータ**：なし。SQLite の表・列・索引・保存形式を変えない。選択は `localStorage` だけ。
- **公開 API**：
  - proto は `WatchStateRequest` に省略可能なフィールドを足すだけ。既存のクライアントは 0 として扱われ、今のまま動く。
  - HTTP は `/api/state` の任意の引数と、`/api/activity`・`/api/runs/[slug]` の追加。既存の URL は変わらない。
  - CLI は変えない。
- **守りの仕組み**：CI・関門・ルールセットの定義は変えない。`make done` の対象に新しいテストが入る。`proto-check` のため生成物をコミットする。
- **LAN・VPN（`0.0.0.0:3000`）**：待ち受けアドレス・ポート・認証・接続元の制限を変えない。新しい経路は同じ origin の HTTP だけで、ブラウザから backend へ直接つながない（今の BFF 構成のまま）。
- **負荷**：
  - `router.refresh()` をやめることで、SSE の受信ごとのページ全体の RSC 再取得（#130）が無くなる。
  - 画面が開いている間は、poller が全プロジェクトを 1 秒で読む。プロジェクト数が多いと takt-mcp の負荷が上がる。画面がないときは 30 秒に戻る。
  - run 詳細の `GetRunDetail` を 1 秒で読むと、ローカルのログ（最大 1 MiB の末尾）を繰り返し読む。負荷が高ければ、WatchState の run の要約だけを先に更新し、詳細は変化があったときだけ読む形に変える（下の「決まっていない点」）。

## Quint（`specs/quint/`）は要るか

**要らない。**

- 変えるのは、4 つの値の選択と保存、既存の `startPolling` の周期、backend の「要求の最小値」と「接続ごとの送信間隔」だけ。
- 要求の登録・解放は `BTreeMap` と `watch` の最小値計算で、状態遷移は「登録」「解放」「最小値の再計算」の 3 つ。既存の Quint のような再試行・並行・資源の二重解放は無い（`DemandGuard` の二重解放は Rust の `Drop` と番号の確認で防ぐ）。
- 時間に依存する「レートより速く送らない」「変化を合流させる」は、Quint の時間の表現より、純粋関数の PBT（P3）と `tokio::time` の境界テストのほうがよく捕まえる。
- 既存の `state-stream-relay`（#385）と `state-stream-watchdog`（#383）の不変条件は変えない。再送間隔が接続ごとに 1〜15 秒になっても、40 秒の見張りより短いので、見張りの契約はそのまま成り立つ。

## 大きさ

**8**（1 つの PR で出す）。

- 基準は #361 = 1、#362 = 5。画面のレートと保存（3 ファイル）、全ページの要素更新（8〜10 ファイル）、backend の要求と poller（5 ファイル）、proto と生成物、単体・PBT・e2e（10 ファイル超）で、5 より明確に大きい。
- 分けるなら、次の 2 つに分けられる。
  - A（5）：画面のレートの選択・保存と、プロジェクト・アクティビティ・run 詳細の要素更新、`router.refresh()` の廃止。受け入れ条件 1・2・3・5。
  - B（3）：`WatchStateRequest` と `RefreshDemand`、poller の追従。受け入れ条件 4。
- A だけでは backend が 30 秒のままだと条件 4 を満たさない。B だけでは画面の選択が無い。したがって #40 を閉じるには 1 つの PR で A と B を出す。分ける場合は、A を先に出して条件 4 を残課題にし、B で閉じる。

## 決まっていない点

1. **1 秒の backend 負荷**：画面が開いている間、全プロジェクトを 1 秒で読む。プロジェクトは並列に読むが、いちばん遅いプロジェクトが周期を決めるので、応答が 1 秒を超えるとその分だけ変化の反映も遅れる。プロジェクト数・takt-mcp の応答時間によっては重い。#130 の実測を実装の前後で行い、必要なら「表示中のプロジェクトだけ読む」「run 詳細は要約だけ先に更新する」に変える。
2. **run 詳細の 1 秒ポーリング**：`GetRunDetail` はローカルのログを最大 1 MiB 読む。1 秒で繰り返すと重い可能性がある。WatchState の run 要約（状態・今のステップ・最後のログ時刻）で先に更新し、ステップやログの本文は変化があったときだけ読む案を、実測後に比べる。
3. **裏のタブの扱い**：裏に回っている間も 1 秒で取得するか、`document.visibilityState` で止めるか。負荷を下げるなら止めるが、戻ったときに即時取得が要る。この設計では既存のタイマーのままとし、実測で必要なら足す。
4. **1 秒ごとの全状態の再送の大きさ**：WatchState は接続ごとのレートで状態を送るので、1 秒の画面には変化がなくても毎秒 `DashboardState` 全体が届く。run やゴールが多いと帯域と JSON の組み立てが増える。変化がないときは送らない、または `readAt` だけの軽いイベントにする案を、#130 の実測と合わせて比べる。
5. **レートの共有単位**：ブラウザごと（`localStorage`）にした。家族・端末をまたいで同じ値にしたい場合は、サーバー側の設定か Cookie が要る。オーナーの意図を確認する。
6. **`docs/requirements.md` の更新**：2.7 の「30秒ごと」と質問 12 の回答は、この設計の「画面がないときは 30 秒、画面があるときは要求の間隔」に読み替える必要がある。この作業では `docs/design/issue-40.md` だけをコミットするため、要件文書の更新は実装の PR で行う。
7. **`ProjectsLoader` の扱い**：削除するか、`LiveProjects` を返す薄いラッパーとして残すか。直接呼んでいるテストの移し方で決める。

## 範囲の外

- SSE の再接続・無音の見張り（#383）と、見捨てられた接続の後始末（#385）の作り。今回は再送間隔だけを接続ごとに変える。
- 判断待ちの回答・マージ承認・ゴール登録の書き込みの流れ。
- 認証、待ち受けアドレス（`0.0.0.0:3000`）、接続元の制限、VPN の設定。
- DB の表・列・索引・保存形式。
- takt と takt-mcp の変更。
- `make done` の対象外の任意の検査（`make e2e` は別途）。

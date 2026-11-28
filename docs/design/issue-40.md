# issue #40 の設計：画面の情報をリフレッシュレート（1秒・2秒・5秒・10秒、既定1秒）で要素単位に自動更新する

対象の issue：[j5ik2o/takt-dash#40](https://github.com/j5ik2o/takt-dash/issues/40)
関連：[j5ik2o/takt-dash#130](https://github.com/j5ik2o/takt-dash/issues/130)（毎秒の全ページ再取得の負荷）、[j5ik2o/takt-dash#383](https://github.com/j5ik2o/takt-dash/issues/383)（SSE の無音の見張り）、[j5ik2o/takt-dash#385](https://github.com/j5ik2o/takt-dash/issues/385)（見捨てられた SSE の後始末）
大きさの基準：[j5ik2o/takt-dash#361](https://github.com/j5ik2o/takt-dash/pull/361) = 1、[j5ik2o/takt-dash#362](https://github.com/j5ik2o/takt-dash/pull/362) = 5

確認したコミット：`110e25f`（この作業ツリーのブランチ `j5ik2o/issue-40-design` の基点）。

## 先頭のまとめ（公開 API・保存データ・秘密・守りの仕組み）

| 項目 | 触れるか | 内容 |
| --- | --- | --- |
| 公開 API（proto） | **はい** | `WatchStateRequest` に `refresh_interval_ms`・`screen`・`attempt` を足し、`DashboardState` に `applied_refresh_interval_ms` を足す。いずれも省略可能または 0 で、古いクライアントは今のまま動く。 |
| 公開 API（HTTP） | **はい** | `/api/state` に省略可能な `refresh_ms` を足す。BFF に `/api/activity` と `/api/runs/[slug]` を足す。いずれも画面専用で、既存の呼び出しは変えない。 |
| 保存されるデータ | **いいえ** | DB・ファイル・proto の保存データは変えない。画面の選択だけを `localStorage` に保存する（origin ごと）。 |
| 秘密 | **いいえ** | 秘密・認証・接続元の制限を増やさない。LAN・Tailscale・SoftEther VPN の中は信頼する範囲、というオーナーの決定のままにする（採否 10）。`refresh_ms` は秘密ではない。 |
| 守りの仕組み | **いいえ** | CI・関門・ルールセットの定義は変えない。テストと Quint の仕様の追加だけで、`make done` を通す。 |

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
| 4 | 1秒で takt の変化がおおむね1秒で現れる | backend に「開いている画面が求める間隔の最小値」を持たせ、poller の読み取り間隔と WatchState の再送間隔をその値にする。`minReadIntervalMs`（既定1秒、config で1〜10秒）を下限にする。画面がないとき・hidden のときは設定の 30 秒に戻す。適用した間隔を `applied_refresh_interval_ms` で返す。 |
| 5 | `make done` が通る | proto の生成物を更新し、単体・PBT・e2e を足す。`proto-check`、`fmt`、`clippy`、`test`、`typecheck`、`lint`、`coverage` を通す。実装 PR の受け入れ条件に #130 の実測（実装の前後、1 秒と 30 秒の比較、`GetActivity` の集計を含む）を入れる。 |

## 方針（決めたこと）

1. **レートは origin ごとの設定にする。** 4 つの値（1秒・2秒・5秒・10秒）だけを認め、既定は 1 秒。保存先は `localStorage`。`localStorage` は origin ごとなので、`localhost`・LAN の IP・tailnet のホスト名では別の値になる。同じ origin のタブは `storage` イベントで同じ値にそろえる。サーバーには保存しない。
2. **SSE の状態を要素単位の更新の主経路にする。** 概要・判断待ち・ゴール・サイドバー・プロジェクトは `StateStreamProvider` の状態から描画する。`router.refresh()` は状態更新の経路から外す。
3. **アクティビティと run 詳細は、RSC の初回表示＋BFF のポーリングにする。** 初回は今と同じくサーバーで読んで内容を出し、開いている間は `/api/activity` と `/api/runs/[slug]` を選んだレートで読む。初回が失敗しても空の内容でポーリングを付け、backend の復旧で回復させる。
4. **backend の読み取りと配信を、開いている画面のレートに合わせる。** `WatchStateRequest.refresh_interval_ms` を受け取り、接続中の要求の最小値を poller の読み取り間隔にする。WatchState は接続ごとに、そのレートを超えない頻度で状態を送る（変化は合流させ、最後の状態を送る）。実際に適用した間隔は `DashboardState.applied_refresh_interval_ms` に載せる。
5. **backend に「poller の最短の間隔」を置く。** 既定は 1 秒（`minReadIntervalMs`）。config で 1〜10 秒の範囲で上げられる。要求はこれより短くできない。上げた場合は、適用した間隔が画面の選択と食い違うので、画面に小さく知らせる。
6. **画面がないとき・見えていないときは要求を出さない。** 最後の画面が切れたら `intervalSeconds`（既定 30 秒）へ戻す。`document.hidden` のタブは SSE を閉じ、ポーリングも止める。表示に戻ったら、復元済みのレートで開き直してすぐ取得する。
7. **古いクライアントは今のまま動かす。** `refresh_interval_ms` が 0 または未指定なら、読み取りは設定の間隔、WatchState の再送は 15 秒のまま。
8. **レートの復元を最初の接続より先に行う。** `localStorage` の読み取りが終わるまで SSE を開かず、ポーリングも始めない。初回の表示は layout が渡す状態で出す。
9. **レート変更は同じ画面の次の試行として扱う。** `screenId` を保ち、`attempt` を進めて開き直す。#385 の `screen-streams` の付け替えで古い接続を閉じ、backend の要求も置き換える。

## 変える場所

### proto

- `proto/taktdash.proto` の `WatchStateRequest` に `uint32 refresh_interval_ms = 1;`、`string screen = 2;`、`uint64 attempt = 3;` を足す。画面の `screen`/`attempt` は `/api/state` が既に受け取っている値をそのまま渡し、backend の要求の置き換えに使う。
- `DashboardState` に `uint32 applied_refresh_interval_ms = 9;` を足す。WatchState が接続ごとに実際に適用した送信間隔を入れる。unary の応答では 0 のまま。
- `refresh_interval_ms` は 0（指定なし）または 1000〜10000 の整数だけを受け付け、それ以外は `INVALID_ARGUMENT` にする。画面の選択肢の上限が 10 秒なので、受理範囲も 10 秒にそろえる。
- 受理範囲の定義は 1 か所にまとめる。`proto/refresh-interval.json` に `{"minMs":1000,"maxMs":10000}` を置き、frontend は `refresh-rate.ts` から import、backend は `refresh.rs` の定数がこの JSON と一致することをテストで固定する。BFF と backend の受理範囲が食い違わないことを同じ表で確かめる。
- `make proto-gen` で `frontend/web/src/proto/taktdash.ts` を更新し、`make proto-check` が通ることを確かめる。

### frontend：レートの選択と保存

- 新規 `frontend/web/src/state/refresh-rate.ts`
  - `REFRESH_RATE_OPTIONS = [1000, 2000, 5000, 10000] as const`。正本は `proto/refresh-interval.json` の `minMs`・`maxMs` とし、この配列がその範囲に収まることをテストで固定する。
  - `DEFAULT_REFRESH_RATE_MS = 1000`
  - `parseRefreshRateMs(value: unknown): number`：文字列・数値・`null`・`undefined`・壊れた値をすべて受け、4 つのどれかならその値、それ以外は既定の 1000 を返す。例外を投げない。
  - `parseRefreshIntervalMs(raw: string | null): number | undefined`：`/api/state` の `refresh_ms` 用。`null`・空は「指定なし」（`undefined`、古い画面と同じ）にし、数値の範囲外は 1000〜10000 へ丸める。数値でない壊れた値は 1000 にする。ストリームを開けない値を作らない。
  - `refreshRateLabel(ms): string`：`1秒`・`2秒`・`5秒`・`10秒`。
- 新規 `frontend/web/src/components/refresh-rate-provider.tsx`
  - `RefreshRateProvider` と `useRefreshRate()`。値は `refreshRateMs`、`ready`、`setRefreshRateMs`。
  - 初期値は 1000、`ready` は false（SSR とハイドレーションを一致させるため、描画中は `localStorage` を読まない）。`useEffect` で `localStorage.getItem("taktdash.refreshRateMs")` を読み、`parseRefreshRateMs` で検証してから設定し、`ready` を true にする。`ready` までは SSE を開かず、ポーリングも始めない（採否 13）。
  - `setRefreshRateMs` は state を更新し、`localStorage.setItem` を試す。`SecurityError` や容量不足でも state は更新し、画面は動き続ける。
  - `storage` イベントを聴き、同じ origin の別のタブの変更を反映する。`localStorage` は origin ごとなので、`localhost`・LAN の IP・tailnet のホスト名では別の値になる（採否 15）。
- 新規 `frontend/web/src/components/page-visibility.ts`（または `refresh-rate-provider.tsx` 内）
  - `usePageVisible()` を足し、`document.visibilityState === "visible"` を返す。`visibilitychange` を聴き、`hidden` になったら SSE を閉じてポーリングを止め、`visible` に戻ったらすぐ取得する（採否 2）。
- 新規 `frontend/web/src/components/refresh-rate-select.tsx`（または `Sidebar` の一部）
  - `<label for="refresh-rate">更新間隔</label>` と `<select id="refresh-rate">`。選択肢は `1秒`・`2秒`・`5秒`・`10秒`。既定は `1秒`。
  - 変更時に `setRefreshRateMs` を呼ぶ。現在の値には `aria-current` ではなく `selected` を使う。
- `frontend/web/app/layout.tsx`
  - `RefreshRateProvider` で `StateStreamProvider` を包む。`Sidebar` にレートの選択と適用間隔の通知を渡す。
- `frontend/web/src/components/sidebar.tsx`
  - `refreshRate?: { value: number; onChange: (ms: number) => void }` と `appliedRefreshIntervalMs?: number` を受け取り、「ページ」の下に「更新間隔」の節を出す。渡されない既存のテストは今のまま動く（任意 prop）。
  - 選択した値より適用間隔が遅いとき、または古い backend で適用間隔が 0 のときは、小さく「backend は 5 秒間隔で更新しています。」のように出す（採否 5）。
- `frontend/web/src/components/live-sidebar.tsx`
  - `useRefreshRate()` の値と、`useLiveState()` の `state.appliedRefreshIntervalMs`・`liveStateReceived` を `Sidebar` に渡す。SSE の最初の受信後にだけ、適用間隔の食い違いを知らせる。

### frontend：SSE とページ単位の再取得の廃止

- `frontend/web/src/components/state-provider.tsx`
  - `useRefreshRate()` の `refreshRateMs`・`ready` と `usePageVisible()` を読み、`ready && visible` のときだけ `openStateStream` を開く。URL は `/api/state?refresh_ms=${refreshRateMs}`。既存の `withStreamTicket` が `&screen=…&attempt=…` を足す。
  - `screenId` は Provider が 1 つ持ち、レート変更では同じ `screenId` のまま試行番号を進めて開き直す。#385 の `screen-streams` が同じ画面の次の試行として古い接続を閉じる（採否 19）。`openStateStream` に `initialAttempt` を足すか、試行番号を Provider が持つ関数を渡して、内部の再接続とレート変更で番号が戻らないようにする。
  - `revalidate` prop と `useRouter` を消す。`StateStreamProviderWithRefresh` は廃止し、`layout.tsx` は `StateStreamProvider` を直接使う。
  - レートが変わると `useEffect` の依存が変わり、古い EventSource を閉じて新しい URL で 1 本だけ開き直す。状態は Context に残るので、画面は消えない。
  - SSE の状態を一度でも受けたかを `liveStateReceived` として Context に足す。初回の RSC 状態（`applied_refresh_interval_ms = 0`）では「backend が対応していない」と出さず、SSE の最初の受信後にだけ出す。
- `frontend/web/src/components/state-stream.ts`
  - `createStateHandlers` から `revalidate` を消す。`onState` は状態を適用するだけにする。
  - `screenId`・`initialAttempt` の省略可能なオプションを足す。再接続・見張り・`checkStale`・試行の URL は変えない。
- `frontend/web/src/state/restore-state.ts`
  - `dashboardStateFromProto` と SSE の復元で `appliedRefreshIntervalMs` を読む。0 は「未対応」として残す。
- `frontend/web/src/grpc/client.ts`
  - `watchState(request?: { refreshIntervalMs?: number; screen?: string; attempt?: number })` にし、`WatchStateRequest` の各フィールドを渡す。省略時は 0・空。
- `frontend/web/app/api/state/route.ts`
  - `refresh_ms` を読み、`parseRefreshIntervalMs` で 1000〜10000 に丸める。無い・空は指定なし（0）として古い画面と同じにし、数値の範囲外は丸め、数値でない壊れた値は 1000 にする。ストリームを開けない値を作らない（採否 6）。
  - `screen`・`attempt` を `parseStreamTicket` から取り、`openStream: () => getBackendClient().watchState({ refreshIntervalMs: refreshMs, screen, attempt })` にする。`screenStreams` の付け替えの扱いは変えない。

### frontend：プロジェクトを SSE の状態にする

- 新規 `frontend/web/src/components/live-projects.tsx`
  - `useLiveState()` の `state` を `ProjectsContent` に渡す。`now` は `state.readAt ?? new Date()` を基準に 1 秒で進める（待ち時間の表示のため。取得はしない。採否 20）。`StateStreamProvider` の内部型は proto の `DashboardState` の部分型なので、`ProjectsContent` の prop の型は変えずに渡せる。
  - `disconnected` のときは既存の `BackendUnavailableNotice` を出し、前に読めた状態は残す（#309 と同じ）。`snapshotLoaded` が false で状態がないときは内容を出さない。
- `frontend/web/app/projects/page.tsx`
  - `state` prop がないときは `ProjectsLoader` を返す。`state` prop があるテスト用の経路は残す。
- `frontend/web/src/components/projects-loader.tsx`
  - RSC の取得はやめるが、`measurePage("/projects", async () => <LiveProjects />)` の計測点として残す（採否 18）。RPC は発行しないので、`page=/projects` の行は `rpc_ms=0` になる。直接呼んでいるテストは `LiveProjects` のテストへ置き換える。

### frontend：アクティビティをポーリングする

- 新規 `frontend/web/app/api/activity/route.ts`
  - `getBackendClient().getActivity()` を呼び、`GetActivityResponse.toJSON` を `Cache-Control: no-store` で返す。失敗は既存の `toBackendUnavailableResponse` に合わせる。
  - 読めた応答を `lastActivity` に覚える（#309 のキャッシュ）。`lastActivity` は `activity-loader.tsx` の module 変数から `src/state/last-loaded.ts` へ移し、Route Handler と Loader で共有する。
- 新規 `frontend/web/src/components/live-activity.tsx`
  - 初回の `response`（無いこともある）と `readAt` を受け取り、`useLiveState()` の `state` と合わせて `ActivityContent` を描く。`response` が無いときは通知だけを出し、取得できたら内容を出す（採否 7）。
  - `ready && visible` のとき、`startPolling` で `/api/activity` を `refreshRateMs` 間隔で読む。成功したら `response` と `now` を差し替え、失敗したら直前の内容を残して `banner warn` を出す。404 は無い。
  - `ActivityContent` は proto の `GetActivityResponse` を受けるが、内部型は proto 型の部分型なので、復元した値はそのまま渡せる。
- `frontend/web/src/components/activity-loader.tsx`
  - 今の RSC の取得は残し、成功・キャッシュの結果を `LiveActivity` の初期値として渡す。初回の内容を SSR で出すため。取得に失敗してキャッシュも無いときも `LiveActivity` を出し、空の内容でポーリングを始める（採否 7）。
- `frontend/web/app/activity/page.tsx`
  - `response` prop がないときは `ActivityLoader`（初回 RSC）を返し、その中で `LiveActivity` を描く。
- `frontend/web/src/state/restore-detail.ts`（または `restore-state.ts`）
  - `restoreActivityResponse(payload)` を足す。`events`・`dailySummaries`・`todayCountsByServer` の時刻と enum を復元する。`state` は `useLiveState` の状態を使うので、応答の `state` は省略してよい。

### frontend：run 詳細をポーリングする

- 新規 `frontend/web/app/api/runs/[slug]/route.ts`
  - `getBackendClient().getRunDetail(slug)` を呼び、`GetRunDetailResponse.toJSON` を `no-store` で返す。`RunNotFoundError` は 404、`BackendUnavailableError` は既存の 503 に写す。
  - 読めた応答を `lastRunDetails` に覚える。`lastRunDetails` は `run-detail-loader.tsx` の module 変数から `src/state/last-loaded.ts` へ移す。
- 新規 `frontend/web/src/components/live-run-detail.tsx`
  - 初回の `detail`（無いこともある）と `serverName` を受け取り、`ready && visible` のとき `startPolling` で `/api/runs/${slug}` を `refreshRateMs` 間隔で読む。成功したら `detail` を差し替え、失敗したら直前の内容を残して `banner warn` を出す。404 は「見つかりません」を出してポーリングを止める。初回が失敗してキャッシュも無いときも、空の内容でポーリングを始める（採否 7）。
  - 1 ページで同時に走る定期的な取得は 2 本（run 詳細と、`LiveRunLog` の `/api/runs/[slug]/updates`）。`RunContentToggle` の本文取得は利用者が開いたときだけの 1 本。詳細は WatchState の run 要約（状態・今のステップ・最後のログ時刻）で変化を検知したときだけ読む形にできるなら 1 本に減らす。まず 2 本で始め、#130 の実測で決める（採否 4）。
  - `serverName` は `useLiveState()` の `state.servers` から `serverId` で引き直し、見つからなければ初回の値を使う。
  - `RunDetailContent` に新しい `detail` を渡す。`LiveRunLog` は今と同じく `key={detail.runSlug}` で、ログの追記分は自分で取る。詳細の応答で `logTail` が変わっても、`LiveRunLog` の積んだ行を初期化しない（採否 16）。
- `frontend/web/src/components/run-detail-loader.tsx`
  - 今の RSC の取得は残し、`LiveRunDetail` に初期値を渡す。404 の `notFound()` も残す。取得に失敗してキャッシュも無いときも `LiveRunDetail` を出し、空の内容でポーリングを始める（採否 7）。
- `frontend/web/src/state/restore-detail.ts`
  - `restoreRunDetail(payload)` を足す。文字列・時刻・`RunStatus`・`StepLog`・`RunReport`・`RunInstruction` を復元する。ブラウザで `@grpc/grpc-js` を値として読まないよう、生成コードの `fromJSON` は使わない。

### frontend：既存のポーリングをレートに合わせる

- `frontend/web/src/components/polling.ts`
  - 間隔は今のとおり「前回の取得の完了から次まで」を保つ（`polling.ts:61-62` と同じ）。前回が終わる前に次を重ねない。
  - 失敗が続いたら間隔を空ける。失敗 1 回目は `intervalMs`、2 回目以降は倍にし、上限（例：`max(intervalMs, 30_000)`）で止める。成功したら `intervalMs` に戻す。待ちの途中で abort されたら取得しない。
  - `stop` / abort 後は取得も適用もしない、という今の契約は変えない。
  - `ready` と `visible` を受け取り、どちらかが false の間は取得しない。true になったらすぐ 1 回取得する。
- `frontend/web/src/components/live-task-detail.tsx`
- `frontend/web/src/components/live-goal-detail.tsx`
- `frontend/web/src/components/live-conversation.tsx`
- `frontend/web/src/components/live-run-log.tsx`
  - `useRefreshRate()` と `usePageVisible()` を読み、`intervalMs` に `refreshRateMs` を渡す。レートが変わったら effect を張り直し、古いタイマーを止める。`ready` と `visible` が false の間は取得しない。
  - 会話は `setInterval` を `startPolling` と同じ「前回の完了から次まで」の扱いに直す（前回の `poll` が終わるまで次を始めない点は今のまま）。
- `frontend/web/src/components/task-detail-loader.tsx`・`goal-detail-loader.tsx`・`conversations-loader.tsx`
  - 初回が `BackendUnavailableError` でキャッシュも無いときも、空の内容で `LiveTaskDetail`・`LiveGoalDetail`・`LiveConversation` を出し、ポーリングで回復させる（採否 7）。404 は今のとおりページの 404。
- `frontend/web/src/components/live-overview.tsx` と `frontend/web/src/components/live-projects.tsx` の経過時間の時計は、取得ではなく表示の時計なので 1 秒で進める（採否 20）。

### backend：読み取りと配信を画面のレートに合わせる

- 新規 `backend/src/refresh.rs`
  - `RefreshDemand`：要求の `BTreeMap` と、最小値を配る `tokio::sync::watch` を持つ。各要求は `screen`・`attempt`・希望間隔・`last_seen`・`ttl`・一意な番号を持つ。
  - `DemandGuard`：`acquire(screen, attempt, Duration) -> DemandGuard` が返す。`Drop` では、登録がまだ自分の番号を指しているときだけ外す（置き換え後の古い guard が新しい要求を消さない）。二重解放しない。
  - 同じ `screen` の新しい `attempt` は、古い要求を置き換える（1 画面 1 要求）。`attempt` が小さい遅れた要求は登録しない。`screen` が空（古いクライアント）は接続ごとの一意な番号で登録し、置き換えない。
  - `last_seen` は取得時と、その接続で状態を送るたびに進める。`ttl` は「その接続の適用間隔（`applied_for`）の数倍」とし、少なくとも 3 秒。寿命を過ぎた要求は `effective` の計算前に外す。半開きで送信が詰まった接続はここで外れる。送信が続く半開きは、同じ `screen` の次の試行（#383 の再接続）が置き換える（採否 3）。
  - `effective(config: Duration, min_read: Duration) -> Duration`：`max(min_read, min(config, 生きている要求の最小値))`。要求が無ければ `max(min_read, config)`。`min_read` は config の `minReadIntervalMs`。
  - `applied_for(guard) -> Duration`：その接続が実際に使う送信間隔（要求を `min_read` で下限にする）。`DashboardState.applied_refresh_interval_ms` に載せる。
  - `subscribe() -> watch::Receiver<Option<Duration>>`：poller が待つための受信。
  - `MIN_REFRESH_INTERVAL = 1s`、`MAX_REFRESH_INTERVAL = 10s`。範囲の正本は `proto/refresh-interval.json`。
- `backend/src/lib.rs`
  - `mod refresh;` を足す。
- `backend/src/grpc/mod.rs`
  - `TaktDashServiceImpl` と `ServiceDeps` に `refresh_demand: RefreshDemand` を足す。
  - `watch_state`：`WatchStateRequest.refresh_interval_ms`・`screen`・`attempt` を読む。`screen` は空か 32 文字の 16 進、`attempt` は 1 以上（空の screen のときは 0 でもよい）だけを受け付け、それ以外は `INVALID_ARGUMENT`。`refresh_interval_ms` は 0 なら要求なし、1000〜10000 なら `DemandGuard` を取る。それ以外は `INVALID_ARGUMENT`。`DemandGuard` は送信タスクへ移し、ストリームの終了（`sender.closed()`・停止・購読の終端）で落とす。
  - 再送と変化の送信を、接続ごとのレートに合わせる。初期状態は今すぐ送る。その後は `next_send = now + interval` まで待ち、変化通知は合流させ、期限に最後の状態を 1 回だけ組み立てて送る。`interval` は要求があれば `max(要求, minReadIntervalMs)`、無ければ今の `WATCH_RESEND_INTERVAL`（15 秒）。1 回の送信の後は必ず `next_send` を進めるので、同じ接続でレートより速く送らない。
  - 送る `DashboardState` の `applied_refresh_interval_ms` に、その接続の `interval` を入れる。初期状態にも入れる。unary の応答は 0 のまま。
  - 既存の `WATCH_CHANNEL_CAPACITY`・`WATCH_RESEND_INTERVAL`・停止の扱いは残す。
- `backend/src/poller.rs`
  - `Poller` に `configured_interval`・`min_read_interval`・`refresh_demand` を足す。`with_refresh_demand` で共有する。テストは指定しなければ要求なしの `RefreshDemand` を持つ。
  - `read_cycle_once_at` で `configured_interval = config.interval_seconds`、`min_read_interval = config.min_read_interval` を更新し、`CycleReport.interval = refresh_demand.effective(configured_interval, min_read_interval)` にする。
  - `effective_interval()` を足し、要求が変わったときに poller が次の待ち時間を計算し直せるようにする。
- `backend/src/poller_stop.rs`
  - `run_until_stopped` の待機に `refresh_demand.subscribe()` の変更を足す。待っている間に新しい画面が 1 秒を要求したら、残りの待ちを切り上げて、新しい間隔で次の周期を始める。要求が減ったら、新しい長い間隔で待ち直す。停止の合図と `CYCLE_GRACE` の扱いは変えない。
  - 設定の読み込みに失敗した周期の待ちも、`refresh_demand.effective(DEFAULT_INTERVAL_SECONDS, min_read_interval)` を通す（採否 14）。
- `backend/src/main.rs`
  - `RefreshDemand::new()` を 1 つ作り、`ServiceDeps` と `Poller` の両方へ同じものを渡す。
- `backend/src/config.rs`
  - `DEFAULT_INTERVAL_SECONDS = 30` は変えない。画面がないときの間隔として残す。
  - `minReadIntervalMs` を足す。既定は 1000、範囲は 1000〜10000。`intervalSeconds` の検証（0 は不正）も変えない。範囲外は `ConfigError::Invalid`。`taktdash.config.example.json` にも `"minReadIntervalMs": 1000` を足す。
  - `minReadIntervalMs` を 10 秒より大きくできない理由：`frontend/web/src/state/state-stream.ts:52` の `STATE_STREAM_STALE_TIMEOUT_MS = 40_000` と、`specs/quint/state-stream-watchdog` の `STALE_TIMEOUT > 2 * RESEND_INTERVAL` を保つため。再送の上限が 10 秒なら 2×10=20 秒 < 40 秒で、見張りが誤って切る余裕は 20 秒ある。要求なしの 15 秒でも 2×15=30 秒 < 40 秒である。10 秒より遅い間隔を許すなら、40 秒の見張りと Quint の不変条件を同時に上げる必要がある。
- `docs/requirements.md`
  - 2.7 の「30秒ごと」と質問 12 の回答を、「画面がないときは `intervalSeconds`（既定30秒）、画面が要求しているときは `minReadIntervalMs` 以上で要求の間隔」に更新する。同じ実装 PR の範囲に入れる（採否 17）。この作業では `docs/design/issue-40.md` だけをコミットする。

### テスト

- 新規・更新する単体テスト（frontend）
  - `test/state/refresh-rate.test.ts`：4 つの値、既定、壊れた値、`localStorage` の往復。
  - `test/components/refresh-rate-provider.test.ts`：保存値の復元が最初の接続より先になること（`ready`）、変更の保存、`localStorage` が使えないときの継続、`storage` イベント。
  - `test/components/live-provider-wiring.test.ts`：レート変更で EventSource が 1 本だけになり、URL に `refresh_ms` が載り、同じ `screenId` の次の `attempt` になる。`revalidate` のテストは削除・置換する。
  - `test/components/state-revalidation.test.ts`：状態を受けても RSC の再取得を促さない契約に置き換える。
  - `test/components/state-stream.test.ts`：`createStateHandlers` から `revalidate` が消えた契約に直す（再接続・見張りのテストは残す）。`screenId`・`initialAttempt` の省略時も今のまま。
  - `test/api/state-route.test.ts`：`refresh_ms` の丸め（0・999・10001・壊れた値）と、`screen`・`attempt` が `watchState` へ渡ること。
  - `test/components/polling.test.ts`：前回の完了から次まで、失敗の backoff、abort、レート変更、`ready`/`visible`。
  - `test/components/page-visibility.test.ts`：hidden で取得しない、visible で即時取得。
  - `frontend/web/src/proto-contract.test.ts`：`WatchStateRequest.refreshIntervalMs`・`screen`・`attempt` と `DashboardState.appliedRefreshIntervalMs` の型が生成物にあること。
  - `test/support/refresh-interval.test.ts`：`proto/refresh-interval.json` と `REFRESH_RATE_OPTIONS`・backend の定数の一致。
  - `test/components/live-projects.test.ts`：SSE の状態だけで更新される。
  - `test/api/activity-route.test.ts`、`test/api/run-route.test.ts`：JSON、404、503、キャッシュ。
  - `test/state/restore-detail.test.ts`：`restoreRunDetail`・`restoreActivityResponse`。
- 新規・更新する単体テスト（backend）
  - `backend/tests/refresh_demand_test.rs`（または `backend/src/refresh_tests.rs`）：複数の要求の最小、同じ `screen` の新しい `attempt` の置き換え、解放、二重解放、TTL で外れる、0 件で設定に戻る、範囲外、`minReadIntervalMs` の下限。
  - `backend/tests/watch_state_test.rs`：`refresh_interval_ms = 1000` で、状態が変わらなくても 1 秒程度で再送される。`applied_refresh_interval_ms` が返る。0 のときは 15 秒のまま（短い待ちで何も来ないことを確かめる既存テストを維持）。古い `WatchStateRequest {}` が今のまま動く。
  - `backend/tests/poller_test.rs`：要求があるときの `CycleReport.interval`、`minReadIntervalMs` が要求より大きいときの下限、要求が消えたら設定に戻る、待機中に要求が増えたら周期を切り上げる、設定エラーの待ちも要求を反映する。
  - `backend/tests/config_test.rs`：`minReadIntervalMs` の既定・範囲・範囲外のエラー。
- e2e
  - `e2e/tests/issue-40-refresh-rate.spec.ts`（新規）：既定 1 秒、4 つの選択、再読み込み後の復元。
  - 同：概要とプロジェクトで、状態ファイルの変更が再読み込みなしで 3 秒以内に現れる。同じ document の印を置いて確かめる。
  - 同：アクティビティ・run 詳細・タスク詳細・ゴール詳細・会話・run ログが、レートを 5 秒にすると 5 秒周期になる（要求の時刻を数える）。1 秒では 3 秒以内に現れる。
  - 同：判断待ちの入力欄に文字を入れ、状態更新の後も値とスクロール位置が残る。
  - 同：初回の `GetActivity`・`GetRunDetail` が失敗しても、空の内容でポーリングが始まり、backend の復旧で内容が出る（採否 7）。
  - `e2e/support/stack.ts` の backend の既定を `intervalSeconds: 30` にする。1 秒の backend が要る既存テストは明示的に `intervalSeconds: 1` を渡す。既定 30 秒のまま、画面の 1 秒要求で backend が 1 秒へ切り替わることを 1 本だけ確かめる（採否 8）。
  - 既存の `e2e/tests/flow-9-run-detail-live-log.spec.ts` は `router.refresh` の `rsc=1` を待っているので、run 詳細のポーリング（`/api/runs/[slug]`）を待つ形に直す。置き換えたテストで、毎回の `/api/runs/[slug]` の応答ではログが初期化されず、積んだ行が残ることを確かめる（採否 16）。
  - `e2e/tests/issue-430-error-reload.spec.ts` の「状態ストリームの受信で router.refresh が走る」という前提のコメントを直す（ボタンの再取得の確認はそのまま）。
  - hidden の停止は、`document.visibilityState` を差し替える単体テストで確かめる。e2e では visible の経路だけを見る。

## 状態と遷移

### 画面のレート

| 状態 | 値 | 遷移 |
| --- | --- | --- |
| 初期（SSR） | 1000ms、`ready=false` | サーバーとハイドレーションを一致させるため、保存値を読まない。SSE もポーリングも始めない |
| 保存値の復元 | 1000/2000/5000/10000 | `useEffect` で `localStorage` を読み、`parseRefreshRateMs` を通し、`ready=true` にする。ここから最初の接続を開く（採否 13） |
| 利用者の選択 | 4 つのどれか | state を更新し、`localStorage` に書く。同じ `screenId` の次の `attempt` で SSE を開き直す。ポーリングの effect を張り直す（採否 19） |
| 同じ origin の別タブ | 4 つのどれか | `storage` イベントで同じ値へ移る |
| 別の origin・ブラウザ・端末 | それぞれの値 | `localStorage` は origin ごとなので、`localhost`・LAN の IP・tailnet のホスト名で別の値になりうる（採否 9・15） |
| hidden | 取得しない | SSE を閉じ、ポーリングを止める。visible に戻ったら、復元済みのレートで開き直してすぐ取得する（採否 2） |
| 保存できない | 選んだ値（メモリだけ） | `localStorage` の例外を握りつぶし、画面はその値で動く |

- 保存先は `localStorage` のキー `taktdash.refreshRateMs`。値は `"1000"` などの文字列。DB・ファイル・Cookie には保存しない。
- 同じ origin のタブは同じ値になる。別の origin・ブラウザ・端末では違いうる。全端末で同じ値にしたい場合は、サーバー側の設定か Cookie が要る（この設計の範囲外）。

### backend の要求と読み取り

```
画面の選択 (localStorage, ready 後)
  → /api/state?refresh_ms=1000（同じ screenId・次の attempt）
  → WatchStateRequest.refresh_interval_ms = 1000, screen, attempt
  → RefreshDemand（1 画面 1 要求、lastSeen と TTL、置き換え）
  → Poller の読み取り間隔 = max(minReadIntervalMs, min(intervalSeconds, 生きている要求の最小値))
  → 変化通知
  → WatchState が接続ごとのレート（要求を minReadIntervalMs で下限にした値）で状態を送る（変化は合流）
  → DashboardState.applied_refresh_interval_ms
  → SSE
  → React の状態 / ポーリング
```

- 要求は `WatchState` の接続ごとに 1 つ。同じ `screen` の新しい `attempt` は古い要求を置き換える。接続が切れたら `DemandGuard` が落ち、最小値を計算し直す。Next の Route Handler が gRPC を閉じれば、backend は `sender.closed()` で気づく。
- 要求は `lastSeen` を持ち、状態を送るたびに進む。`ttl`（その接続の適用間隔の数倍、最低 3 秒）を過ぎた要求は外す。送信が詰まった半開きはここで外れ、送信が続く半開きは同じ `screen` の次の試行が置き換える（採否 3）。
- 画面がないとき・hidden のときは要求が 0 件なので、poller は `intervalSeconds`（既定 30 秒）で読む。
- 複数の画面があるときは、いちばん速い要求に合わせる。10 秒の画面と 1 秒の画面が同時にあれば、読み取りは 1 秒。10 秒の画面への送信は 10 秒ごと（変化は合流）で、1 秒の画面への送信は 1 秒ごと。
- `minReadIntervalMs` が 5 秒なら、1 秒の要求でも読み取りと送信は 5 秒になる。`applied_refresh_interval_ms` が 5000 になり、画面は「backend は 5 秒間隔で更新しています」と小さく知らせる。
- 設定の `intervalSeconds` が 1 秒など要求より速い場合は、設定のほうが速い（`min`）。ただし `minReadIntervalMs` より短くはならない。

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
- 初回の取得が失敗して内容が無い場合も、ライブの wrapper を出してポーリングを始める。404 は除く（採否 7）。
- 前回の取得が終わる前に次を重ねない。abort 後は取得も適用もしない。失敗が続いたら backoff で間隔を空ける。

## 守る不変条件

1. **レートは 4 つのどれか**：`refreshRateMs` は 1000・2000・5000・10000 のどれか。保存値が壊れていても既定の 1000 になる。BFF と backend の受理範囲は 1000〜10000 で一致する。
2. **既定は 1 秒、再読み込み後も保たれる**：保存値がなければ 1000。保存できたら次の読み込みで同じ値になる。保存できない環境でも画面はその値で動く。復元は最初の接続より先に行う。
3. **データ更新でページ全体を再取得しない**：SSE の受信で `router.refresh()` を呼ばない。ポーリングは対象の BFF だけを読む。
4. **接続ごとのレートを超えて送らない**：WatchState は初期状態の後、接続の `interval` の間に 1 回までしか送らない。変化が何度来ても、最後の状態を 1 回にまとめる。`interval` は `max(要求, minReadIntervalMs)`。
5. **要求の最小値を読み取りに使う**：`Poller` の間隔は `max(minReadIntervalMs, min(intervalSeconds, 生きている要求の最小値))`。要求が無ければ `max(minReadIntervalMs, intervalSeconds)`。
6. **要求は接続の終了・置き換え・寿命で外れる**：`DemandGuard` は正常終了・エラー・停止・クライアント切断のどれでも 1 回だけ外れる。同じ `screen` の新しい `attempt` は古い要求を置き換え、置き換え後の古い guard は新しい要求を消さない。`lastSeen` が `ttl` を過ぎた要求は外れる。外れ忘れて 1 秒のまま残らない。
7. **適用した間隔を知らせる**：WatchState の各状態の `applied_refresh_interval_ms` は、その接続が実際に使う送信間隔。画面の選択と違えば小さく知らせる。古い backend で 0 なら「対応していない」と知らせる。
8. **1 秒のとき、変化がおおむね 1 秒で届く**：読み取り周期 1 秒＋状態の組み立て・送信・描画。LAN の実測で 1〜2 秒程度を目標にし、e2e は 3 秒以内で確かめる。読み取りが 1 秒を超えるプロジェクトがあれば、その分だけ遅れる。
9. **取得は重ならず、失敗したら間隔を空ける**：同じ画面の同じ経路で、前回の取得が終わる前に次を始めない。abort 後は取得も適用もしない。失敗が続いたら上限つきの backoff で間隔を空ける。
10. **失敗しても内容を消さない**：取得に失敗したら直前の内容を残し、通知を出す。次の成功で戻す（#309 と同じ）。初回が失敗して内容が無くても、ポーリングを付けて復旧させる。
11. **見えていないタブは取得しない**：`document.hidden` の間は SSE を閉じ、ポーリングも止める。`visible` に戻ったらすぐ取得する。
12. **古いクライアントは今のまま**：`refresh_interval_ms = 0` のときは、読み取りは設定の間隔、再送は 15 秒。`/api/state` の引数なしも同じ。
13. **LAN・VPN の使い方を壊さない**：Next の待ち受け（`0.0.0.0:3000`）・gRPC の待ち受け・認証・接続元の制限を変えない。新しい経路は同じ origin の HTTP だけで、新しいポートを開けない。
14. **保存データを変えない**：DB の表・列・索引・保存形式を変えない。`localStorage` 以外に選択を保存しない。

## テストにできる性質（例・PBT・e2e）

| ID | 性質 | 確かめ方 | 置き場所 |
| --- | --- | --- | --- |
| E1 | 4 つの値だけを受け、それ以外は 1000 になる。BFF は範囲外を 1000〜10000 に丸める | 例 | `test/state/refresh-rate.test.ts` |
| E2 | 保存した値が最初の接続より先に戻る。保存できない環境でも動く | 例 | `test/components/refresh-rate-provider.test.ts` |
| E3 | `select` の選択肢が 4 つで、既定が 1 秒。変更が state と保存へ伝わる | 例 | 同 |
| E4 | レート変更で EventSource が 1 本だけになり、同じ `screenId` の次の `attempt`・`refresh_ms` が載る | 例 | `test/components/live-provider-wiring.test.ts` |
| E5 | 状態を受けても `router.refresh` を呼ばない | 例 | `test/components/state-revalidation.test.ts`（置換） |
| E6 | プロジェクトは SSE の状態だけで更新される。時計は 1 秒 | 例 | `test/components/live-projects.test.ts` |
| E7 | アクティビティは `/api/activity` だけをレートで読む。初回失敗でもポーリングする | 例 | `test/components/live-activity.test.ts`、`test/api/activity-route.test.ts` |
| E8 | run 詳細は `/api/runs/[slug]` だけをレートで読む。ログは updates を読む。詳細の応答でログを初期化しない | 例 | `test/components/live-run-detail.test.ts`、`test/api/run-route.test.ts` |
| E9 | 前回の完了から次まで。失敗の backoff、abort 後 0 回 | 例 | `test/components/polling.test.ts` |
| E10 | 要求の最小値が読み取り間隔になる。解放・置き換え・TTL で設定に戻る | 例 | `backend/tests/refresh_demand_test.rs` |
| E11 | 1 秒の要求で、状態が変わらなくても 1 秒程度で再送され、適用間隔が返る | 例 | `backend/tests/watch_state_test.rs` |
| E12 | 0 の要求（古いクライアント）は今のまま | 例 | 同 |
| E13 | レートの選択が再読み込み後も残る（origin ごと） | e2e | `e2e/tests/issue-40-refresh-rate.spec.ts` |
| E14 | 全ページで、状態の変更が再読み込みなしで現れる | e2e | 同、既存の flow |
| E15 | 同じ document のまま、入力値とスクロール位置が残る | e2e | 同 |
| E16 | 1 秒のとき、変更が 3 秒以内に現れる | e2e | 同 |
| E17 | 5 秒にすると、要求の間隔が 5 秒になる | e2e | 同 |
| E18 | backend の既定 30 秒のまま、画面の 1 秒要求で 1 秒に切り替わる | e2e | 同 |
| E19 | hidden では取得せず、visible で即時取得する | 例 | `test/components/page-visibility.test.ts` |
| E20 | 画面の選択と `applied_refresh_interval_ms` が違えば通知する。0 でも通知する | 例 | `test/components/live-sidebar.test.ts` |
| P1 | `parseRefreshRateMs` はどんな値でも例外を投げず、4 つの値か 1000 を返す。BFF の丸めも範囲内 | PBT | `test/state/refresh-rate.test.ts` |
| P2 | 任意の acquire/release/replace/expire の列で、`effective` は `max(minRead, min(config, 生きている要求の最小値))`（要求なしは `max(minRead, config)`）。解放・置き換え・TTL で他の要求を壊さない | PBT | `backend/tests/refresh_demand_test.rs` |
| P3 | 任意の変化の到着と期限の列で、送信は interval より速くなく、変化を合流させて最後の状態を送る（途中の版を送らなくてよいが、最後の版は落とさない） | PBT | `backend/src/grpc` の純粋な送信スケジュール関数 |
| P4 | 任意の取得完了・失敗・abort・レート変更の列で、取得は重ならず、backoff は上限を超えず、abort 後は 0 回 | PBT | `test/components/polling.test.ts` |
| P5 | 任意のベース URL に `screen`・`attempt` と `refresh_ms` を付けても、両方が保たれる | PBT | `test/support/stream-url.ts` の拡張 |

- PBT は既存の frontend と同じく、シード固定の PRNG で操作列を作る。除外は付けない。
- 送信スケジュール（P3）は、`now`・`last_sent`・`interval`・`dirty` を受けて「送る／いつまで待つか」を返す純粋関数に切り出し、`tokio::time` の実時間に頼らずに確かめる。実時間の 1 秒は E11 と e2e で確かめる。
- 受理範囲は `proto/refresh-interval.json` を正本にし、frontend の `REFRESH_RATE_OPTIONS` と BFF の丸め、backend の `MIN_REFRESH_INTERVAL`・`MAX_REFRESH_INTERVAL` が一致することを `test/support/refresh-interval.test.ts` と `backend/tests/refresh_demand_test.rs` で固定する（採否 6）。
- `make done` の coverage 80% を満たすため、新規の `refresh-rate.ts`・`RefreshDemand`・`LiveActivity`・`LiveRunDetail`・新しい Route Handler には対応するテストを置く。

## 壊しうる入力

| 入力・状況 | 期待する振る舞い |
| --- | --- |
| `localStorage` が無い・読めない・書き込めない（プライベートモード、容量不足） | 例外を握りつぶし、メモリの値で動く。既定は 1000 |
| 保存値が `""`・`"0"`・`"1001"`・`"1e3"`・`"NaN"`・`"１０秒"`・巨大な文字列・Unicode | 1000 に戻す。例外を投げない |
| `refresh_ms` が無い・空・負・小数・`999`・`10001`・`1e3`・繰り返し | BFF は 1000〜10000 に丸め、SSE を止めない。backend に直接不正値が来たら `INVALID_ARGUMENT` |
| `minReadIntervalMs` が要求より大きい（例：5 秒） | 読み取りも送信も 5 秒。`applied_refresh_interval_ms = 5000` で、画面に「5 秒間隔」と出す |
| 古い backend（`applied_refresh_interval_ms` を返さない） | 0 を「対応していない」として小さく知らせる。接続は今の間隔で続く |
| レートを 1→10→1 と素早く変える | 同じ `screenId` の次の `attempt` で開き直し、古い EventSource を閉じる。古い接続の遅延イベントは捨てる（#383 の契約）。ポーリングは古いタイマーを止めて張り直す |
| 同じ origin の複数タブで違うレート | `storage` イベントで同じ値にそろう。別の origin・ブラウザ・端末なら違いうる。backend は生きている要求の最小値で読み、各接続へ自分のレートで送る |
| 画面を裏に回す・スリープから戻る | hidden になったら SSE を閉じて取得を止める。visible に戻ったら復元済みのレートで開き直し、すぐ取得する。既存の `checkStale` はそのまま |
| 取得がレートより遅い | 前回の完了まで次を始めない。失敗が続いたら上限つきの backoff で間隔を空ける |
| 取得が失敗する・backend が落ちる | 直前の内容を残し、通知を出す。次の成功で戻す。初回で内容が無くてもポーリングを付ける |
| run が消える（404） | 直前の内容を残して通知を出し、その run のポーリングを止める。初回の 404 は今のとおりページの 404 |
| run 詳細の応答が毎回 `logTail` を返す | `LiveRunLog` は初期値だけに使い、積んだ行を初期化しない（採否 16） |
| アクティビティの応答が壊れた JSON | 取得失敗として扱い、直前の内容を残す。例外でページを落とさない |
| 古い画面の JavaScript（`refresh_ms` を付けない） | 今のまま動く。読み取りは設定の間隔、再送は 15 秒 |
| `intervalSeconds` が 1 秒・30 秒・大きい値 | `min` を取るので、要求が速ければ要求、遅ければ設定。`minReadIntervalMs` より短くはならない。0 は今のとおり設定エラー |
| 接続が突然切れる（タブを閉じる、VPN が切れる） | backend は `sender.closed()` で気づき、`DemandGuard` を落とす。届かない半開きは `ttl` で外れるか、同じ `screen` の次の試行が置き換える。Next 側は既存の #385 の後始末で閉じる |
| `WatchState` の初期組み立てが失敗する | エラーを送って終了し、`DemandGuard` を落とす。1 秒の要求が残らない |

## 影響

- **上流（takt、takt-mcp）**：なし。takt の読み取り回数は、画面が開いている間だけ増える。画面がないとき・hidden のときは今と同じ 30 秒。
- **保存済みデータ**：なし。SQLite の表・列・索引・保存形式を変えない。選択は `localStorage`（origin ごと）だけ。
- **公開 API**：
  - proto は `WatchStateRequest` に `refresh_interval_ms`・`screen`・`attempt` を足し、`DashboardState` に `applied_refresh_interval_ms` を足す。既存のクライアントは 0・空として扱われ、今のまま動く。
  - HTTP は `/api/state` の任意の引数と、`/api/activity`・`/api/runs/[slug]` の追加。既存の URL は変わらない。
  - CLI は変えない。
- **守りの仕組み**：CI・関門・ルールセットの定義は変えない。`make done` の対象に新しいテストが入る。`proto-check` のため生成物をコミットする。Quint の仕様はテストの担当が `specs/quint/` に足す（この設計では `.qnt` を書かない）。
- **LAN・VPN（`0.0.0.0:3000`）**：待ち受けアドレス・ポート・認証・接続元の制限を変えない。LAN・Tailscale・SoftEther VPN の中は信頼する範囲、というオーナーの決定のままにする（採否 10）。`refresh_ms` の最小は backend の `minReadIntervalMs` で決まる。新しい経路は同じ origin の HTTP だけで、ブラウザから backend へ直接つながない（今の BFF 構成のまま）。
- **負荷**：
  - `router.refresh()` をやめることで、SSE の受信ごとのページ全体の RSC 再取得（#130）が無くなる。
  - 画面が開いている間は、poller が全プロジェクトを 1 秒（または `minReadIntervalMs`）で読む。プロジェクト数が多いと takt-mcp の負荷が上がる。画面がないとき・hidden のときは 30 秒に戻る。
  - run 詳細の `GetRunDetail` は、初回だけ本体ログの末尾 1 MiB を読み、2 回目からは前回の続きだけを読む（`RunLogHandles` の読み位置）。それでも 1 秒ごとに詳細を組み立てる費用はある。負荷が高ければ、WatchState の run の要約だけを先に更新し、詳細は変化があったときだけ読む形に変える（下の「決まっていない点」）。
  - アクティビティの `GetActivity` は、毎秒、7 日分の日次集計と当日のサーバー別件数を SQLite で数える。この費用も #130 の実測に含める。
  - e2e の backend の既定は 30 秒にし、1 秒の確認は要求で切り替わる 1 本だけにする（採否 8）。

## Quint（`specs/quint/`）は要るか

**要る。ただしこの設計では `.qnt` を書かない。実装 PR のテスト担当が書く。**

- `RefreshDemand` は、`specs/quint/state-stream-relay` の `ScreenStreams` の registry と同じ形の状態機械にする。状態は「未登録」「登録済み（`screen`・`attempt`・間隔・`lastSeen`・`ttl`）」「置き換え済み」「寿命切れ」「解放済み」、遷移は「登録」「同じ `screen` の次の `attempt` で置き換え」「1 回だけの解放」「寿命で外す」「最小値の再計算」。
- 状態機械の置き場所は、`specs/quint/state-stream-relay` に `refreshDemand.qnt` を足すか、同じ形の registry を持つ `specs/quint/refresh-demand/` を新設する。どちらでも、registry の遷移と不変条件を写す。
- 確かめる性質：
  1. **解放の後に残らない**：guard を解放した要求は registry に残らない。古い guard の解放は、置き換え後の新しい要求を消さない。
  2. **最小値は生きている要求だけから決まる**：`effective` は、未登録・寿命切れ・解放済みの要求を入れず、生きている要求の最小値と `minReadIntervalMs`・`intervalSeconds` だけで決まる。
  3. **寿命で必ず外れる**：`lastSeen` から `ttl` を過ぎた要求は、次の `effective` か掃除で必ず外れ、最小値が再計算される。
  4. **置き換えは 1 画面 1 要求**：同じ `screen` の新しい `attempt` は古い要求を置き換え、registry に同じ画面の要求が 2 つ残らない。遅れて届いた小さい `attempt` は登録しない。
  5. **二重解放しない**：同じ要求の資源を 2 回解放しない（`DemandGuard` の `Drop` と番号の確認）。
  6. **適用間隔は範囲内**：`applied_refresh_interval_ms` は `max(minReadIntervalMs, 要求)` で、1000〜10000 の範囲と `minReadIntervalMs` の下限を満たす。
- 時間に依存する「レートより速く送らない」「変化を合流させる」は、Quint の時間の表現より、純粋関数の PBT（P3）と `tokio::time` の境界テストで確かめる。Quint は registry の登録・置き換え・解放・寿命・最小値に絞る。
- 既存の `state-stream-relay`（#385）と `state-stream-watchdog`（#383）の不変条件は変えない。再送間隔は最大 10 秒（要求なしは 15 秒）で、40 秒の見張りに対して `STALE_TIMEOUT > 2 * RESEND_INTERVAL` を満たす（2×10=20 秒 < 40 秒、2×15=30 秒 < 40 秒）。

## 大きさ

**13（8 を超える。3 つに分ける）**。

- 基準は #361 = 1、#362 = 5。前回の 8 に、TTL と置き換え、`minReadIntervalMs`、適用間隔の通知、hidden の停止、復元順序、共有範囲の定義、初回失敗からの回復、e2e の既定 30 秒、Quint の registry が加わった。
- 分ける案：
  - A（5）：画面のレートの選択・保存（`ready` まで接続しない）・hidden の停止、プロジェクト・アクティビティ・run 詳細の要素更新、`router.refresh()` の廃止、`measurePage("/projects")` の維持。受け入れ条件 1・2・3・5 の大半。
  - B（5）：`WatchStateRequest` の `refresh_interval_ms`・`screen`・`attempt`、`DashboardState.applied_refresh_interval_ms`、`RefreshDemand`（TTL・置き換え・最小値）、`minReadIntervalMs`、poller と WatchState の追従、Quint の registry。受け入れ条件 4。
  - C（3）：適用間隔の表示と食い違いの通知、初回失敗からの回復、e2e の既定 30 秒と切り替えの 1 本、flow-9 のログ初期化の確認、`docs/requirements.md` の更新。
- A と B は同じ画面の経路を共有する。A を先に出し、B で条件 4 を満たし、C で表示と試験を仕上げる。1 つの PR にまとめるなら 13 として扱い、レビューの単位を分ける。

## 決まっていない点

1. **1 秒の backend 負荷**：画面が開いている間、全プロジェクトを 1 秒（または `minReadIntervalMs`）で読む。プロジェクトは並列に読むが、いちばん遅いプロジェクトが周期を決めるので、応答が 1 秒を超えるとその分だけ変化の反映も遅れる。`GetActivity` の毎秒の集計（7 日分の日次集計と当日のサーバー別件数）も含めて、#130 の実測を実装の前後・1 秒と 30 秒で行う。実測で重ければ `minReadIntervalMs` を上げるか、間隔を分ける。
2. **run 詳細の 1 秒ポーリング**：`GetRunDetail` は初回だけ本体ログの末尾 1 MiB を読み、2 回目からは前回の続きだけを読む。それでも詳細の組み立てとステップの解析は 1 秒ごとに走る。WatchState の run 要約（状態・今のステップ・最後のログ時刻）で先に更新し、ステップやログの本文は変化があったときだけ読む案を、実測後に比べる。1 ページの定期的な取得は現状 2 本（詳細とログ）で、詳細を変化時だけにすると 1 本になる。
3. **1 秒ごとの全状態の再送の大きさ**：WatchState は接続ごとのレートで状態を送るので、1 秒の画面には変化がなくても毎秒 `DashboardState` 全体が届く。run やゴールが多いと帯域と JSON の組み立てが増える。変化がないときは送らない、または `readAt` だけの軽いイベントにする案を、#130 の実測と合わせて比べる。
4. **レートの共有単位**：origin ごと（`localStorage`）にした。全端末で同じ値にしたい場合は、サーバー側の設定か Cookie が要る。オーナーの意図を確認する。
5. **`applied_refresh_interval_ms` の載せ方**：`DashboardState` のフィールドにした。gRPC のヘッダに載せる案より、SSE の JSON にそのまま乗って画面とテストで扱いやすい。古い backend は 0 を返すので、画面は「対応していない」と知らせる。
6. **`minReadIntervalMs` の上限**：40 秒の無音見張りとの整合で 10 秒に抑えた。将来 10 秒より遅い間隔を許すなら、`STATE_STREAM_STALE_TIMEOUT_MS` と `state-stream-watchdog` の `STALE_TIMEOUT > 2 * RESEND_INTERVAL` を同時に上げる必要がある。

## 範囲の外

- SSE の再接続・無音の見張り（#383）と、見捨てられた接続の後始末（#385）の作り。今回は再送間隔と要求の寿命だけを足す。
- 判断待ちの回答・マージ承認・ゴール登録の書き込みの流れ。
- 認証、待ち受けアドレス（`0.0.0.0:3000`）、接続元の制限、VPN の設定（LAN・Tailscale・SoftEther VPN の中は信頼する範囲、というオーナーの決定のまま）。
- DB の表・列・索引・保存形式。
- takt と takt-mcp の変更。
- `make done` の対象外の任意の検査（`make e2e` は別途）。

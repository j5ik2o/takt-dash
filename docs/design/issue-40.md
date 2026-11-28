# issue #40 の設計：画面の情報をリフレッシュレート（1秒・2秒・5秒・10秒、既定1秒）で要素単位に自動更新する

対象の issue：[j5ik2o/takt-dash#40](https://github.com/j5ik2o/takt-dash/issues/40)
関連：[j5ik2o/takt-dash#130](https://github.com/j5ik2o/takt-dash/issues/130)（毎秒の全ページ再取得の負荷）、[j5ik2o/takt-dash#383](https://github.com/j5ik2o/takt-dash/issues/383)（SSE の無音の見張り）、[j5ik2o/takt-dash#385](https://github.com/j5ik2o/takt-dash/issues/385)（見捨てられた SSE の後始末）
大きさの基準：[j5ik2o/takt-dash#361](https://github.com/j5ik2o/takt-dash/pull/361) = 1（+23 −2、1 ファイル）、[j5ik2o/takt-dash#362](https://github.com/j5ik2o/takt-dash/pull/362) = 5（+1147 −122、18 ファイル）

確かめたコード：`110e25f`（ブランチ `j5ik2o/issue-40-design` の基点。`4fc634c` までの 2 コミットは文書だけ）。
この版は、`4fc634c` の設計を読み直した結果（20 件の指摘、増えたものの矛盾、テストにできない 11 か所、単独マージの検討）と、takt-dash中将の採否で書き直したもの。どの指摘をどこで直したかは末尾の「読み直しの指摘との対応表」にある。前の版の「採否 N」の番号は使わない。

## 先頭のまとめ

### 公開 API・保存データ・秘密・守りの仕組み

| 項目 | 触れるか | 内容 |
| --- | --- | --- |
| 公開 API（proto） | **はい** | enum `RefreshRate` を足し、`WatchStateRequest` に `refresh_rate`・`screen`・`attempt`、`DashboardState` に `applied_refresh_interval_ms` を足す。どれも省略時は 0・空で、古いクライアントと古い BFF は今のまま動く（要求なしの接続は、変化をすぐ送り、15 秒は再送だけ）。 |
| 公開 API（HTTP） | **はい** | `/api/state` に省略可能な `refresh_ms` を足す。BFF に `/api/activity` と `/api/runs/[slug]` を足す。どれも画面専用で、既存の URL と引数の意味は変えない。 |
| 保存データ | **いいえ** | DB の表・列・索引・保存形式、設定以外のファイルを変えない。画面の選択は `localStorage`（origin ごと）だけに置く。設定ファイルに任意の `minReadIntervalMs` を足す（無ければ既定 1000）。 |
| 秘密 | **いいえ** | 秘密・認証・接続元の制限を増やさない。LAN・Tailscale・SoftEther VPN の中は信頼する範囲、というオーナーの決定のまま。`refresh_ms`・`screen`・`attempt` は秘密ではない。 |
| 守りの仕組み | **一部** | CI・関門・ルールセット・`make done` の定義は変えない。この設計の PR で `specs/quint/refresh-demand/` を足し、`specs/quint/ci-specs.json` に 1 件登録するので、既存の `quint` ジョブ（`scripts/quint-check-specs.sh`）がこの仕様も検査する。実装の PR はテストを足すだけ。 |

### 大きさ

**合わせて 20。8 を超えるので 4 本に分ける（5・8・5・2）。** 分け方と、それぞれが単独でマージできる理由は「分け方」の節にある。PR 2 は 8（変えるファイルが 26 で #362 の 18 より多い）。8 を超えないので分けないが、レビューで小さくしたい場合の分け方も「分け方」に書いた。

前の版の 13 から増えたのは、読み直しで足した次の作業のため：試行番号を Provider が持つ変更とその試験、寿命切れで送信タスクを閉じる経路と時計の注入、`applied` の再計算、既存テストの書き直し（一覧の 16 件）、e2e の口（`StartStackOptions.intervalSeconds`）と対象 spec の切り替え。

## 目的

- 画面にリフレッシュレート（1秒・2秒・5秒・10秒、既定1秒）の選択を置き、再読み込み後も選択を保つ。
- すべてのページの表示中の情報を、選んだレートで自動更新する。ページ全体を再読み込みせず、情報を持つコンポーネントや要素の単位で差し替える。
- 1秒を選んだときに、takt 側の状態の変化がおおむね1秒程度で画面へ届くよう、backend の読み取りと配信の間隔も選んだレートに追従させる。

## いまの状態（`110e25f` で確かめた）

### できていること

- 概要・判断待ち・ゴール一覧・サイドバーは、`StateStreamProvider` が受ける WatchState の SSE の状態をそのまま描画に使っている（`live-overview.tsx:16-24`、`live-decisions.tsx:8-10`、`live-goals.tsx:14-16`、`live-sidebar.tsx:17-19`）。
- タスク詳細・ゴール詳細・会話・run 詳細のログは、ページを保ったまま 1 秒固定の間隔で BFF の Route Handler を読み、値だけを差し替えている（`live-task-detail.tsx:37-38`、`live-goal-detail.tsx:28-29`、`live-conversation.tsx:75-78`、`live-run-log.tsx:55-57`）。間隔は `startPolling`（`polling.ts`）の「前回の完了から次まで」。
- SSE の再接続・無音の見張り（#383）と、見捨てられた接続の後始末（#385）は入っている。

### ページ単位の再取得になっているところ

- プロジェクト・アクティビティ・run 詳細は、SSE の状態を受けるたびに `router.refresh()` を呼び、RSC のページ全体を取り直している（`state-stream.ts:71-82` の `createStateHandlers` が受信ごとに `revalidate`、`state-provider.tsx:132-164`、`state-provider.tsx:188-198` の `StateStreamProviderWithRefresh`）。
  - `projects-loader.tsx:8-20` は要求内で共有する `loadServerState()`（layout の `GetOverview` の結果）を読むだけで、自分では RPC を出さない。
  - `activity-loader.tsx:18-49` は `loadServerState()` と `GetActivity`、`run-detail-loader.tsx:29-64` は `GetRunDetail` を読む。
- そのため、状態が変わったときだけでなく backend の 15 秒の再送でも、ページ全体の RSC 再取得が走る（#130 の一部）。

### できていないこと

- リフレッシュレートの選択と保存がない。
- アクティビティと run 詳細に、要素単位のクライアント側の取得経路がない。
- backend の読み取り間隔は設定の `intervalSeconds`（既定 30 秒、`config.rs:23`、`poller.rs:196-200`）、WatchState の再送は 15 秒（`grpc/mod.rs:77-80`）で、画面の選択と結び付いていない。

## 受け入れ条件ごとの対応

| # | 受け入れ条件 | 対応 | 入る PR |
| --- | --- | --- | --- |
| 1 | 1秒・2秒・5秒・10秒から選べ、既定が1秒 | サイドバーに「更新間隔」の `select`。選択肢は proto の `RefreshRate` から作る。既定は `REFRESH_RATE_ONE_SECOND`。`localStorage` の `taktdash.refreshRateMs` に保存し、再読み込み後に復元する。 | 3 |
| 2 | どのページでも、クリックや再読み込みなしで表示中の情報が選んだレートで更新される | SSE の画面（概要・判断待ち・ゴール・サイドバー・プロジェクト）は backend が接続ごとのレートで送る。ポーリングの画面（アクティビティ・run 詳細・タスク詳細・ゴール詳細・会話・run ログ）は選んだレートで BFF を読む。 | 1・2・3 |
| 3 | ページ全体が再読み込みされない | SSE の受信ごとの `router.refresh()` をやめる。e2e で同じ document・入力値・スクロール位置を確かめる。 | 2 |
| 4 | 1秒で takt の変化がおおむね1秒で現れる | backend に画面ごとの要求の登録簿 `RefreshDemand` を置き、poller はその最小値で読み、各接続は自分の間隔で送る。下限は `minReadIntervalMs`（既定 1000）。 | 1・3 |
| 5 | `make done` が通る | 各 PR で `make done` を通す。proto の生成物は同じ PR でコミットする。#130 の実測を、その経路が入る PR ごとに受け入れに入れる（影響の「負荷」）。 | 全部 |

## 方針（決めたこと）

番号は本文と対応表から参照する（D1〜D16）。

### D1. レートは origin ごとの設定にする

`localStorage` は origin ごとなので、`localhost`・LAN の IP・tailnet のホスト名では別の値になる。同じ origin のタブは `storage` イベントで同じ値にそろう。同じ origin でも、ブラウザのプロファイルやコンテナ（Firefox の Multi-Account Containers など）が違えば別の値になる。サーバーには保存しない。この方針で決まったので、前の版の「決まっていない点 4（オーナーの意図を確かめる）」は消した。

### D2. SSE の状態を要素単位の更新の主経路にし、`router.refresh()` をやめる

概要・判断待ち・ゴール・サイドバー・プロジェクトは `StateStreamProvider` の状態から描画する。`createStateHandlers` の `revalidate` と `StateStreamProviderWithRefresh` を消す。

### D3. アクティビティと run 詳細は、RSC の初回表示＋BFF のポーリングにする

初回はサーバーで読んで内容を出し、開いている間は `/api/activity` と `/api/runs/[slug]` を読む。初回が失敗してキャッシュも無いときの表示は D13。

### D4. 要求のある接続だけが間隔で送る。要求の無い接続は今のまま

| 接続 | 送り方 | `applied_refresh_interval_ms` |
| --- | --- | --- |
| 要求なし（`refresh_rate` が 0。古い画面・古い BFF・`refresh_ms` の無い `/api/state`） | 今の `watch_state` と同じ。変化を受けたらすぐ送り、変化がなくても 15 秒ごとに再送する（`grpc/mod.rs:626-637`）。登録簿に触れない。 | 0 |
| 要求あり（1000・2000・5000・10000） | 初期状態はすぐ送る。その後は送信の期限 `next_send` まで待ち、変化は合流させ、期限に最後の状態を 1 回だけ組み立てて送る。変化がなくても期限に送る。 | その送信で使った間隔（D6） |

- 要求のある接続は、送信の間隔に 1 回までしか送らない。変化がすぐ届かない代わりに、遅れは最大でその間隔。
- 期限の計算は純粋関数 `send_schedule(now, last_sent, interval, dirty) -> SendDecision` に切り出し（P3）、`watch_state` はその結果で `sleep_until` する。
- `applied_refresh_interval_ms` は `assemble_state_for`（`grpc/assembly.rs:220-233`）では入れない。`watch_state` が組み立てた `DashboardState` に、送る直前に入れる。unary の応答は 0 のまま。

### D5. 選択肢と受ける範囲は proto の enum 1 か所に置く

前の版の `proto/refresh-interval.json` はやめる。`proto/taktdash.proto` に次を置く：

```proto
// 画面が選べる更新間隔。値はミリ秒。0 は要求なし。
enum RefreshRate {
  REFRESH_RATE_UNSPECIFIED = 0;
  REFRESH_RATE_ONE_SECOND = 1000;
  REFRESH_RATE_TWO_SECONDS = 2000;
  REFRESH_RATE_FIVE_SECONDS = 5000;
  REFRESH_RATE_TEN_SECONDS = 10000;
}
```

- frontend は `make proto-gen` が作る `frontend/web/src/proto/taktdash.ts` の `RefreshRate` を使う。`make proto-check` が生成物の食い違いを落とすので、関門に乗る。`frontend/web` の外の JSON を import しないので、`next build` の解決（`outputFileTracingRoot`）にも関わらない。
  - **確かめた**：ts-proto（`frontend/web` の固定版）で上の enum を生成すると `export enum RefreshRate { REFRESH_RATE_UNSPECIFIED = 0, REFRESH_RATE_ONE_SECOND = 1000, …, REFRESH_RATE_TEN_SECONDS = 10000, UNRECOGNIZED = -1 }` になる（作業用の proto で生成して確認）。
- backend は `backend/build.rs:32` の `tonic_prost_build::configure().compile_protos(...)` がビルド時に生成する `RefreshRate` を使う（確かめた）。
  - **仮定（PR 1 で確かめる）**：prost は前置きの `REFRESH_RATE_` を外して `RefreshRate::OneSecond` などの名前にする。
- 画面の選択肢は `Object.values(RefreshRate)` のうち 0 より大きい数値（`UNSPECIFIED` と `UNRECOGNIZED` を除く）。文言は `ms / 1000` から `1秒` などを作る。数値を TypeScript に書き写さない。
- backend が受けるのは enum の 5 つの値だけ。0 は要求なし、4 つの値は要求あり、それ以外（未知の enum 値）は `INVALID_ARGUMENT`。
- 既定の選択は UI の決まりなので frontend の `DEFAULT_REFRESH_RATE = RefreshRate.REFRESH_RATE_ONE_SECOND` の 1 か所に置く（数値ではなく名前）。
- `minReadIntervalMs` と `applied` は 4 つの選択肢と別の量（1000〜10000 の任意の整数）なので、範囲の両端を `RefreshRate` の最小・最大から作る（下の「数値の置き場所」）。

### D6. `applied_refresh_interval_ms` の意味と、下限の変更の効き方

- **値の意味**
  - `0`：その接続には要求が掛かっていない。要求なしの接続、unary の応答、この機能の無い古い backend のどれか。
  - `1000〜10000`：その接続が、この状態を送ったときに使った送信間隔。`max(要求, minReadIntervalMs)`。次の送信はこの値だけ後。
  - 15000 は使わない。要求なしの接続の 15 秒の再送は `applied` に出さない（前の版の矛盾 ⑤(a) を消す）。
- **毎回再計算する**。送るたびに `RefreshDemand` が持つ今の `minReadIntervalMs` で計算し直し、その値を `applied` に入れ、次の期限と寿命もその値から決める。接続の開始で固定しない。
- **`minReadIntervalMs` を変えたとき**：poller が設定を読み直した周期（`read_cycle_once_at`）で `RefreshDemand::set_min_read` を呼ぶ。開いている接続は、その後の最初の送信から新しい下限で送り、`applied` も変わる。効くまでの遅れは「poller の 1 周期＋その接続の前の間隔」以内。
- **起動直後と設定の失敗**：`RefreshDemand` の下限の初期値は既定の 1000。設定の読み込みに失敗した周期は、前に読めた値のまま（下げも上げもしない）。失敗した周期の待ちは `refresh_demand.effective(DEFAULT_INTERVAL_SECONDS)` を通す（要求があれば要求に従う）。
- **画面の通知**（サイドバーの「更新間隔」の下）。SSE の最初の受信の後にだけ出す。
  - `applied` が選んだ値より大きい：「状態の配信は 5 秒ごとです（backend の下限。選んだ間隔は 1 秒）。概要・判断待ち・プロジェクト・ゴール・アクティビティの内容はこの間隔で変わります。」
  - 要求を出したのに `applied` が 0：「backend が更新間隔の指定に対応していません。状態の配信は、変化のときと 15 秒ごとです。」
  - 通知は SSE の配信と poller に従う内容のことだけを言う。タスク詳細・ゴール詳細・会話・run 詳細・run ログは画面の要求のたびに takt を読むので、`minReadIntervalMs` に関わらず選んだレートで変わる（通知の対象に書かない）。

### D7. 要求の寿命（TTL）は「最後の送信から 3 × 適用間隔」

- **数値**：寿命 = 3 × `applied`。`applied` は 1〜10 秒なので、寿命は 3〜30 秒。30 秒は画面の無音の見張り（`STATE_STREAM_STALE_TIMEOUT_MS = 40_000`）より短い。
  - 理由：3 倍は、送信 2 回分の遅れ（組み立てと Next の読み遅れ）を許して 3 回目で見切る値。30 秒 < 40 秒なので、画面が見張りで開き直す前に backend の古い要求が外れ、開き直した画面の要求だけが残る。
  - `const _: () = assert!(TTL_FACTOR * MAX_REFRESH_MS < 40_000)` を `refresh.rs` に置き、frontend の `STATE_STREAM_STALE_TIMEOUT_MS > 3 × max(RefreshRate)` を frontend のテストで確かめる（数値の置き場所の表）。
- **寿命を延ばすもの**：`sender.send(message)` が完了したとき（列に入ったとき）だけ。要求のある接続の送信の列は 1 件（`WATCH_DEMAND_CHANNEL_CAPACITY = 1`）にする。最新の状態だけが意味を持つので、深い列は遅れを増やすだけだから。要求なしの接続は今の 16 件のまま。
- **寿命切れでしたこと**：登録簿から外し、**送信タスクも閉じる**（`select!` の `sleep_until(guard.deadline())` の分岐で `return`）。ストリームが終わるので、Next の中継が終わり、画面の EventSource は `onerror` から再接続し、次の試行で要求が戻る。寿命切れの後も要求なしとして送り続けて `applied` を偽る経路は無い（Quint の `expiryKeepsTask` が違反を返す）。
- **初期の組み立てとの順序**：要求は初期状態を送り終えてから登録する（`acquire` は初期送信の後）。組み立てがどれだけ長くても、最初の送信の前に寿命は切れない。その間、poller はまだ速くならないが、初期状態は組み立てた時点の最新なので困らない（Quint の `acquireEarly` が、受け付けで登録すると最初の送信の前に切れることを示す）。
- **時計の注入**：`RefreshDemand` の操作は時刻を引数で受ける（`acquire(screen, attempt, rate, now)`、`touch(&guard, now)`、`effective(config, now)`）。`watch_state` は `tokio::time::Instant::now()` を渡す。単体テストは任意の `Instant` を渡し、`watch_state` の試験は `tokio::time::pause()`・`advance()` で進める。
- **寿命で捕まえられないもの**：画面と Next の間の半開き。Next の中継は gRPC を読み続け、画面へ送れない間は最新の 1 件だけを持つ（#385 の間引き）ので、backend の送信は詰まらない。この場合は、画面が生きていれば見張り（40 秒）で開き直して置き換わり、画面が消えていれば Next の TCP が再送をあきらめたときに中継が閉じ、backend の `sender.closed()` で外れる（OS の再送の上限まで、分の単位で残りうる）。タブが hidden になれば D9 で画面が先に閉じる。残りの危険は「決まっていない点 4」。

### D8. 画面の識別子と試行番号は Provider が持ち、開き直しでも戻さない

決めたこと：**`StateStreamProvider` が `screenId`（`useRef` で 1 回だけ作る）と試行番号の数え手 `nextAttempt()`（`useRef` の数値を 1 増やして返す）を持ち、`openStateStream({ screenId, nextAttempt })` に渡す**。`openStateStream` の中の再接続も、この `nextAttempt` を呼ぶ。`createAttemptUrls`（`state-stream.ts:167-176`）は、渡されないとき（既存の呼び出しとテスト）だけ今の「1 から数える」数え手を作る。

204 で固まる経路が無いこと：

1. Next の `screen-streams.ts:31-33` と backend の `RefreshDemand` が断るのは「同じ画面にもっと大きい試行が登録済み」のときだけ。
2. `screenId` は Provider のインスタンスに 1 つで、保存しないので、別のタブや再読み込みと共有されない。
3. 同じ `screenId` の試行番号は、`nextAttempt()` だけが出し、`useRef` の値は Provider が生きている間減らないので、新しい URL の番号はそれまでに出したどの番号よりも大きい。
4. よって、画面の最新の試行より大きい番号の登録はどこにも無く、最新の試行は断られない。断られるのは、画面がもう閉じた古い試行だけ。

開き直しの経路ごと：

| 経路 | 何が起きるか | 番号 |
| --- | --- | --- |
| 中の再接続（`onerror`・見張り・`checkStale`） | `openStateStream` の中で `nextAttempt()` | 増える |
| レート変更（選択・`storage` イベント） | effect の依存（`refreshRateMs`）が変わり、古い stream を `close` して新しい `openStateStream` | 同じ `nextAttempt` なので増える |
| hidden → visible | effect の依存（`visible`）が変わる。hidden で `close`、visible で開き直す | 増える |
| React の StrictMode（開発時の effect の二重実行） | `useRef` は保たれ、effect だけが掃除と再実行をする | 増える |
| ページの移動（App Router） | layout は残るので Provider は作り直されない | 変わらない |
| 再読み込み・新しいタブ | Provider が新しく作られ、新しい `screenId` | 1 から（別の画面） |

この性質は Quint の `latestNeverRejected` と、変異 `resetAttempts`（今の `createAttemptUrls` のまま effect で開き直す版）の違反（`resetAttemptsRejectsLatestTest`）で確かめた。

### D9. 見えていないタブは取得しない

`usePageVisible()` が `document.visibilityState === "visible"` を返し、`visibilitychange` を聴く。hidden になったら SSE を閉じ、ポーリングを止める。visible に戻ったら開き直し、ポーリングはすぐ 1 回取得する。既存の `checkStale` の配線（`state-provider.tsx:151-162`）はそのまま。

### D10. レートの復元を最初の接続より先に行う

`RefreshRateProvider` の初期値は既定と `ready = false`（SSR とハイドレーションを一致させるため、描画中は `localStorage` を読まない）。`useEffect` で読み、検証してから `ready = true`。React は子の effect を親より先に実行するので、子（`StateStreamProvider` とポーリングの各コンポーネント）は `ready` を effect の条件と**依存配列**の両方に入れ、`ready` が true になった再実行で始める（読み直しの ⑬）。

### D11. ポーリングの間隔と失敗の扱い

- 間隔は今のとおり「前回の取得の完了から次まで」。重ねない。`stop`・abort の後は取得も適用もしない。
- **失敗の待ち（決めた値）**：連続 n 回目の失敗の後は `min(intervalMs × 2^(n−1), 30_000)` 待つ。ただし `intervalMs` より短くしない（10 秒の選択なら 10 秒、20 秒、30 秒、30 秒…）。成功したら `intervalMs` に戻す。30 秒は SSE の再接続の上限（`RECONNECT_MAX_DELAY_MS`）とそろえた。
- `startPolling` と `startRunLogUpdates`（`run-log-updates.ts:22-38`）に ready・visible の引数は足さない。呼ぶ側の effect が `ready && visible` のときだけ呼び、依存配列に `refreshRateMs`・`ready`・`visible` を入れ、変わったら古いハンドルを止めて張り直す。張り直しの最初の取得はすぐ。
- ポーリングの画面の遅れは「選んだ間隔＋取得の時間」。1 秒の選択でも取得が 300ms かかれば 1.3 秒周期になる。受け入れ条件 4 の「おおむね1秒」は SSE の画面の約束で、ポーリングの画面は「選んだ間隔＋取得時間」と書く（不変条件 8）。

### D12. requirements は 2.7 に足し、第 5 章は注記だけ

`docs/requirements.md:100` は第 5 章を「質疑の記録、要件の正は 2 章」と定めているので、質問 12 の回答は書き換えない。2.7 に「画面が開いて間隔を求めている間は、その間隔（`minReadIntervalMs` 以上）で読む。画面が無いときは `intervalSeconds`（既定 30 秒）で読む」を足し、質問 12 の行の末尾に「（2.7 で置き換えた。issue #40）」と注記する。PR 3 で入れる（画面が実際に要求を出すのが PR 3 からなので）。

### D13. 初回の取得が失敗したときの「空の内容」

- `LiveActivity` の初期値の型は `{ response: GetActivityResponse; readAt: Date } | undefined`、`LiveRunDetail` は `GetRunDetailResponse | undefined`。
- `undefined` のときは、内容の部品（`ActivityContent`・`RunDetailContent`）に空の値を作って渡さない。見出し（「アクティビティ」、run 詳細は slug）と `BackendUnavailableNotice` だけを出し、ポーリングを始める。最初の成功で内容を出す。`RunDetailContent` の `taskName`・`project` が `undefined` で描かれる経路（`run-detail-content.tsx:39-55`）は作らない。
- 404 は今のとおり。初回の 404 はページの 404（`notFound()`）。ポーリング中の 404 は「この run は見つかりません」を出して、その run のポーリングを止める。

### D14. 切断の通知は Live の部品が持ち、Provider の外では使わない

- `LiveProjects`・`LiveActivity`・`LiveRunDetail` は、`BackendUnavailableOver` を自分で描く。初期の失敗はローダーから props で受け、その後はポーリングの結果と `useLiveState()` の `disconnected` で変える。ローダーが外側に静的な通知を残すと、回復しても消えないため。
- `useLiveState()` が Provider の外で例外を投げる今の守り（`state-provider.tsx:34-39`）は緩めない。本番ではいつも layout の Provider の中にあり、外で黙って既定値に落ちると配線の誤りが隠れるため。ローダーを単体で描いていたテストは、本番と同じく `RootLayout` の中で描く形に直す（既存テストの表）。
- **仮定（PR 2 で確かめる）**：`await RootLayout({ children: await Loader(), params })` の要素を `renderToStaticMarkup` で描ける（今の `timing-wiring.test.ts` は `RootLayout` を呼ぶだけで描いていない）。描けない場合は、layout と同じ Provider（`StateStreamProvider` に `loadServerState()` の結果を渡す）で包むテスト用の関数を 1 つ置く。どちらでも本番の守りは緩めない。

### D15. run ログの読み位置は取り合わない

run ログの追記は `/api/runs/[slug]/updates` が画面の持つ cursor（`GetRunUpdatesRequest.epoch`・`after_sequence`、`proto/taktdash.proto:424-428`）を送って取る。読み位置は画面ごとなので、複数のタブや詳細のポーリングと取り合わない。backend の `RunLogHandles`（`run_progress.rs:643-675`）はプロジェクトごとに積み上げた集計で、`GetRunDetail` の呼び出しは同じ集計を `Mutex` で順に読むだけ。1 秒の詳細のポーリングが増やすのは、この `Mutex` の待ちと組み立ての費用（負荷の節と決まっていない点 2）。`LiveRunLog` は詳細の応答の `logTail` で積んだ行を初期化しない（`live-run-log.tsx:40-43,58-76` の今の契約）。

### D16. poller の実効間隔の式

`effective = 要求が無ければ intervalSeconds、あれば min(intervalSeconds, max(minReadIntervalMs, 生きている要求の最小値))`。

- 下限 `minReadIntervalMs` は画面の要求にだけ掛かる。運用者が `intervalSeconds` を下限より短く設定した場合は、設定のほうが速い（前の版の L300 の「設定のほうが速い（min）」は、この式で正しくなる）。
- 例：`intervalSeconds = 30`、下限 5 秒、要求 1 秒 → 5 秒。`intervalSeconds = 1`、下限 5 秒、要求 1 秒 → 1 秒。要求なし → `intervalSeconds`。
- 今の設定（`minReadIntervalMs` なし、`intervalSeconds` は 1 以上の整数秒）では、要求なしの間隔は今と同じ。
- 接続の送信間隔（`applied = max(要求, minReadIntervalMs)`）は `intervalSeconds` に左右されない。`intervalSeconds` を下限より短くした場合、poller は速く読むが、各接続へは下限の間隔で送る。

## 数値の置き場所

| 数値 | 正本 | ほかに出る場所 | 一致の確かめ方 |
| --- | --- | --- | --- |
| 選択肢 1000・2000・5000・10000 | `proto/taktdash.proto` の `RefreshRate` | `frontend/web/src/proto/taktdash.ts`（生成）、backend の prost 生成物（ビルド時） | `make proto-check`（生成物の差分）。backend は生成物そのものを使う |
| 既定の選択（1 秒） | `frontend/web/src/state/refresh-rate.ts` の `DEFAULT_REFRESH_RATE` | なし（名前で参照） | `test/state/refresh-rate.test.ts` |
| 選択肢の文言（`1秒` など） | `refreshRateLabel(ms)` が `ms / 1000` から作る | なし | 同 |
| 受ける値（0 と 4 つ） | `RefreshRate` | `refresh.rs` の `parse_refresh_rate`（生成 enum の `try_from`） | `refresh_demand_test.rs` |
| `minReadIntervalMs` の範囲 1000〜10000 | `refresh.rs` の `MIN_REFRESH_MS = RefreshRate::OneSecond as u32`、`MAX_REFRESH_MS = RefreshRate::TenSeconds as u32` | `config.rs` の検証はこの定数を使う | `config_test.rs` |
| `minReadIntervalMs` の既定 1000 | `config.rs` の `DEFAULT_MIN_READ_INTERVAL_MS = MIN_REFRESH_MS` | `taktdash.config.example.json`（例） | `config_test.rs`（例の設定を読む既存の試験に足す） |
| 寿命の倍率 3 | `refresh.rs` の `TTL_FACTOR` | Quint の `TTL_FACTOR` | `refresh.rs` の `const` assert。Quint とは README の表で対応させる（機械的にはつながらない） |
| 寿命の上限 30 秒 < 見張り 40 秒 | `refresh.rs` の `const _: () = assert!(TTL_FACTOR * MAX_REFRESH_MS < 40_000)` | frontend の `STATE_STREAM_STALE_TIMEOUT_MS = 40_000`（`state-stream.ts:52`） | frontend の `test/state/refresh-rate.test.ts` が `STATE_STREAM_STALE_TIMEOUT_MS > 3 × max(RefreshRate)` を確かめる（3 は frontend のテストに書く。backend の定数は TS から読めないため、両側で同じ式を 1 本ずつ持つ） |
| 要求なしの再送 15 秒 | `grpc/mod.rs` の `WATCH_RESEND_INTERVAL`（変えない） | Quint の watchdog の `RESEND_INTERVAL = 3`（5 秒×3） | 既存のまま |
| 要求ありの送信の列 1 件 | `grpc/mod.rs` の `WATCH_DEMAND_CHANNEL_CAPACITY` | なし | `watch_state_test.rs` |
| 失敗の待ちの上限 30 秒 | `polling.ts` が `RECONNECT_MAX_DELAY_MS` を import | `state-stream.ts` | `polling.test.ts` |
| `localStorage` のキー | `refresh-rate.ts` の `REFRESH_RATE_STORAGE_KEY` | e2e の spec（文字列） | e2e が再読み込み後の復元を確かめる |

## 変える場所（PR ごと）

### PR 1：backend の要求と proto、BFF の転送（大きさ 5）

proto

- `proto/taktdash.proto`：`RefreshRate`（D5）、`WatchStateRequest { RefreshRate refresh_rate = 1; string screen = 2; uint64 attempt = 3; }`、`DashboardState.applied_refresh_interval_ms = 9`（uint32）。
- `make proto-gen` の生成物 `frontend/web/src/proto/taktdash.ts` を同じ PR でコミットする。

backend

- 新規 `backend/src/refresh.rs`
  - `RefreshDemand`（`Arc<Mutex<Inner>>` と `tokio::sync::watch::Sender<()>`）。`Inner` は `entries: BTreeMap<ScreenId, Entry>`、`min_read: Duration`、`next_id: u64`。`Entry { id: DemandId, attempt: u64, rate: Duration, deadline: Instant, superseded: watch::Sender<bool> }`。
  - `ScreenId` は 32 文字の 16 進を検証した newtype。`Attempt` は 1 以上。
  - `acquire(screen, attempt, rate, now) -> Result<DemandGuard, Stale>`：同じ画面にもっと大きい試行があれば `Stale`。それ以外は登録を置き換え、古い登録の `superseded` に通知する。
  - `touch(&guard, applied, now)`：自分が持ち主なら `deadline = now + TTL_FACTOR × applied`。
  - `applied_for(&guard) -> Duration`：`max(rate, min_read)`（D6）。
  - `set_min_read(Duration)`：poller から。
  - `effective(config: Duration, now) -> Duration`：D16。期限切れの登録は数えない。
  - `subscribe() -> watch::Receiver<()>`：登録・解放・下限の変更で通知する。
  - `DemandGuard`：`Drop` で、登録がまだ自分の `DemandId` を指すときだけ外し、通知する。`deadline()` と `superseded()` を持つ。
  - `TTL_FACTOR = 3`、`MIN_REFRESH_MS`、`MAX_REFRESH_MS`、`const` assert（数値の置き場所）。
- `backend/src/lib.rs`：`mod refresh;`。
- `backend/src/grpc/mod.rs`
  - `TaktDashServiceImpl` と `ServiceDeps` に `refresh_demand: RefreshDemand`。
  - `watch_state`：`refresh_rate` を検証する（未知の値は `INVALID_ARGUMENT`）。`refresh_rate` が 0 なら今の処理のまま（D4）。0 でなければ `screen` は 32 文字の 16 進、`attempt` は 1 以上でなければ `INVALID_ARGUMENT`。
  - 要求のある接続：購読 → 初期の組み立て → `applied` を入れて送信 → `acquire`（`Stale` なら `Status::aborted` を送って終わる）→ 送信タスクへ guard を移す。送信タスクの `select!` は `sender.closed()`・停止・購読の終端・`guard.superseded()`・`sleep_until(guard.deadline())`（寿命切れ）・`sleep_until(next_send)`・変化の受信（dirty にするだけ）。送信は `sender.send` を寿命の期限・切断・停止と `select!` し、完了で `touch`。
  - 列の大きさは要求ありで 1（D7）、要求なしで今の 16。
- `backend/src/poller.rs`：`Poller` に `refresh_demand`。`with_refresh_demand` で共有する（テストで指定しなければ要求なしの `RefreshDemand`）。`read_cycle_once_at` で `refresh_demand.set_min_read(config.min_read_interval)` を呼び、`CycleReport.interval = refresh_demand.effective(Duration::from_secs(config.interval_seconds), now)`。
- `backend/src/poller_stop.rs`：`run_until_stopped` の待ちを、`sleep_until(started + effective)` と `refresh_demand.subscribe().changed()` の `select!` にする。通知を受けたら今の `effective` で期限を計算し直す（短くなれば切り上げ、長くなれば待ち直す）。設定の失敗の周期は D6。停止と `CYCLE_GRACE` は変えない。
- `backend/src/main.rs`：`RefreshDemand::new()` を 1 つ作り、`ServiceDeps` と `Poller` へ同じものを渡す。
- `backend/src/config.rs`：`minReadIntervalMs`（既定 `DEFAULT_MIN_READ_INTERVAL_MS`、範囲は `MIN_REFRESH_MS..=MAX_REFRESH_MS`、外れは `ConfigError::Invalid`）。`DEFAULT_INTERVAL_SECONDS = 30` は変えない。`taktdash.config.example.json` に `"minReadIntervalMs": 1000`。

BFF

- `frontend/web/src/grpc/client.ts`：`watchState(request?: { refreshRate?: RefreshRate; screen?: string; attempt?: number })`。省略時は 0・空。
- `frontend/web/app/api/state/route.ts`：`refresh_ms` を `parseRefreshRateParam`（`src/state/refresh-rate.ts`）で読む。無い・空は `REFRESH_RATE_UNSPECIFIED`。`RefreshRate` の 4 つの値のどれかならその値。それ以外（`999`・`1e3`・負・壊れた値）は既定の 1 秒にし、ストリームを止めない。要求あり（0 でない）で `parseStreamTicket` が無いときは要求なしにする（backend の `INVALID_ARGUMENT` を作らない）。`screen`・`attempt` は ticket から渡す。`screenStreams` の扱いは変えない。
- `frontend/web/src/state/refresh-rate.ts`（新規、この PR では BFF の解析と選択肢だけ）：`REFRESH_RATE_OPTIONS`（生成 enum から）、`DEFAULT_REFRESH_RATE`、`parseRefreshRateParam`。

この PR の後も画面は `refresh_ms` を付けないので、どの接続も要求なしで今と同じに動く。

### PR 2：ページ単位の再取得をやめる（大きさ 8）

- `state-stream.ts`：`createStateHandlers` から `revalidate` を消す。
- `state-provider.tsx`：`revalidate` prop と `useRouter`、`StateStreamProviderWithRefresh` を消す。`layout.tsx` は `StateStreamProvider` を直接使う。
- 新規 `live-projects.tsx`：`useLiveState()` の `state`・`snapshotLoaded`・`disconnected`・`failure` から `ProjectsContent` と `BackendUnavailableOver` を描く。時計（待ち時間の表示）は `state.readAt ?? new Date()` を起点に 1 秒で進める。取得はしない。
- `projects-loader.tsx`：`measurePage("/projects", async () => <LiveProjects />)`。`loadServerState()` は呼ばない（Provider が layout の結果を持つ）。RPC は出さないので `rpcs=` のまま。
- 新規 `app/api/activity/route.ts`：`getBackendClient().getActivity()` を `GetActivityResponse.toJSON` で `Cache-Control: no-store`。失敗は既存の `toBackendUnavailableResponse`。成功を `lastActivity` に覚える。
- 新規 `app/api/runs/[slug]/route.ts`：`getRunDetail(slug)` を JSON で返す。`RunNotFoundError` は 404、`BackendUnavailableError` は 503。成功を `lastRunDetails` に覚える。
- `src/state/last-loaded-caches.ts`（新規）：`lastActivity` と `lastRunDetails` の実体を置く（今は `activity-loader.tsx:11` と `run-detail-loader.tsx:27` の module 変数）。Route Handler とローダーが同じものを使う。作り方は既存の `createLastLoaded`。
- 新規 `live-activity.tsx`：初期値（D13）と初期の失敗を受け、`useLiveState()` の `state`（サーバー名の解決）と合わせて描く。`/api/activity` を 1 秒で読む（この PR ではタスク詳細などと同じ固定 1 秒。PR 3 で選んだレートにする）。
- 新規 `live-run-detail.tsx`：初期値（D13）と初期の失敗を受け、`/api/runs/${slug}` を 1 秒で読む。`serverName` は `useLiveState()` の `state.servers` から引く。`LiveRunLog` は今と同じ `key={detail.runSlug}`。
- `activity-loader.tsx`：`GetActivity` だけを読み、`<LiveActivity initial=… initialFailure=… />` を返す。`loadServerState()` は呼ばない。
- `run-detail-loader.tsx`：`GetRunDetail` を読み、`<LiveRunDetail initial=… initialFailure=… serverName=… />` を返す。404 の `notFound()` は残す。
- `src/state/restore-detail.ts`：`restoreActivityResponse`、`restoreRunDetail` を足す（既存の `restoreGoalDetail` と同じ作り。ブラウザで `@grpc/grpc-js` を読み込まないよう生成コードの `fromJSON` は使わない）。

### PR 3：画面のレートの選択と、レートに従う取得（大きさ 5）

- `src/state/refresh-rate.ts`：`parseStoredRefreshRate(value: unknown): RefreshRate`（4 つのどれかでなければ既定、例外を投げない）、`refreshRateLabel`、`REFRESH_RATE_STORAGE_KEY`。
- 新規 `components/refresh-rate-provider.tsx`：`RefreshRateProvider`・`useRefreshRate()`（`refreshRate`・`ready`・`setRefreshRate`）。D10。`setRefreshRate` は state を変え、`localStorage.setItem` を試す（`SecurityError`・容量不足でも state は変える）。`storage` イベントで同じ origin の別のタブの変更を受ける。
- 新規 `components/page-visibility.ts`：`usePageVisible()`（D9）。
- 新規 `components/refresh-rate-select.tsx`：`<label for="refresh-rate">更新間隔</label>` と `<select id="refresh-rate">`。
- `app/layout.tsx`：`RefreshRateProvider` で `StateStreamProvider` を包む。
- `state-provider.tsx`：`screenId` と `nextAttempt`（D8）。`ready && visible` のときだけ `openStateStream({ url: "/api/state?refresh_ms=" + refreshRate, screenId, nextAttempt, … })`。依存は `[refreshRate, ready, visible]`。Context に `liveStateReceived`（SSE で一度でも状態を受けたか）を足す。
- `state-stream.ts`：`OpenStateStreamOptions` に `nextAttempt?: () => number`。渡されたらそれで URL を作る（`screenId` は既にある）。
- `restore-state.ts`：`appliedRefreshIntervalMs` を読む。
- `sidebar.tsx`・`live-sidebar.tsx`：「更新間隔」の節と D6 の通知。`refreshRate?`・`appliedRefreshIntervalMs?`・`liveStateReceived?` は任意の prop で、渡さない既存のテストは今のまま。
- `polling.ts`：D11 の失敗の待ち。
- `live-task-detail.tsx`・`live-goal-detail.tsx`・`live-conversation.tsx`・`live-run-log.tsx`・`live-activity.tsx`・`live-run-detail.tsx`：`intervalMs = refreshRate`、effect の条件と依存に `ready`・`visible`（D10・D11）。会話の `setInterval` は `startPolling` に置き換える。
- `docs/requirements.md`：D12。
- 新規 e2e `e2e/tests/issue-40-refresh-rate.spec.ts`（E13〜E17）。この PR の時点では e2e の backend は 1 秒のまま。

### PR 4：e2e の既定を 30 秒にする（大きさ 2）

- `e2e/support/stack.ts`：`StartStackOptions` に `intervalSeconds?: number`（既定 30）と `minReadIntervalMs?: number`（既定なし）を足し、`buildConfig` へ渡す。`stack.ts:308` の `intervalSeconds: 1` を `input.intervalSeconds` にする。
- `startStack` を直接呼ぶ 5 本（`decisions-layout`・`issue-311-confirm-dialogs`・`issue-315-narrow`・`overview-responsive`・`screenshot-states`）は、新しい口を渡さず既定を使う。
- `e2e/support/test.ts`：名前付きの stack（13・51・71・82 行の `twoProjectStack`・`longServerStack`・`longTitleStack`・`conversationStack`、103 行の `stack`、115 行の `isolatedStack`）は既定（30 秒）のまま。新しい fixture `fastPollIsolatedStack`（`startStack({ intervalSeconds: 1 })`）を足す。
- 1 秒が要る spec を `fastPollIsolatedStack` へ移す（下の一覧）。
- `issue-40-refresh-rate.spec.ts` に E18（既定 30 秒のまま、画面の 1 秒の要求で 3 秒以内に変化が出る）を足す。

## 状態と遷移

### 画面のレート

| 状態 | 値 | 遷移 |
| --- | --- | --- |
| 初期（SSR） | 既定、`ready = false` | 保存値を読まない。SSE もポーリングも始めない |
| 復元 | 4 つのどれか | effect で読み、検証し、`ready = true`。ここから最初の接続 |
| 利用者の選択 | 4 つのどれか | state と保存。SSE は同じ `screenId` の次の試行で開き直す。ポーリングは張り直す |
| 別のタブの選択（同じ origin） | 4 つのどれか | `storage` イベントで同じ値へ。開き直しは選択と同じ |
| hidden | 値は保つ | SSE を閉じ、ポーリングを止める |
| visible に戻る | 値は保つ | 次の試行で開き直し、ポーリングはすぐ取得 |
| 保存できない | 選んだ値（メモリだけ） | 例外を握りつぶして動く |

### backend の要求と読み取り

```
画面（ready かつ visible）
  → /api/state?refresh_ms=1000&screen=…&attempt=n（n は Provider の数え手）
  → Next の screen-streams（同じ画面の古い試行を閉じる）
  → WatchStateRequest { refresh_rate, screen, attempt }
  → watch_state：初期状態を送る → RefreshDemand.acquire（同じ画面の古い要求を置き換える）
  → 送信タスク：間隔ごとに最後の状態を送る（applied を入れる）→ touch（寿命を延ばす）
  → poller：effective = min(intervalSeconds, max(minReadIntervalMs, 要求の最小値))
  → 変化通知 → 送信タスクは dirty にするだけ
```

| 登録の状態 | 入る | 出る |
| --- | --- | --- |
| 無い | — | 初期送信の後の `acquire` で「ある」へ |
| ある（`deadline` を持つ） | `acquire`、`touch` で `deadline` を延ばす | 接続の終わり（guard の `Drop`）、同じ画面の新しい試行による置き換え、寿命切れ（送信タスクも閉じる） |

## 守る不変条件

1. **レートは 4 つのどれか**：画面の `refreshRate` は `RefreshRate` の 4 つのどれか。保存値が壊れていれば既定。backend が受けるのは 0 と 4 つだけ。
2. **既定は 1 秒、再読み込み後も保つ**：保存値が無ければ既定。保存できた値は次の読み込みで戻る。復元は最初の接続より先。
3. **データ更新でページ全体を再取得しない**：SSE の受信で `router.refresh()` を呼ばない。ポーリングは対象の BFF だけを読む。
4. **要求のある接続は、その間隔に 1 回までしか送らない**：初期状態の後は、前の送信から `applied` が過ぎるまで送らない。変化は合流させ、最後の状態を送る。
5. **要求の無い接続は今のまま**：変化をすぐ送り、変化が無くても 15 秒で再送する。`applied` は 0。
6. **poller は生きている要求の最小値で読む**：D16 の式。期限切れ・解放済み・置き換え済みの要求は入れない。要求が変わったら、残りの待ちを計算し直す。
7. **要求は残らない**：接続の終わり・置き換え・寿命切れで外れる。寿命切れでは送信タスクも閉じる。古い guard の解放は新しい要求を消さない。画面の最新の試行は断られない。
8. **`applied` は実際の送信間隔**：要求のある接続が送る `applied` は、その送信で使った `max(要求, 今の下限)`。下限を変えたら、次の送信から新しい値。0 は要求なし。
9. **1 秒の遅れの約束**：SSE の画面は「poller の 1 周期＋組み立て・送信・描画」で、LAN で 1〜2 秒を目標にし、e2e は 3 秒以内で確かめる。ポーリングの画面は「選んだ間隔＋取得時間」。読み取りに 1 秒より長くかかるプロジェクトがあれば、その分だけ遅れる。
10. **取得は重ならず、失敗したら間隔を空ける**：D11。
11. **失敗しても内容を消さない**：直前の内容を残して通知を出し、次の成功で戻す（#309）。初回で内容が無ければ D13。
12. **見えていないタブは取得しない**：D9。
13. **LAN・VPN の使い方を壊さない**：Next の待ち受け（`0.0.0.0:3000`）・gRPC の待ち受け・認証・接続元の制限を変えない。新しい経路は同じ origin の HTTP だけで、ブラウザから backend へ直接つながない。
14. **保存データを変えない**：DB と保存形式を変えない。選択は `localStorage` だけ。

## テストにできる性質

| ID | 性質 | 確かめ方 | 置き場所 | PR |
| --- | --- | --- | --- | --- |
| E1 | 選択肢は `RefreshRate` の 4 つで、既定は 1 秒。保存値の検証。見張り 40 秒 > 3 × 最大の選択肢 | 例 | `test/state/refresh-rate.test.ts` | 1・3 |
| E2 | 復元が最初の接続より先（`ready` の前に EventSource もポーリングも作らない）。保存できない環境でも動く。`storage` イベント | 例 | `test/components/refresh-rate-provider.test.ts` | 3 |
| E3 | `select` の選択肢が 4 つ、既定が `1秒`、変更が state と保存へ | 例 | 同 | 3 |
| E4 | レート変更・hidden→visible・中の再接続・StrictMode の二重実行のどれでも、EventSource は 1 本だけで、同じ `screenId` の試行番号が増え続ける | 例 | `test/components/live-provider-wiring.test.ts` | 3 |
| E5 | 状態を受けても再取得を促さない | 例 | `test/components/state-revalidation.test.ts`（置き換え） | 2 |
| E6 | プロジェクトは Provider の状態だけで更新される | 例 | `test/components/live-projects.test.ts` | 2 |
| E7 | アクティビティは `/api/activity` だけを読む。初回失敗でも見出しと通知を出してポーリングし、成功で内容を出す | 例 | `test/components/live-activity.test.ts`、`test/api/activity-route.test.ts` | 2 |
| E8 | run 詳細は `/api/runs/[slug]` だけを読む。ログを初期化しない。ポーリング中の 404 で止まる | 例 | `test/components/live-run-detail.test.ts`、`test/api/run-route.test.ts` | 2 |
| E9 | 前回の完了から次まで。失敗の待ちの列（D11）。abort 後 0 回 | 例 | `test/components/polling.test.ts` | 3 |
| E10 | `RefreshDemand`：最小値、置き換え、古い試行の拒否、解放、古い guard の解放が新しい要求を消さない、寿命切れ（注入した `Instant`）、下限 | 例 | `backend/tests/refresh_demand_test.rs` | 1 |
| E11 | 1 秒の要求で、変化がなくても 1 秒ごとに送られ、`applied = 1000`。初期状態にも入る | 例（同じプロセスで `service.watch_state` を呼び、`tokio::time::pause`。`watch_state_test.rs:300-330` と同じ形） | `backend/tests/watch_state_test.rs` | 1 |
| E12 | 要求なし（`WatchStateRequest::default()`）は今のまま：変化ですぐ送り、15 秒の再送、`applied = 0` | 例 | 同 | 1 |
| E13 | レートの選択が再読み込み後も残る | e2e | `e2e/tests/issue-40-refresh-rate.spec.ts` | 3 |
| E14 | 全ページで、状態の変更が再読み込みなしで現れる（同じ document の印） | e2e | 同、既存の flow | 2・3 |
| E15 | 入力値とスクロール位置が状態の更新の後も残る | e2e | 同 | 2 |
| E16 | 1 秒のとき、SSE の画面の変更が 3 秒以内に現れる | e2e | 同 | 3 |
| E17 | 5 秒にすると、ポーリングの要求の間隔が 5 秒になる（要求の時刻を数える） | e2e | 同 | 3 |
| E18 | e2e の backend の既定 30 秒のまま、画面の 1 秒の要求で 3 秒以内に変化が出る | e2e | 同 | 4 |
| E19 | hidden では取得せず、visible ですぐ取得する | 例（`visibilityState` を差し替え） | `test/components/page-visibility.test.ts` | 3 |
| E20 | 通知：`applied` > 選択で「backend の下限」、要求したのに 0 で「対応していない」。初回の RSC の状態では出さない | 例 | `test/components/live-sidebar.test.ts` | 3 |
| E21 | `applied` の真実性：`minReadIntervalMs = 3000` の設定で、1 秒を要求した接続の連続する送信の間隔が 3 秒で、各状態の `applied = 3000`。設定を書き換えて次の周期の後、`applied` と間隔が新しい値に変わる | 例（同じプロセスで Poller と `service.watch_state`、`pause`） | `backend/tests/watch_state_test.rs` | 1 |
| E22 | 寿命切れで送信タスクが閉じる：要求ありの接続でストリームを読まずに持ち続けると、列 1 件が埋まって送信が待ちに入り、3 × `applied` の後にストリームが終わり、登録が外れる | 例（同じプロセスで `service.watch_state` を呼んで読まない。`watch_state_test.rs:300-330` の形。h2 の窓は通らないので、窓が埋まるまでの遅れは確かめない） | 同 | 1 |
| E23 | `refresh_ms` の解析：無い・空は要求なし、4 つはそのまま、`999`・`10001`・`1e3`・負・文字は既定。ticket が無ければ要求なし。`screen`・`attempt` が渡る | 例 | `test/api/state-route.test.ts` | 1 |
| E24 | poller の待ち：30 秒の待ちの途中で 1 秒の要求が来たら切り上げて読み、要求が消えたら 30 秒で待ち直す（`pollerNotLate` に対応） | 例（`pause`） | `backend/tests/poller_test.rs` | 1 |
| E25 | 置き換え：同じ画面の次の試行が登録されたら、古いストリームが終わる（`guard.superseded()`）。古い試行が後から来たら `ABORTED` で終わり、登録は新しいまま | 例 | `backend/tests/watch_state_test.rs` | 1 |
| E26 | 初期の組み立てが寿命より長くても（組み立てを止めて時刻を進める）、最初の送信の前に寿命は切れず、送った後に登録される（`noExpiryBeforeFirstSend`） | 例（`pause`） | 同 | 1 |
| E27 | 設定の読み込みに失敗した周期は、前に読めた下限のままで、待ちは `effective` を通る（読み直しの ⑭） | 例 | `backend/tests/poller_test.rs` | 1 |
| E28 | `minReadIntervalMs` の既定 1000、範囲 1000〜10000、外れは `ConfigError::Invalid`、例の設定を読める | 例 | `backend/tests/config_test.rs` | 1 |
| P1 | `parseStoredRefreshRate` と `parseRefreshRateParam` はどんな入力でも例外を投げず、`RefreshRate` の値を返す | PBT | `test/state/refresh-rate.test.ts` | 1・3 |
| P2 | 任意の acquire・drop・置き換え・時刻の列で、`effective` は D16 の式で、登録を置き換えるのは同じ画面のもっと新しい試行だけ、古い guard の drop は新しい登録を消さない、期限切れは数えない | PBT | `backend/tests/refresh_demand_test.rs` | 1 |
| P3 | 任意の変化の到着と時刻の列で、`send_schedule` は間隔より速く送らず、最後の変化を落とさない | PBT | `backend/src/grpc` の `send_schedule` | 1 |
| P4 | 任意の取得完了・失敗・abort・レート変更の列で、取得は重ならず、待ちは D11 の列で、abort 後 0 回 | PBT | `test/components/polling.test.ts` | 3 |
| P5 | 任意のベース URL に `refresh_ms` と ticket を足しても両方が残る | PBT | `test/support/stream-url.ts` の拡張 | 3 |
| P6 | 任意の開き直しの列（レート変更・hidden・visible・中の再接続）で、`nextAttempt` が出す番号は狭義に増える | PBT | `test/components/state-stream.test.ts` | 3 |

PBT は既存の frontend と同じく、シード固定の PRNG で操作列を作る。backend は `proptest`（既存の使い方に合わせる）。

### テストにできない・しにくいところ（読み直しの 11 か所）

| # | 箇所 | どう扱うか |
| --- | --- | --- |
| 1 | TTL の「数倍」 | 3 倍に決めた（D7）。時刻を引数で受けるので E10・P2 で確かめる |
| 2 | 寿命切れで送信タスクを閉じるか | E22 で、同じプロセスの呼び出しを読まずに列を埋めて確かめる。h2 の窓が埋まるまでの遅れ（実際のネットワーク越し）はテストしない。窓は送信の待ちを遅らせるだけで、待ちに入った後の寿命切れの扱いは同じ |
| 3 | `applied` の真実性 | E21（設定 → 実際の送信間隔 → `applied`）と E20（`applied` → 通知）。両者をつなぐのは `DashboardState` の値だけなので、間は型で保つ |
| 4 | 15 秒と `applied` の矛盾 | 要求なしは 0 と決めて消えた（D6）。E12 |
| 5 | frontend と Rust の定数の一致を 1 本で | 数値は proto だけに置き、両側は生成物を使う（D5）。`make proto-check` が生成物を守る。40 秒との関係だけは両側に 1 本ずつ（数値の置き場所） |
| 6 | 正本 JSON の実行時の解決 | JSON をやめたので無くなった |
| 7 | origin ごとの `localStorage` | Playwright の 1 つのコンテキストでは別 origin を作れない。単体テスト（E2）で「キーは origin に依存しない 1 つ」を確かめ、origin ごとに分かれるのはブラウザの仕様として扱う。e2e では同じ origin の再読み込み（E13）だけ |
| 8 | hidden の実際の挙動（タイマーの抑制、EventSource の停止） | 単体テスト（E19）で `visibilitychange` への反応を確かめる。実ブラウザのタイマーの抑制はテストしない。抑制されても、visible に戻った時点で開き直してすぐ取得するので、遅れは戻った時点で消える |
| 9 | 1 秒で 1 秒 | 約束を書き分けた（不変条件 9）。e2e は SSE の画面を 3 秒以内、ポーリングの画面は要求の間隔（E17）で見る。#130 の実測（PR 1〜3）で LAN の値を記録する |
| 10 | 試行番号の単調性 | D8 の決定と Quint（`latestNeverRejected`、`resetAttemptsRejectsLatestTest`）、E4、P6 |
| 11 | 既存テストの更新 | 下の表 |

## 既存テストで変わるもの

弱めずに直す。「変えない」ものは理由を書く。

| テスト | 今の期待 | 直し方 | PR |
| --- | --- | --- | --- |
| `backend/tests/watch_state_test.rs` の `WatchStateRequest {}`（58・99・130・163・217・260・311・347・376 行ほか） | 要求なしの購読 | フィールドが増えると構造体の literal は通らないので `WatchStateRequest::default()` にする。意味は同じ（要求なし）。期待は変えない | 1 |
| `test/api/state-route.test.ts` | `watchState()` を引数なしで呼ぶ | 引数なしの `/api/state` で `watchState` に `{ refreshRate: 0 }`（要求なし）が渡ることを期待に足す。既存の付け替え・204 の期待は変えない | 1 |
| `test/components/state-revalidation.test.ts:51`「受信のたびに再取得を促す」 | `revalidate` が受信ごとに呼ばれる | 「受信しても再取得を促さず、状態を適用する」に置き換える（E5）。受信が状態に反映されることの確かめは残す | 2 |
| 同 `:73`「切断時は再取得を促さず再接続する」 | 切断で `revalidate` なし | `revalidate` の口が無くなるので、再接続の確かめだけを残す | 2 |
| `test/components/live-provider-wiring.test.ts:730-757` | 受信で `revalidate` が 1 回 | 受信で Context の状態が変わり、`router` に触れない、に置き換える | 2 |
| 同 `:802-830` | 再描画で effect が再実行されない（`revalidate` の同一性） | 「依存（`refreshRate`・`ready`・`visible`）が変わらない再描画では開き直さない」に置き換える（PR 2 では依存が空なので「再描画で開き直さない」） | 2・3 |
| 同 `:835-` | `revalidate` が変わると 1 本だけ開き直す | 「レートが変わると 1 本だけ開き直し、試行番号が増える」に置き換える（E4） | 3 |
| `test/components/state-stream.test.ts` | `createStateHandlers` に `revalidate` | `revalidate` を外す。再接続・見張り・`checkStale`・試行の URL の期待は変えない。`nextAttempt` を渡したときの番号を足す（P6） | 2・3 |
| `test/pages/state-sharing.test.ts:41` | `ProjectsLoader` 単体で `["GetOverview"]` | ローダーは RPC を出さないので `[]`（GetProjects も GetOverview も呼ばない）に。内容（`corporate-site`・`run-aaa`・`判断待ち`）は `RootLayout` の中で描いて確かめる（layout の 1 回の `GetOverview` で描けることの確かめになる） | 2 |
| 同 `:60` | `ActivityLoader` 単体で `["GetOverview","GetActivity"]` | `["GetActivity"]` に。サーバー名（`Mac mini`）は `RootLayout` の中で描いて確かめる | 2 |
| 同（同じクライアントの再利用） | `ProjectsLoader` を 2 回で `["GetOverview","GetOverview"]` | RPC を出す `ActivityLoader` を 2 回呼び、`["GetActivity","GetActivity"]` と同じクライアントを確かめる（確かめる性質は同じ） | 2 |
| `test/pages/disconnected-keeps-content.test.ts:83-97`（プロジェクト・アクティビティ・run 詳細の前の内容を残す） | ローダー単体の HTML | 描き方を `RootLayout` の中に直す（D14）。期待（バナー・見出し・前の内容・バナーが上）は変えない | 2 |
| 同 `:222-246`（切断中は読めた時刻を出す） | ローダー単体 | `RootLayout` の中で描く。期待は変えない（`LiveActivity` は初期値の `readAt` を出す） | 2 |
| 同 `:248-273`（run 詳細は接続中と切断中で内容を同じ位置に同じ型で） | ローダーの子が `RunDetailContent` | ローダーの返す要素がどちらも `LiveRunDetail` であること、`LiveRunDetail` を描いた HTML で内容の区画が同じ位置にあることの 2 つに分ける（React が状態を保つ条件は「同じ位置に同じ型」なので、確かめる性質は同じ） | 2 |
| `test/pages/timing-wiring.test.ts:62-68` | `/projects` は `rpcs=`、`/activity` は `rpcs=GetActivity` | 変えない。ローダーに `measurePage` を残し、`/activity` は `GetActivity` だけを読むので同じ行が出る | — |
| `test/components/polling.test.ts` | 固定間隔、失敗でも同じ間隔 | 失敗の待ちの期待を D11 に直す。重ならない・abort の期待は変えない | 3 |
| `test/components/live-sidebar.test.ts`・`sidebar.test.ts` | 更新間隔の節なし | 任意 prop なので既存の期待は変えない。E3・E20 を足す | 3 |
| `test/components/run-log-updates.test.ts` | — | 変えない（`startRunLogUpdates` の口を変えないため。D11） | — |
| `e2e/tests/flow-9-run-detail-live-log.spec.ts:78` | `rsc: 1` の応答（`router.refresh`）を待つ | `/api/runs/[slug]` の応答を待つ形にし、毎回の応答でログの積んだ行が残ることを確かめる | 2 |
| `e2e/tests/issue-430-error-reload.spec.ts` | 「状態ストリームの受信で `router.refresh` が走る」前提のコメント | コメントを直す。ボタンの再取得の確かめはそのまま | 2 |
| `e2e/support/stack.ts:308` | `intervalSeconds: 1` 固定 | PR 4 で口にする | 4 |

## e2e の既定を 30 秒にする（PR 4）

- 判断の基準：**画面が要求を出していない間（ページを開く前、SSE が切れている間）に、poller の読み取りで起きる変化を待つ spec は 1 秒が要る**。ページを開いて待つ spec は、画面の 1 秒の要求で poller が 1 秒になるので 30 秒の既定でよい。
- 静的に読んで決めた一覧（PR 4 で既定 30 秒の `make e2e` を流して確かめる）：

| spec | 理由 | 扱い |
| --- | --- | --- |
| `issue-317-layout.spec.ts:92-135`（1280px の種類の列） | `goto` の前に状態ファイルへ通知を足し、アクティビティに poller が記録した出来事を待つ | `fastPollIsolatedStack` |
| `issue-329-kind-column.spec.ts:16-59` | 同じ（`goto` の前に状態を書く） | `fastPollIsolatedStack` |
| `flow-6-unavailable.spec.ts` | 1 本目はページを開いたまま mock を止める（要求あり）。2 本目は backend を止める（poller と無関係） | 既定のまま。流して確かめる |
| `flow-7-delay-recovery.spec.ts` | ページを開いたまま遅延を入れる（要求あり） | 既定のまま。流して確かめる |
| `issue-307-disconnected-decisions.spec.ts` | 切断中は SSE が無く要求も無い。回復の判定は画面の再接続に依存 | 既定のまま。流して確かめる（落ちたら `fastPollIsolatedStack`） |
| `issue-383-half-open-sse.spec.ts`・`issue-385-abandoned-sse.spec.ts` | SSE の切断と再接続。要求ありの接続は 1 秒で送るので、無音の時間は今より短い | 既定のまま。流して確かめる |
| `decisions-layout`・`issue-311-confirm-dialogs`・`issue-315-narrow`・`overview-responsive`・`screenshot-states` | `startStack` を直接呼ぶ。状態は `mockState` で起動前に置き、起動直後の 1 回目の読み取りで足りる | `startStack` の既定（30 秒）のまま。流して確かめる |
| ほかの 16 本 | ページを開いてから待つか、状態を変えない | 既定のまま |

- 流して落ちた spec は、機能の誤り（画面の要求で poller が速くならない）か、要求の無い間の読み取りに頼る spec かを分け、後者だけを `fastPollIsolatedStack` へ移し、理由をコメントに書く。

## 壊しうる入力

| 入力・状況 | 期待する振る舞い |
| --- | --- |
| `localStorage` が無い・読めない・書けない | 例外を握りつぶし、メモリの値で動く |
| 保存値が `""`・`"0"`・`"1001"`・`"1e3"`・`"NaN"`・`"１０秒"`・巨大な文字列 | 既定に戻す。例外を投げない |
| `refresh_ms` が無い・空・負・小数・`999`・`10001`・`1e3`・繰り返し | 無い・空は要求なし、ほかは既定。SSE を止めない（E23） |
| backend に未知の `refresh_rate`・不正な `screen`・0 の `attempt` | `INVALID_ARGUMENT`。BFF からは作らない |
| `minReadIntervalMs` が要求より大きい（例：5 秒） | 送信と poller は 5 秒、`applied = 5000`、画面に D6 の通知 |
| 運用中に `minReadIntervalMs` を変える | 次の poller の周期の後の最初の送信から新しい値（D6、E21） |
| 古い backend（`applied` を返さない） | 最初の SSE の受信の後に「対応していません」と知らせる。接続は今の送り方で続く |
| 古い画面の JavaScript（`refresh_ms` を付けない） | 要求なしで今のまま |
| レートを 1→10→1 と素早く変える | そのたびに次の試行で開き直し、古い接続は Next と backend で置き換わる。断られるのは古い試行だけ（D8） |
| 同じ origin の複数タブ | `storage` で同じ値にそろう。backend は画面ごとに要求を持ち、最小値で読む |
| 送信が詰まる | 列 1 件と h2 の窓が埋まってから 3 × `applied` で寿命が切れ、送信タスクが閉じ、画面は再接続する（D7） |
| 初期の組み立てが遅い | 要求は初期送信の後に登録するので寿命は切れない（D7） |
| タブを閉じる・VPN が切れる | 閉じれば `sender.closed()` で外れる。届かない半開きは D7 の「寿命で捕まえられないもの」 |
| 取得がレートより遅い・失敗が続く | D11 |
| run が消える（404） | D13 |
| アクティビティの応答が壊れた JSON | 取得の失敗として扱い、直前の内容を残す |
| `intervalSeconds` が 1 秒・30 秒・大きい値 | D16 の式。0 は今のとおり設定エラー |

## 影響

- **上流（takt、takt-mcp）**：変えない。takt の読み取りは、画面が開いて見えている間だけ増える。画面が無いとき・すべて hidden のときは `intervalSeconds`（既定 30 秒）。
- **保存データ**：なし。
- **公開 API**：先頭の表のとおり。CLI は変えない。
- **守りの仕組み**：先頭の表のとおり。
- **LAN・VPN（`0.0.0.0:3000`）**：待ち受け・ポート・認証・接続元の制限を変えない。
- **負荷**：
  - `router.refresh()` をやめるので、SSE の受信ごとのページ全体の RSC 再取得（layout の `GetOverview` を含む）が無くなる。
  - poller は、どれかの画面が 1 秒を求めている間、全プロジェクトを 1 秒（または下限）で読む。読み取りの回数はタブの数によらず 1 つ。
  - タブの数だけ増えるもの（見えているタブ N 枚）：要求のある WatchState の組み立て（各タブに毎秒 1 回、ストアから）と SSE の送信、ポーリングの画面の BFF の要求（アクティビティは毎秒 `GetActivity` の 7 日分の集計、run 詳細は毎秒 `GetRunDetail` の組み立て）。N は端末ごとの見えているタブの数で、hidden は 0。
  - #130 の実測は、その経路が入る PR の受け入れに入れる。
    - PR 1：poller の 1 秒と 30 秒（要求は `curl` などで `/api/state?refresh_ms=1000&screen=…&attempt=1` を開いて出す）。1 周期の時間と takt-mcp の呼び出し数。
    - PR 2：`/api/activity` と `/api/runs/[slug]` の 1 秒のポーリング。`GetActivity` と `GetRunDetail` の所要時間、`router.refresh()` をやめる前後の RSC の要求数。
    - PR 3：見えているタブ 1 枚と 3 枚で、WatchState の組み立ての回数と送信の大きさ。

## Quint

### 置き場所と範囲

- 置き場所：**`specs/quint/refresh-demand/`**（新設）。`refreshDemand.qnt`（状態機械）、`refreshDemand_test.qnt`（17 件のテスト）、`README.md`（実行の記録）。`specs/quint/ci-specs.json` に登録したので、CI の `quint` ジョブが `typecheck`・`test`・`quint run --main=fixed --invariant=safety --max-steps=60 --max-samples=10000` を流す。
- `state-stream-relay` に足さなかった理由：あちらは Next の中継（`route.ts`）の状態機械で、この仕様は backend の登録簿と poller と画面の試行番号の組み合わせ。マニフェストは仕様ごとに不変条件と実行条件を持つので、分けたほうが CI の記録が読める。
- 範囲：登録簿（登録・置き換え・古い試行の拒否・解放・寿命）、送信の間隔と `applied`、poller の待ち、画面の試行番号。変化の合流と送信スケジュールの細部は P3、要求なしの接続は E12 が受け持つ。
- 時計は注入した `now`（1 目盛り = 1 秒）だけ。期限は素通りできない（`tick` は最も近い期限で止まる）。

### 確かめたこと（quint 0.32.0、`quint-modeling` の見直しの手順）

`quint typecheck` は 2 ファイルともエラーなし。`quint test` は 17 件すべて成功。`bash scripts/quint-check-specs.sh` は全仕様で成功（終了コード 0）。そのうえで、Quint が通ったことだけを根拠にしないよう、次の 4 つを確かめた。

#### 見直しの手順の報告（`quint-modeling` の review.md）

```
── Spec review: specs/quint/refresh-demand/refreshDemand.qnt ──
Structural
  guarded actions               ⚠ arrive と initialSend は登録するか・断るかの分岐を action の中に持つ
                                  （register・isStale・dropGuard は pure def。分岐を別 action に分けると、
                                  同じ状態から両方が選べてしまい、実装の 1 本の if と合わなくなるため残した）
  enums over strings            ✓ Phase・CloseReason は和型
  record grouping               ✓ Conn・Entry・Client・Fired
  witnesses present & per-action✓ 13 の遷移すべてに firedX、意味のある到達 6
  right abstraction             ✓ 識別子は整数、時刻は秒の整数、直列化・再試行の詳細なし
  non-trivial invariants        ✓ 9 つとも var を参照し、どれも変異で違反する（(a)）
Runtime
  init assigns all vars         ✓
  var assignment per action     ✓ どの action も全 var を代入する（typecheck と quint run が通る）
  step exists & complete        ✓ 13 の action すべてが step の any に入る
  invariants hold at init       ✓
  no counterexample after acts  ✓ 反例なし（(d) の大きさで）
```

#### (a) 不変条件が恒真でない

10 の定数のうち 1 つだけを `false` にした変異を 10 個作り、不変条件を 1 つずつ渡して `quint run --max-steps=60 --max-samples=20000 --seed=0x40` を流した。9 の不変条件は、どれも少なくとも 1 つの変異で違反する。

| 不変条件 | 違反を返す変異（外したガード・変えた遷移） | 再現 |
| --- | --- | --- |
| `liveConnsRegistered` | `keepSuperseded`（置き換えた古い送信タスクを閉じない）、`noOwnerCheck`（解放が持ち主を確かめない）、`expiryKeepsTask`（寿命切れで登録だけ外す） | `--seed=0x478`、`0x478`、`0x1990` |
| `registryPointsToOpen` | `leakOnClose`（接続の終わりで guard を落とさない） | `--seed=0x40` |
| `supersededOnlyByNewer` | `noRejectStale`（古い試行の拒否を外す） | `--seed=0x47f8` |
| `latestNeverRejected` | `resetAttempts`（開き直しで番号を 1 に戻す） | 無作為の探索では 100,000 サンプル・60 ステップでも見つからない。決まった手順の `resetAttemptsRejectsLatestTest` で違反を確かめた |
| `noOverdueEntry` | `lazyExpiry`（寿命の期限を時計が素通りする） | `--seed=0x1990` |
| `noExpiryBeforeFirstSend` | `acquireEarly`（受け付けで登録する） | `--seed=0x8b0` |
| `appliedTracksFloor` | `fixedAtStart`（`applied` を接続の開始で固定） | `--seed=0x7a98` |
| `pollerNotLate` | `noRewake`（poller が要求の増加で起きない） | `--seed=0x40` |
| `closedReleasedGuard` | `leakOnClose` | `--seed=0x40` |

`refreshDemand_test.qnt` の 17 件のうち 6 件は、変異が違反に至る筋書き（`acquireEarlyExpiresBeforeFirstSendTest`、`fixedAtStartIgnoresFloorTest`、`noOwnerCheckDropsNewDemandTest`、`expiryKeepsTaskLiesTest`、`noRewakeSleepsThroughDemandTest`、`resetAttemptsRejectsLatestTest`）。残りの 11 件は、同じ筋書きを設計の案でたどって不変条件が保たれることを確かめる。

途中で見つけたこと：最初に書いた不変条件「登録は登録まで進んだ試行の最大」は、設計の案でも違反した（`--seed=0x40`）。新しい試行が解放された後に、画面がもう閉じた古い試行が遅れて届くと登録される。これは画面の閉じた接続が 1 本登録されるのと同じで、`sender.closed()` か寿命で外れる。守りたいのは「新しい要求を古い試行が置き換えない」なので、不変条件を `supersededOnlyByNewer` に言い直した。backend に試行の最大を覚え続ける墓標は置かない（画面の数だけ増え続けるため）。

#### (b) 各 action が実行の中で少なくとも 1 回起きる

遷移ごとに回数を数える変数 `fired` を持ち、`--witnesses` で数えた（`--main=fixed --max-steps=60 --max-samples=10000 --seed=0x40`）。

| action | 起きた跡の数（10,000 本中） |
| --- | --- |
| `connect` | 9,997 |
| `arrive` | 9,492 |
| `initialSend` | 6,194 |
| `send` | 632 |
| `stall` | 2,404 |
| `changeRate` | 9,972 |
| `hide` | 10,000 |
| `show` | 10,000 |
| `releaseArrives` | 9,698 |
| `expire` | 837 |
| `configChange` | 10,000 |
| `pollerRead` | 9,980 |
| `tick` | 10,000 |

意味のある状態：置き換え 503、古い試行の拒否 253、詰まった接続の寿命切れ 336、下限で `applied` が上がる 3,670、poller が 1 秒で読む 2,597、2 つの画面が違うレートで同時に登録 586。どの跡も 61 状態（60 ステップ）まで伸び、途中で止まる跡は無かった。

#### (c) 状態・action と本番のコードの対応

| Quint | 本番（予定の関数・状態） | 備考 |
| --- | --- | --- |
| `now` | `RefreshDemand` の操作に渡す `tokio::time::Instant`（D7 の時計の注入） | |
| `registry`（画面 → `Entry`） | `RefreshDemand.Inner.entries: BTreeMap<ScreenId, Entry>` | |
| `Entry.conn`・`attempt`・`rate`・`deadline` | `Entry.id: DemandId`・`attempt`・`rate`・`deadline` | |
| `minRead`・`configChange` | `Inner.min_read`・`RefreshDemand::set_min_read`（`Poller::read_cycle_once_at` が呼ぶ） | |
| `floorEpoch`・`Conn.floorEpochAtSend` | 対応なし | 不変条件 `appliedTracksFloor` のための記録。本番は送るたびに `min_read` を読むので世代を持たない |
| `Conn.phase = Requested` | 画面が `/api/state` を開いた後、backend の `watch_state` が呼ばれる前 | |
| `arrive` | `watch_state` の入口（検証と購読） | |
| `Assembling`・`initialSend` | 初期の `assemble_state_for` と最初の `tx.send`、その後の `RefreshDemand::acquire` | |
| `Live`・`send` | 送信タスクの `sleep_until(next_send)` の分岐、`send_schedule`、`sender.send`、`touch` | |
| `Conn.applied` | 送った `DashboardState.applied_refresh_interval_ms`（`applied_for`） | |
| `stall` | `sender.send` が終わらない（列 1 件と h2 の窓が埋まった） | |
| `clientClosed`・`releaseArrives` | 画面の `EventSource.close` → Next の中継の終わり → backend の `sender.closed()` の分岐 | |
| `Conn.held`・`releases`・`closed()` の guard | `DemandGuard` の所有と `Drop` | 本番の `Drop` は 1 回だけなので「2 回落とさない」は不変条件に入れていない（型で保つ） |
| `register` の置き換え・`CLOSE_SUPERSEDED` | `acquire` が古い `Entry.superseded` に通知 → 古い送信タスクの `guard.superseded()` の分岐 | |
| `isStale`・`Rejected` | `acquire` の `Err(Stale)` → `Status::aborted`。Next の `screen-streams.ts` の `admit` の `undefined` → 204 | 断る時点は Next が受け付け、backend が初期送信の後。規則は同じなので 1 つで代表した |
| `expire` | 送信タスクの `sleep_until(guard.deadline())` の分岐 → `return`（guard の `Drop`） | |
| `effective` | `RefreshDemand::effective` | D16 |
| `pollerRead`・`lastReadAt` | `run_until_stopped` の 1 周期（`read_cycle_once_at`） | 読み取りにかかる時間は 0 として扱う |
| `capturedInterval` | 変異 `noRewake` だけが使う（前回の `CycleReport.interval` で待つ） | 本番は `subscribe().changed()` で計算し直す |
| `lastDemandChangeAt` | `RefreshDemand` の `watch` の通知 | |
| `clients`・`connect`・`Client.attempt` | `StateStreamProvider` の effect、`screenId`（`useRef`）と `nextAttempt`（D8）、`openStateStream` | |
| `changeRate` | `RefreshRateProvider` の `setRefreshRate`・`storage` イベント → effect の依存 | |
| `hide`・`show` | `usePageVisible` → effect の依存 | |
| `tick` | 時間の経過 | |
| `fired`、`supersededBy`、`MAX_CONNS`、`MAX_TIME` | 対応なし | witness と不変条件のための記録、状態空間を閉じるための上限 |

#### (d) 検査の大きさと、それで見られないもの

- 大きさ：画面 2、接続 6 本、時刻 200 秒まで、`tick` は 1・4・15 秒、レート 1・2・5・10 秒、下限 1・3・10 秒、`intervalSeconds` 30 秒。CI は `fixed` を 60 ステップ・10,000 サンプル（seed は毎回乱数）。変異は 60 ステップ・20,000 サンプル（`resetAttempts` だけ 100,000）。
- `fixed` の `safety` は、seed 0x40 の 10,000 サンプルに加え、CI と同じく seed を固定しない 200,000 サンプル × 5 回（合わせて 100 万本、60 ステップ）で反例なし。CI の乱数の探索が無関係な PR を止める見込みは、この範囲では小さい。
- 「反例が見つからなかった」のは、上の大きさでサンプルした跡の中だけ。「成り立つ」とは言えない。
- 見られないもの：
  - 画面 3 つ以上、接続 7 本以上、200 秒より長い開きっぱなし、60 ステップより長い列。
  - ミリ秒の端数（例：下限 1500ms）。秒の整数だけ。
  - 要求なしの接続、変化の合流、`DashboardState` の中身、組み立ての失敗、停止の合図（E12・P3・既存の `watch_state_test.rs`）。
  - 画面と Next の間の半開き。Next の中継が読み続けるので backend の寿命は切れない（D7）。この経路は `state-stream-relay` と `state-stream-watchdog` の範囲。
  - OS の TCP の再送の上限、Next のプロセスが複数ある場合、tokio のスケジュールの遅れ、poller の読み取りにかかる時間。
  - 進行（「いつかは外れる」）は時間の性質としては書いていない。期限を素通りできない作り（`EXPIRY_URGENT`）と `noOverdueEntry` で代える。

### 既存の Quint の定数は合うか

- `state-stream-watchdog` は 1 目盛り 5 秒、`RESEND_INTERVAL = 3`（15 秒）、`STALE_TIMEOUT = 8`（40 秒）。要求のある接続の送信間隔は 1〜10 秒で、15 秒より短い。この仕様が使うのは「健全な接続は `RESEND_INTERVAL` ごとに受信がある」という上限の側なので、短い間隔は保守的に含まれる（15 秒で成り立つ性質は、もっと頻繁に受ける接続でも成り立つ）。1 目盛り 5 秒なので 1 秒の細かさは表せないが、見張りの性質は細かさに依存しない。`STALE_TIMEOUT > 2 × RESEND_INTERVAL` は、要求ありの 2 × 10 = 20 秒 < 40 秒、要求なしの 2 × 15 = 30 秒 < 40 秒で保たれる。仕様の冒頭のコメント「backend が 15 秒ごとに送り直す」は「最大 15 秒ごと」に古くなるので、PR 1 でコメントと README の 1 行を直す（定数と不変条件は変えない）。
- 寿命切れや古い試行の拒否で backend がストリームを終えると、画面から見ると `onerror` になる。`state-stream-watchdog` の `deliverError` と、`state-stream-relay` の `backendEnds`（gRPC の終わりで中継を閉じる）が受け持つ経路で、新しい遷移は要らない。
- `state-stream-relay` の `connect` は試行番号を `cl.attempt + 1` で増やす（画面ごとに単調）。D8 はこの前提を本番でも保つ。今のコードは effect が 1 回しか走らないので保たれているが、PR 3 で開き直しの経路が増えるので、D8 が無いと前提が崩れて既存の仕様もコードを表さなくなる。

### 直したあとの食い違いの読み直し

Quint を書いて分かったことを、ほかの節へ戻した：

- 登録は初期送信の後（D7）→ 変える場所の `watch_state`、壊しうる入力の「初期の組み立てが遅い」、E10。
- `supersededOnlyByNewer` の言い直し（新しい試行が解放された後の、遅れた古い試行の登録はありうる）→ D8 の「断られるのは古い試行だけ」、P2 の性質を「置き換えるのは新しい試行だけ」に直した。
- poller の式（D16）→ 不変条件 6、変える場所の `poller.rs`、Quint の `effective` を同じ式にした（`386fe5c`）。
- 寿命が延びるのは送信の完了だけ（D7）→ E22、壊しうる入力の「送信が詰まる」。

## 分け方

条件：各 PR が単独でマージでき、その時点で機能が嘘をつかず、既存のテストが緑であること。

| PR | 内容 | 大きさ | 単独でマージしたとき |
| --- | --- | --- | --- |
| 1 | proto（`RefreshRate`、要求と `applied`）と生成物、backend の `RefreshDemand`・`watch_state`・poller・config、BFF の `refresh_ms` の転送（`client.ts`・`route.ts`）、poller の #130 の実測 | 5 | 画面はまだ `refresh_ms` を付けないので、すべての接続が要求なしで今と同じ。新しい経路は E10〜E12・E21〜E23 と BFF のテストで確かめる。選択の UI が無いので、嘘になる表示は無い |
| 2 | `router.refresh()` の廃止、プロジェクト・アクティビティ・run 詳細の要素単位の更新、2 つの Route Handler、既存テストの書き直し、flow-9 | 8 | アクティビティと run 詳細はタスク詳細などと同じ固定 1 秒のポーリング、プロジェクトは SSE の状態。選択の UI はまだ無い。受け入れ条件 3 を満たす |
| 3 | レートの選択・保存・復元、hidden の停止、試行番号（D8）、`applied` の通知、ポーリングのレート、requirements、e2e の新しい spec | 5 | PR 1 の backend が要求に従うので、SSE の画面も選んだレートで動く。e2e の backend は 1 秒のまま。受け入れ条件 1・2・4 を満たす |
| 4 | e2e の既定 30 秒、`StartStackOptions.intervalSeconds`、1 秒が要る spec の切り替え、E18 | 2 | e2e だけ。受け入れ条件 4 を、本番と同じ既定の backend で確かめる |

- 順序は 1 → 2 → 3 → 4。2 は 1 に依存しない（並べてもよい）。3 は 1 と 2 に依存する（`applied` の型、`LiveActivity`・`LiveRunDetail`）。
- 前の版の分け方（A に UI、B に backend、C に通知と e2e）をやめた理由：A だけでは SSE の画面が選んだレートで動かず、選択の UI が嘘になる。C は B と A の型を参照し、単独で通らない。
- 1 つの PR にまとめると 20 になるので、まとめない。
- PR 2 を小さくしたい場合は 2 本に分けられる。2a：アクティビティと run 詳細の要素単位の更新（Route Handler・`LiveActivity`・`LiveRunDetail`・復元・そのローダーと既存テスト・flow-9。`router.refresh()` は残すが、Live の部品は初期値しか props から読まないので、再取得されても表示は二重にならない）、大きさ 5。2b：`router.refresh()` の廃止と `LiveProjects`・残りの既存テスト、大きさ 3。2a を先に入れれば、どちらの時点でもアクティビティと run 詳細は止まらない。

## 決まっていない点

1. **既定の下限 1 秒の負荷**：`minReadIntervalMs` の既定 1000 は、受け入れ条件 4（1 秒で 1 秒）を既定で満たすための値で、負荷の根拠はまだ無い。PR 1 の #130 の実測（影響の節）で重ければ、既定を上げるか、間隔を分ける。上げると受け入れ条件 4 を既定では満たさなくなるので、そのときはオーナーに判断を仰ぐ。
2. **run 詳細の 1 秒のポーリング**：1 ページで定期的に読むのは 2 本（詳細とログ）。WatchState の run の要約（状態・今のステップ・最後のログ時刻）で変化を知ったときだけ詳細を読む形にすれば 1 本になる。#130 の実測の後に比べる。
3. **1 秒ごとの全状態の送信の大きさ**：要求のある接続には、変化が無くても間隔ごとに `DashboardState` 全体が届く。変化が無ければ送らない、または `read_at` だけの軽いイベントにする案を、実測と合わせて比べる（その場合は D7 の寿命の延ばし方と、画面の見張りへの影響を見直す）。
4. **画面と Next の間の半開きで残る要求**：D7 のとおり、画面が消えて TCP が切れないとき、要求は OS の再送の上限まで残り、その間 poller は速いまま。hidden での切断（D9）で多くは避けられる。残る場合に、Next の中継が「画面へ送れない時間」を見て閉じる案があるが、#385 の作りに触れるので、実測で問題になってから決める。

## 範囲の外

- SSE の再接続・無音の見張り（#383）と、見捨てられた接続の後始末（#385）の作り。今回は送り方と要求の寿命だけを足す。
- 判断待ちの回答・マージ承認・ゴール登録の書き込みの流れ。
- 認証、待ち受けアドレス（`0.0.0.0:3000`）、接続元の制限、VPN の設定。
- DB の表・列・索引・保存形式。
- takt と takt-mcp の変更。
- 全端末で同じレートにすること（サーバー側の設定や Cookie。D1）。

## 読み直しの指摘との対応表

### 20 件

| # | 読み直しの要点 | 直したところ |
| --- | --- | --- |
| ① | watchdog の Quint の 15 秒 | Quint の「既存の Quint の定数は合うか」 |
| ② | 既定 1 秒の数値は実装 PR 送り、N タブ × 毎秒が負荷に無い | 影響の「負荷」（タブの数だけ増えるもの）、決まっていない点 1 |
| ③ | TTL が「数倍」、寿命切れで送信タスクを閉じない、初期組み立てとの順序 | D7、Quint の `expiryKeepsTask`・`acquireEarly`、E22 |
| ④ | backoff の上限が「例」、ポーリングは 1 秒＋取得時間、run ログに ready/visible の口が無い | D11、不変条件 9 |
| ⑤ | 要求なし 15 秒と `applied` の矛盾、通知が SSE 限定と書いていない、固定か再計算か | D4・D6 |
| ⑥ | 正本 JSON が生成・関門・ビルドの外、数値が 4 か所 | D5、数値の置き場所 |
| ⑦ | 「空の内容」の型と表示、Provider 無しで落ちるテスト | D13・D14、既存テストの表 |
| ⑧ | e2e の既定 30 秒の口と対象 spec | PR 4、e2e の既定を 30 秒にする |
| ⑨ | プロファイル・コンテナで分かれる注記 | D1 |
| ⑩ | 直っている | — |
| ⑪ | Quint の置き場所・倍率・watchdog・make done が回さない | Quint の節（置き場所、CI の `quint` ジョブ、定数） |
| ⑫ | 複数タブと run ログの読み位置 | D15 |
| ⑬ | 子の effect が先に走り、ready を依存に入れないと止まる | D10・D11 |
| ⑭ | 起動直後の設定失敗で下限が未設定 | D6（初期値 1000、失敗時は前の値） |
| ⑮ | 決まっていない点 4 が方針 1 と矛盾 | D1（消した） |
| ⑯ | e2e の待ち先の書き換え | 既存テストの表（flow-9） |
| ⑰ | 第 5 章の書き換えは記録の改変 | D12 |
| ⑱ | `projects-loader` は RPC を出していない、state-sharing:41 が落ちる | いまの状態、PR 2 の `projects-loader.tsx`、既存テストの表 |
| ⑲ | 試行番号の either/or、204 で固まる | D8、Quint の `latestNeverRejected` |
| ⑳ | `readAt` を更新しない（既存と同じ） | PR 2 の `live-projects.tsx`（今と同じ扱い） |

### 増えたものの矛盾（読み直しの 2 章）

| 矛盾 | 直したところ |
| --- | --- |
| 正本 JSON が生成・関門・ビルドに乗らず、一致の確かめが 2 本に割れる | D5、数値の置き場所 |
| `applied` の 3 つの値、通知の範囲、固定か再計算か、`assemble_state_for` が間隔を受けない、`liveStateReceived` が A の変更と書いていない | D4・D6、PR 3 の `state-provider.tsx` |
| `minReadIntervalMs` の範囲の再定義、送信側は開始時の値、L300 の `min`、既定 1000 の根拠 | 数値の置き場所、D6、D16、決まっていない点 1 |
| A と B をつなぐ転送が無い、C が単独で通らない | 分け方（PR 1 に転送を入れた） |

### テストにできない 11 か所

「テストにできない・しにくいところ」の表。

### 単独マージ（読み直しの 3 章）

分け方の表。

# issue #40 の設計：画面の情報をリフレッシュレート（1秒・2秒・5秒・10秒、既定1秒）で要素単位に自動更新する

対象の issue：[j5ik2o/takt-dash#40](https://github.com/j5ik2o/takt-dash/issues/40)
関連：[j5ik2o/takt-dash#130](https://github.com/j5ik2o/takt-dash/issues/130)（毎秒の全ページ再取得の負荷）、[j5ik2o/takt-dash#383](https://github.com/j5ik2o/takt-dash/issues/383)（SSE の無音の見張り）、[j5ik2o/takt-dash#385](https://github.com/j5ik2o/takt-dash/issues/385)（見捨てられた SSE の後始末）
大きさの基準：[j5ik2o/takt-dash#361](https://github.com/j5ik2o/takt-dash/pull/361) = 1（+23 −2、1 ファイル）、[j5ik2o/takt-dash#362](https://github.com/j5ik2o/takt-dash/pull/362) = 5（+1147 −122、18 ファイル）

確かめたコード：`7917195`（`main`）。前の版が見た `d7e6d5b` からの差分は `tools/prod-check/prod_log_test.py` だけで、この設計が参照するコードの行は変わっていない。その後に付け直した `a49595b` までの差分も `.github/workflows/review-thread-resolution.yml` だけ。この版で付け直した `a59ca97` までの差分は `docs/design/issue-134.md` と `specs/quint/review-thread-ack/` だけで、参照する行は変わっていない（3 回目の読みが足した `frontend/cli` の 2 か所、`client.ts:26`、`grpc/mod.rs:80,233,598-665`、`lib.rs:11-25`、`last-loaded.ts:43-44`、`poller.rs:384-393` も `a59ca97` で読み直した）。
この版は、前の版（`584bd07`）への 3 回目の敵対的な読み（A〜O）と、takt-dash中将の採否（すべて採る）と決め 1〜6 を入れたもの。どれをどこに入れたかは末尾の「3 回目の読みとの対応表」にある。2 回目（22 件）と 1 回目（20 件）の対応表もその下に残した。

## 先頭のまとめ

### 公開 API・保存データ・秘密・守りの仕組み

| 項目 | 触れるか | 内容 |
| --- | --- | --- |
| 公開 API（proto） | **はい** | enum `RefreshRate` を足し、`WatchStateRequest` に `refresh_rate`・`screen`・`attempt`、`DashboardState` に `applied_refresh_interval_ms` を足す。どれも省略時は 0・空で、古いクライアントと古い BFF は今のまま動く（要求なしの接続は、変化をすぐ送り、15 秒は再送だけ）。 |
| 公開 API（HTTP） | **はい** | `/api/state` に省略可能な `refresh_ms` を足す。BFF に `/api/activity` と `/api/runs/[slug]` を足す。どれも画面専用で、既存の URL と引数の意味は変えない。新しい Route Handler は既存と同じく `dynamic = "force-dynamic"` と `Cache-Control: no-store`。 |
| 保存データ | **いいえ** | DB の表・列・索引・保存形式、設定以外のファイルを変えない。画面の選択は `localStorage`（origin ごと）だけに置く。設定ファイルに任意の `minReadIntervalMs` を足す（無ければ既定 1000）。 |
| 秘密 | **いいえ** | 秘密・認証・接続元の制限を増やさない。LAN・Tailscale・SoftEther VPN の中は信頼する範囲、というオーナーの決定のまま。`refresh_ms`・`screen`・`attempt` は秘密ではない。 |
| 守りの仕組み | **一部** | CI・関門・ルールセット・`make done` の定義は変えない。この設計の PR で `specs/quint/refresh-demand/` を足し、`specs/quint/ci-specs.json` に 1 件登録するので、既存の `quint` ジョブ（`scripts/quint-check-specs.sh`）がこの仕様も検査する（`fixed` を 150 ステップ・10,000 サンプル。手元で約 7 秒）。変異を流し直す `specs/quint/refresh-demand/mutants.sh` も置くが、CI には載せない（手元で流す道具。`review-thread-ack/mutants.sh` と同じ扱い）。実装の PR はテストを足すだけ。 |

### 大きさ

**合わせて 30 を見込みとする。8 を超えるので 7 本に分ける（1a：2、1b：5、2a：5、2b：3、読みを減らす PR：5、3：8、4：2）。** 分け方と、それぞれが単独でマージできる理由は「分け方」の節にある。

- 読みを減らす PR は、#130 の実測（「負荷の合格線」の節）が合格線を外れたときだけ入れる（決め 1）。**今の本番の実測では外れる見込みが高い**（1 周期の所要が p50 9.4 秒）ので、見込みには入れて 30 とする（決め 1 の最後）。合格線を満たせば 25。
- 2 回目の読みで 20 から 25 に増えたのは、PR 1 の型の追従（`applied` のフィールドを PR 1 に前倒しし、`assembly.rs`・試験の literal・`restore-state.ts`・`client.ts` を直す）、内容の部品を画面の型へ移す変更とそのテスト、ローダーを Provider の外で描くテストの描き方と Context の literal、組み立ての後の寿命の確かめ、時計の一本化、#130 の合格線の実測のため。
- 3 回目の読みで足した作業（PR 1a の `frontend/cli` の literal 2 か所と `client.ts` の import、PR 1b の組み立てを差し替える口、PR 3 の「読み取りに N 秒かかっています」）は、どれも各 PR の段の中に収まり、段を変えない（理由は「分け方」の節）。
- PR 1 は 8 になるので 1a（proto と型の追従）と 1b（backend の要求）に分けた。PR 2 は 8 を超えるので 2a・2b に分けた。PR 3 は変えるファイルが 28 前後で #362 の 18 より多いので 8。

## 目的

- 画面にリフレッシュレート（1秒・2秒・5秒・10秒、既定1秒）の選択を置き、再読み込み後も選択を保つ。
- すべてのページの表示中の情報を、選んだレートで自動更新する。ページ全体を再読み込みせず、情報を持つコンポーネントや要素の単位で差し替える。
- 1秒を選んだときに、takt 側の状態の変化がおおむね1秒程度で画面へ届くよう、backend の読み取りと配信の間隔も選んだレートに追従させる。

## いまの状態（`7917195` で確かめた）

### できていること

- 概要・判断待ち・ゴール一覧・サイドバーは、`StateStreamProvider` が受ける WatchState の SSE の状態をそのまま描画に使っている（`live-overview.tsx:16-24`、`live-decisions.tsx:8-10`、`live-goals.tsx:14-16`、`live-sidebar.tsx:17-19`）。
- タスク詳細・ゴール詳細・会話・run 詳細のログは、ページを保ったまま 1 秒固定の間隔で BFF の Route Handler を読み、値だけを差し替えている（`live-task-detail.tsx:37-38`、`live-goal-detail.tsx:28-29`、`live-conversation.tsx:75-78`、`live-run-log.tsx:55-57`）。間隔は `startPolling`（`polling.ts`）の「前回の完了から次まで」。
- SSE の再接続・無音の見張り（#383）と、見捨てられた接続の後始末（#385）は入っている。

### ページ単位の再取得になっているところ

- プロジェクト・アクティビティ・run 詳細は、SSE の状態を受けるたびに `router.refresh()` を呼び、RSC のページ全体を取り直している（`state-stream.ts:71-82` の `createStateHandlers` が受信ごとに `revalidate`、`state-provider.tsx:132-164`、`state-provider.tsx:188-198` の `StateStreamProviderWithRefresh`）。
  - `projects-loader.tsx:12-21` は要求内で共有する `loadServerState()`（layout の `GetOverview` の結果）を読むだけで、自分では RPC を出さない。
  - `activity-loader.tsx:14-44` は `loadServerState()` と `GetActivity`、`run-detail-loader.tsx:23-65` は `GetRunDetail` と、サーバー名のための `loadServerState()`（`:53-59`）を読む。
- そのため、状態が変わったときだけでなく backend の 15 秒の再送でも、ページ全体の RSC 再取得が走る（#130 の一部）。

### 型の境界

- `ProjectsContent`（`projects-content.tsx:2,9-12`）と `ActivityContent`（`activity-content.tsx:2,7-11`）は proto の `DashboardState` を受ける。`StateStreamProvider` の Context が持つのは画面の型（`restore-state.ts:120-130` の `DashboardState`）で、`GoalsContent`（`goals-content.tsx:2,16`）はこちらを受ける。
- 画面の型は `Omit<ProtoDashboardState, …>` なので、proto に `applied_refresh_interval_ms` を足すと、`restoreDashboardState`・`dashboardStateFromProto`（`restore-state.ts:333-360`）が返す値に足りない欄ができ、型で落ちる。

### できていないこと

- リフレッシュレートの選択と保存がない。
- アクティビティと run 詳細に、要素単位のクライアント側の取得経路がない。
- backend の読み取り間隔は設定の `intervalSeconds`（既定 30 秒、`config.rs:23`、`poller.rs:200`）、WatchState の再送は 15 秒（`grpc/mod.rs:80`）で、画面の選択と結び付いていない。

### 本番の読み取りの実測（2026-11-28 に読んだ）

本番の backend のログ（`data/backend-launchd.err.log` の「定期読み取りの1サイクルを終えました」の行、2026-11-27 06:38〜11-28 07:42 UTC の 1,000 周期）と `ps` から：

| 項目 | 値 |
| --- | --- |
| 設定 | プロジェクト 5、takt-mcp は常駐の HTTP サーバー 1 本（5 プロジェクトとも同じサーバーを読む）、`intervalSeconds` 90 |
| 1 周期の所要（`cycle_ms`） | 最小 4.3 秒、p50 9.4 秒、p90 28.7 秒、p95 43.0 秒、最大 242 秒 |
| 周期の開始の間隔 | p50 90.0 秒、p90 105.5 秒 |
| 1 周期の呼び出し（`tool_calls`） | p50 77 回（27〜82） |
| 道具ごと（最後の周期の例） | `takt_list_tasks` 11 回・合計 9.1 秒・1 回の最大 1.46 秒、`takt_list_goals` 15 回・合計 3.4 秒、`takt_get_goal` 21 回・合計 1.3 秒、ほか 3 つで 30 回・合計 1.0 秒。`takt_list_tasks` の 1 周期の合計は p50 9.2 秒・p90 15.9 秒 |
| 平均の呼び出し | 0.86 回/秒 |
| CPU（`ps` の累積時間 ÷ 経過時間、1 コア比） | takt-mcp の HTTP サーバー 4.3%（4 時間 24 分で 11 分 19 秒）、backend 0.5%（38 時間で 10 分 57 秒。本番は debug ビルド） |
| マシン | Apple M4 Pro、14 コア |

poller の待ちは「周期の開始から `effective`」（`poller_stop.rs:76,99-102,157-162`）なので、1 秒の要求があると周期は休まずに続けて回る。今の読み方のままなら、鮮度はおおむね 1 周期の所要（p50 9.4 秒）になり、呼び出しは約 77 ÷ 9.4 ≒ 8.2 回/秒（今の約 9.5 倍）になる。

## 受け入れ条件ごとの対応

| # | 受け入れ条件 | 対応 | 入る PR |
| --- | --- | --- | --- |
| 1 | 1秒・2秒・5秒・10秒から選べ、既定が1秒 | サイドバーに「更新間隔」の `select`。選択肢は proto の `RefreshRate` から作る。既定は `REFRESH_RATE_ONE_SECOND`。`localStorage` の `taktdash.refreshRateMs` に保存し、再読み込み後に復元する。 | 3 |
| 2 | どのページでも、クリックや再読み込みなしで表示中の情報が選んだレートで更新される | SSE の画面（概要・判断待ち・ゴール・サイドバー・プロジェクト）は backend が接続ごとのレートで送る。ポーリングの画面（アクティビティ・run 詳細・タスク詳細・ゴール詳細・会話・run ログ）は選んだレートで BFF を読む。 | 1b・2a・2b・3 |
| 3 | ページ全体が再読み込みされない | SSE の受信ごとの `router.refresh()` をやめる。e2e で同じ document・入力値・スクロール位置を確かめる。 | 2b |
| 4 | 1秒で takt の変化がおおむね1秒で現れる | backend に画面ごとの要求の登録簿 `RefreshDemand` を置き、poller はその最小値で読み、各接続は自分の間隔で送る。下限は `minReadIntervalMs`（既定 1000）。**読み取りの所要が 1 秒より長いと満たせない**ので、PR 1b の後に本番で測り、合格線（「負荷の合格線」）を満たすまで PR 3 をマージしない。 | 1b・3 |
| 5 | `make done` が通る | 各 PR で `make done` を通す。proto の生成物は同じ PR でコミットする。#130 の実測を、その経路が入る PR ごとに受け入れに入れる（「負荷の合格線」と影響の「負荷」）。 | 全部 |

## 方針（決めたこと）

番号は本文と対応表から参照する（D1〜D18）。

### D1. レートは origin ごとの設定にする

`localStorage` は origin ごとなので、`localhost`・LAN の IP・tailnet のホスト名では別の値になる。同じ origin のタブは `storage` イベントで同じ値にそろう。同じ origin でも、ブラウザのプロファイルやコンテナ（Firefox の Multi-Account Containers など）が違えば別の値になる。サーバーには保存しない。

### D2. SSE の状態を要素単位の更新の主経路にし、`router.refresh()` をやめる

概要・判断待ち・ゴール・サイドバー・プロジェクトは `StateStreamProvider` の状態から描画する。`createStateHandlers` の `revalidate` と `StateStreamProviderWithRefresh` を消す。

### D3. アクティビティと run 詳細は、RSC の初回表示＋BFF のポーリングにする

初回はサーバーで読んで内容を出し、開いている間は `/api/activity` と `/api/runs/[slug]` を読む。初回が失敗してキャッシュも無いときの表示は D13。

### D4. 要求のある接続だけが間隔で送る。要求の無い接続は今のまま

| 接続 | 送り方 | `applied_refresh_interval_ms` |
| --- | --- | --- |
| 要求なし（`refresh_rate` が 0。古い画面・古い BFF・`refresh_ms` の無い `/api/state`・不正な `refresh_ms`） | 今の `watch_state` と同じ。変化を受けたらすぐ送り、変化がなくても 15 秒ごとに再送する（`grpc/mod.rs:625-661`）。登録簿に触れない。 | 0 |
| 要求あり（1000・2000・5000・10000） | 初期状態はすぐ送る。その後は送信の期限 `next_send`（前の送信の完了から `applied`）まで待ち、変化は合流させ、期限に最後の状態を 1 回だけ組み立てて送る。変化がなくても期限に送る。 | その送信で予定した間隔（D6） |

- 要求のある接続は、送信の間隔に 1 回までしか送らない。変化がすぐ届かない代わりに、遅れは最大でその間隔＋組み立てと送信の時間。
- 期限の計算は純粋関数 `send_schedule(now, last_sent, interval, dirty) -> SendDecision` に切り出し（P3）、`watch_state` はその結果で `sleep_until` する。
- `applied_refresh_interval_ms` は `assemble_state_for`（`grpc/assembly.rs:220-233`）では 0 を入れる（unary の応答と要求なしの接続は 0 のまま）。要求のある接続は、`watch_state` が組み立てた `DashboardState` に、送る直前に入れる。

### D5. 選択肢と受ける範囲は proto の enum 1 か所に置く

`proto/taktdash.proto` に次を置く：

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
  - **確かめた**：ts-proto（`frontend/web` の固定版）で上の enum を生成すると `export enum RefreshRate { REFRESH_RATE_UNSPECIFIED = 0, REFRESH_RATE_ONE_SECOND = 1000, …, REFRESH_RATE_TEN_SECONDS = 10000, UNRECOGNIZED = -1 }` になる。
- backend は `backend/build.rs:32` の `tonic_prost_build::configure().compile_protos(...)` がビルド時に生成する `RefreshRate` を使う。
  - **確かめた**（2 回目）：捨てる worktree で proto に足して `cargo check --tests` を流すと、prost は前置きを外して `RefreshRate::{Unspecified, OneSecond, TwoSeconds, FiveSeconds, TenSeconds}` を作る。`WatchStateRequest.refresh_rate` は `i32` の欄になる。
- 画面の選択肢は `Object.values(RefreshRate)` のうち 0 より大きい数値（`UNSPECIFIED` と `UNRECOGNIZED` を除く）。文言は `ms / 1000` から `1秒` などを作る。数値を TypeScript に書き写さない。
- backend が受けるのは enum の 5 つの値だけ。0 は要求なし、4 つの値は要求あり、それ以外（未知の enum 値）は `INVALID_ARGUMENT`。
- 既定の選択は UI の決まりなので frontend の `DEFAULT_REFRESH_RATE = RefreshRate.REFRESH_RATE_ONE_SECOND` の 1 か所に置く（数値ではなく名前）。
- `minReadIntervalMs` と `applied` は 4 つの選択肢と別の量（1000〜10000 の任意の整数）なので、範囲の両端を `RefreshRate` の最小・最大から作る（下の「数値の置き場所」）。

### D6. `applied_refresh_interval_ms` の意味と、下限の変更の効き方

- **値の意味**
  - `0`：その接続には要求が掛かっていない。要求なしの接続、unary の応答、この機能の無い古い backend のどれか。
  - `1000〜10000`：その接続が、この状態を送ったときに**予定した**送信間隔。`max(要求, minReadIntervalMs)`。次の組み立ては、この送信の完了から `applied` の後に始まる。実際の間隔は、これに組み立てと送信の時間が足される（不変条件 8）。
  - 15000 は使わない。要求なしの接続の 15 秒の再送は `applied` に出さない。
  - `applied` はデータの新しさではない。poller の 1 周期が `applied` より長いと、`applied = 1000` の状態が毎秒届いても、中身（`read_at`）は周期ごとにしか変わらない。そのときは画面が読み取りの遅さを知らせる（下の「画面の通知」の 3 つ目。決め 2）。
- **毎回再計算する**。送るたびに `RefreshDemand` が持つ今の `minReadIntervalMs` で計算し直し、その値を `applied` に入れ、次の期限と寿命もその値から決める。接続の開始で固定しない。
- **`minReadIntervalMs` を変えたとき**：poller が設定を読み直した周期（`read_cycle_once_at`）で `RefreshDemand::set_min_read` を呼ぶ。開いている接続は、その後の最初の送信から新しい下限で送り、`applied` も変わる。効くまでの遅れは「poller の 1 周期＋その接続の前の間隔」以内。
- **起動直後と設定の失敗**：`RefreshDemand` の下限の初期値は既定の 1000。設定の読み込みに失敗した周期は、前に読めた値のまま（下げも上げもしない）。失敗した周期の待ちは `refresh_demand.effective(DEFAULT_INTERVAL_SECONDS, now)` を通す（要求があれば要求に従う）。
- **画面の通知**（サイドバーの「更新間隔」の下）。SSE の最初の受信の後にだけ出す。
  - `applied` が選んだ値より大きい：「状態の配信は 5 秒ごとです（backend の下限。選んだ間隔は 1 秒）。概要・判断待ち・プロジェクト・ゴール・アクティビティの内容はこの間隔で変わります。」
  - 要求を出したのに `applied` が 0：「backend が更新間隔の指定に対応していません。状態の配信は、変化のときと 15 秒ごとです。」
  - 読み取りの周期が選んだ間隔より長い（決め 2。PR 3）：「読み取りに N 秒かかっています。概要・判断待ち・プロジェクト・ゴール・アクティビティの内容は、およそ N 秒ごとに変わります。」
    - N は、プロジェクトごとの `lastReadAt`（`DashboardState.projects[].last_read_at`、`assembly.rs:74`）が前の値から進んだ間隔の、全プロジェクトでの最大を秒に丸めたもの。読めていないプロジェクト（`connected` が false）は数えない（応答なしは既存の表示が受け持つ）。`lastReadAt` は読み取りが成功するたびに、中身の変化の有無によらず進む（`store.rs:169-183` の `record_success`）。全体の `read_at`（`assembly.rs:230`）はプロジェクトの最大なので 1 周期の中で何度も進み、周期の長さを表さないので使わない。
    - 間隔は、画面が状態を受けた時刻（ブラウザの時計）どうしの差で測る。backend の時刻とブラウザの時計を比べない（LAN の端末は時計がずれうる）。進んでから次に進むまでの間も、受けた時刻で経過を数え、それが前の間隔より長ければ N に使う（周期が止まっても数字が増える）。精度は送信の間隔（`applied`）の 1 回分。
    - 起点はストリームを開くたびに捨てる（hidden → visible、レート変更、中の再接続。D8 の開き直しの経路のどれでも）。開き直した後の最初の受信は起点を作るだけで、N を出さない。捨てないと、hidden や切断の間の時間が最初の受信で N に出てしまう。
    - 出す条件は `N ≥ 2 × max(選んだ間隔, applied)`。消すのは、それを下回る間隔を 1 回受けたとき。SSE の最初の受信より前と、`applied` が 0（要求なし）のときは出さない。
    - 判定は純粋関数 `readLag(previous, received, nowMs)`（`src/state/refresh-rate.ts`）に置き、`live-sidebar.tsx` が受信ごとに呼ぶ。proto は変えない（PR 1a を開き直さない）。
  - 通知は SSE の配信と poller に従う内容のことだけを言う。タスク詳細・ゴール詳細・会話・run 詳細・run ログは画面の要求のたびに takt を読むので、`minReadIntervalMs` に関わらず選んだレートで変わる（通知の対象に書かない）。

### D7. 要求の寿命（TTL）は「最後の送信の完了から max(3 × applied, 15 秒)」

- **数値**：寿命 = `max(3 × applied, 15 秒)`。`applied` は 1〜10 秒なので、寿命は 15〜30 秒（1・2・5 秒と、下限で 3 秒になった接続は 15 秒、10 秒は 30 秒）。上限の 30 秒は前の版と同じ。
- **理由**
  - 15 秒の下限は、要求なしの接続の再送の間隔（`WATCH_RESEND_INTERVAL`）と同じ。「健全な接続は 15 秒以内に次を受ける」という今の前提（`state-stream-watchdog` の `RESEND_INTERVAL`）にそろえ、それより短い詰まりでは要求を外さない。前の版の 3 × 1 秒 = 3 秒は、Next の処理の遅れや組み立ての遅れ（本番では数秒ありうる）で健全な接続まで切っていた。
  - 3 倍は、遅い間隔（10 秒）で送信 2 回分の遅れを許して 3 回目で見切る値。
- **寿命が効くのは、backend の送信の列が詰まったときだけ**：寿命を延ばすのは送信の完了（列に入ったとき）で、Next の中継は gRPC を読み続ける（#385 の間引きで、画面へ送れない間は最新の 1 件だけを持つ）。だから画面と Next の間が半開きでも backend の列は詰まらず、寿命は切れない。詰まるのは Next が gRPC を読まないとき（Next のプロセスやイベントループが止まる、Next と backend の h2 の窓が埋まる）。
- **見張り 40 秒との関係**：前の版の「30 秒 < 40 秒なので、画面が見張りで開き直す前に backend の古い要求が外れる」は、半開きでは偽だった（半開きでは寿命は切れない）。詰まりの場合も、画面が開き直した新しい試行は古い試行を置き換える（D8）ので、置き換えの正しさは 30 < 40 に頼らない。30 < 40 は「詰まった接続の要求が、画面の開き直しより長く poller を速いまま保たない」ための上限として残す（`refresh.rs` の `const` assert）。
- **寿命を延ばすもの**：`sender.send(message)` が完了したときだけ。`touch(&guard, applied, now)` は、期限の前なら `deadline = now + ttl(applied)` にし、期限を過ぎていれば延ばさずに `Err(Expired)` を返す（送信タスクは閉じる）。要求のある接続の送信の列は 1 件（`WATCH_DEMAND_CHANNEL_CAPACITY = 1`）にする。最新の状態だけが意味を持つので、深い列は遅れを増やすだけだから。要求なしの接続は今の 16 件（`grpc/mod.rs:76`）のまま。
- **組み立ての時間**（2 回目の読みの指摘 5）：送信タスクは定期の組み立て（`assemble_state_for`、`grpc/mod.rs:642-647` の `blocking(...)`）の間は `select!` の外にいて、寿命の期限の分岐を待てない。そこで次の 2 つを置く。
  1. 組み立ての後、送る前に `guard.deadline() <= now` と `guard.is_superseded()` を確かめ、どちらかなら送らずに `return` する（guard の `Drop` で登録が外れる）。
  2. 送信の `select!`（`grpc/mod.rs:652-661` の形）にも、寿命の期限と置き換えの分岐を入れる。
  - 期限を過ぎた要求は `effective` が数えないので、確かめずに送ると、poller は `intervalSeconds` に戻ったのに画面には `applied = 1000` が届き、`touch` が寿命を延ばし直して poller がまた速くなる。Quint の変異 `noPostAssemblyCheck` が不変条件 `noSendAfterExpiry` に違反することで、この経路を示した。
  - 組み立てを `select!` に入れる案は採らない。`spawn_blocking` の組み立ては途中で止められず、待つのをやめても CPU を使い続けるので、結果は「組み立ての後に確かめる」と同じで、作りだけが重くなる。
  - 組み立てが寿命より長い（1 秒の要求で 14 秒を超える）のは backend がひどく詰まったときで、そのときストリームを閉じて画面に開き直させるのは正しい（poller はもうその要求を数えていない）。
- **寿命切れでしたこと**：登録簿から外し、**送信タスクも閉じる**（`select!` の `sleep_until(guard.deadline())` の分岐、または組み立ての後の確かめで `return`）。ストリームが終わるので、Next の中継が終わり、画面の EventSource は `onerror` から再接続し、次の試行で要求が戻る。寿命切れの後も要求なしとして送り続けて `applied` を偽る経路は無い（Quint の `expiryKeepsTask` と `noPostAssemblyCheck` が違反を返す）。
- **初期の組み立てとの順序**：要求は初期状態を送り終えてから登録する（`acquire` は初期送信の後）。組み立てがどれだけ長くても、最初の送信の前に寿命は切れない。その間、poller はまだ速くならないが、初期状態は組み立てた時点の最新なので困らない（Quint の `acquireEarly` が、受け付けで登録すると最初の送信の前に切れることを示す）。
- **寿命で捕まえられないもの**：画面と Next の間の半開き（上のとおり backend の列は詰まらない）。画面が生きていれば見張り（40 秒）で開き直して置き換わり、画面が消えていれば Next の TCP が再送をあきらめたときに中継が閉じ、backend の `sender.closed()` で外れる（OS の再送の上限まで、分の単位で残りうる）。タブが hidden になれば D9 で画面が先に閉じる。**残る要求は既知の制限として受け入れる**（決め 5。寿命が効くのは backend の列が詰まったときだけ、と書いたまま）。Next の中継が「画面へ送れない時間」を見て閉じる案は #385 の作りに触れるので、この issue では入れない。

### D8. 画面の識別子と試行番号は Provider が持ち、開き直しでも戻さない

決めたこと：**`StateStreamProvider` が `screenId`（`useRef` で 1 回だけ作る）と試行番号の数え手 `nextAttempt()`（`useRef` の数値を 1 増やして返す）を持ち、`openStateStream({ screenId, nextAttempt })` に渡す**。

- `openStateStream` は入口で `screenId` を 1 回だけ解決する（`options.screenId ?? createScreenId()`。今の `state-stream.ts:183` と同じく関数の最初に 1 回で、再接続のたびには作らない）。中の再接続は、この `screenId` と `nextAttempt` で次の URL を作る。
- `createAttemptUrls`（`state-stream.ts:163-169`）は、`nextAttempt` を渡されないとき（既存の呼び出しとテスト）だけ今の「1 から数える」数え手を作る。

204 で固まる経路が無いこと：

1. Next の `screen-streams.ts:31-33` と backend の `RefreshDemand` が断るのは「同じ画面にもっと大きい試行が登録済み」のときだけ。
2. `screenId` は Provider のインスタンスに 1 つで、保存しないので、別のタブや再読み込みと共有されない。
3. 同じ `screenId` の試行番号は、`nextAttempt()` だけが出し、`useRef` の値は Provider が生きている間減らないので、新しい URL の番号はそれまでに出したどの番号よりも大きい。
4. よって、画面の最新の試行より大きい番号の登録はどこにも無く、最新の試行は断られない。断られるのは、画面がもう閉じた古い試行だけ。

開き直しの経路ごと：

| 経路 | 何が起きるか | 番号 |
| --- | --- | --- |
| 中の再接続（`onerror`・見張り・`checkStale`） | `openStateStream` の中で `nextAttempt()` | 増える |
| レート変更（選択・`storage` イベント） | effect の依存（`refreshRate`）が変わり、古い stream を `close` して新しい `openStateStream` | 同じ `nextAttempt` なので増える |
| hidden → visible | effect の依存（`visible`）が変わる。hidden で `close`、visible で開き直す | 増える |
| React の StrictMode（開発時の effect の二重実行） | `useRef` は保たれ、effect だけが掃除と再実行をする | 増える |
| ページの移動（App Router） | layout は残るので Provider は作り直されない | 変わらない |
| 再読み込み・新しいタブ | Provider が新しく作られ、新しい `screenId` | 1 から（別の画面） |

この性質は Quint の `latestNeverRejected` と、変異 `resetAttempts`（今の `createAttemptUrls` のまま effect で開き直す版）の違反で確かめた。

### D9. 見えていないタブは取得しない

`usePageVisible()` は `useSyncExternalStore(subscribe, getSnapshot, getServerSnapshot)` で作る。`subscribe` は `document` の `visibilitychange` を聴き、`getSnapshot` は `document.visibilityState === "visible"`、`getServerSnapshot` は `true` を返す。SSR とハイドレーションの最初の描画は `true` でそろい、ハイドレーションの後に実際の値へ変わる（描画中に `document` を読まないので SSR を壊さない）。hidden になったら SSE を閉じ、ポーリングを止める。visible に戻ったら開き直し、ポーリングはすぐ 1 回取得する。既存の `checkStale` の配線（`state-provider.tsx:151-162`）はそのまま。

### D10. レートの復元を最初の接続より先に行う

`RefreshRateProvider` の初期値は既定と `ready = false`（SSR とハイドレーションを一致させるため、描画中は `localStorage` を読まない）。`useEffect` で読み、検証してから `ready = true`。React は子の effect を親より先に実行するので、子（`StateStreamProvider` とポーリングの各コンポーネント）は `ready` を effect の条件と**依存配列**の両方に入れ、`ready` が true になった再実行で始める。

### D11. ポーリングの間隔と失敗の扱い

- 間隔は今のとおり「前回の取得の完了から次まで」。重ねない。`stop`・abort の後は取得も適用もしない。
- **失敗の待ち（決めた値）**：連続 n 回目の失敗の後は `min(intervalMs × 2^(n−1), 30_000)` 待つ。ただし `intervalMs` より短くしない（10 秒の選択なら 10 秒、20 秒、30 秒、30 秒…）。成功したら `intervalMs` に戻す。30 秒は SSE の再接続の上限（`state-stream.ts:88` の `RECONNECT_MAX_DELAY_MS`。PR 3 で export する）とそろえた。
- `startPolling` と `startRunLogUpdates`（`run-log-updates.ts:22-38`）に ready・visible の引数は足さない。呼ぶ側の effect が `ready && visible` のときだけ呼び、依存配列に `refreshRateMs`・`ready`・`visible` を入れ、変わったら古いハンドルを止めて張り直す。張り直しの最初の取得はすぐ。
- ポーリングの画面の遅れは「選んだ間隔＋取得の時間」。1 秒の選択でも取得が 300ms かかれば 1.3 秒周期になる。受け入れ条件 4 の「おおむね1秒」は SSE の画面の約束で、ポーリングの画面は「選んだ間隔＋取得時間」と書く（不変条件 9）。

### D12. requirements は 2.7 の読み取りの行を書き換え、第 5 章は注記だけ

- `docs/requirements.md:66`（2.7）の「バックエンドは Mac mini の takt-mcp から30秒ごとに状態を読み取る。」を、次に書き換える：「バックエンドは Mac mini の takt-mcp から状態を読み取る。画面が開いて更新間隔を求めている間は、その間隔（`minReadIntervalMs` 以上）で読む。画面が無いときは30秒ごと（設定の `intervalSeconds`）に読む。」2 章は要件の正なので書き換えてよい。
- `docs/requirements.md:90` は第 5 章を「質疑の記録、要件の正は 2 章」と定めているので、質問 12（`:104`）の回答は書き換えない。行の末尾に「（2.7 で置き換えた。issue #40）」と注記する。
- PR 3 で入れる（画面が実際に要求を出すのが PR 3 からなので）。

### D13. 初回の取得が失敗したときの「空の内容」

- `LiveActivity` の初期値の型は `{ response: GetActivityResponse; readAt: Date } | undefined`、`LiveRunDetail` は `GetRunDetailResponse | undefined`。
- `undefined` のときは、内容の部品（`ActivityContent`・`RunDetailContent`）に空の値を作って渡さない。見出し（「アクティビティ」、run 詳細は slug）と `BackendUnavailableNotice` だけを出し、ポーリングを始める。最初の成功で内容を出す。`RunDetailContent` の `taskName`・`project` が `undefined` で描かれる経路（`run-detail-content.tsx:39-55`）は作らない。
- 404 は今のとおり。初回の 404 はページの 404（`notFound()`）。ポーリング中の 404 は「この run は見つかりません」を出して、その run のポーリングを止める。

### D14. 切断の通知は Live の部品が持ち、Provider の外では使わない

- `LiveProjects`・`LiveActivity`・`LiveRunDetail` は、`BackendUnavailableOver` を自分で描く。ローダーが外側に静的な通知を残すと、回復しても消えないため。
  - `LiveProjects` は自分の RPC を持たないので、初期の失敗も含めて Context（`useLiveState()` の `failure`・`disconnected`・`snapshotLoaded`。layout の `loadServerState()` の結果から Provider が作る）だけから描く。props で失敗を受けない。
  - `LiveActivity`・`LiveRunDetail` は、自分の RPC（`GetActivity`・`GetRunDetail`）の初期の失敗だけをローダーから props で受け、その後はポーリングの結果で変える。backend 全体の切断は Context の `disconnected` から受ける。
- `useLiveState()` が Provider の外で例外を投げる今の守り（`state-provider.tsx:32-39`）は緩めない。本番ではいつも layout の Provider の中にあり、外で黙って既定値に落ちると配線の誤りが隠れるため。
- **ローダーを単体で描いていたテストの描き方（確かめた）**：捨てる worktree で試した。
  - 今の layout（`StateStreamProviderWithRefresh` が `useRouter` を呼ぶ）のままでは、`RootLayout` の要素を `renderToStaticMarkup` で描くと `invariant expected app router to be mounted` で落ちる。
  - `StateStreamProviderWithRefresh` を `StateStreamProvider` に替えた layout なら、`await RootLayout({ children, params })` の要素を描け、子の `useLiveState()` は layout の状態を受ける。
  - そこで、テストは `test/support` に置く 1 つの関数（layout と同じく `loadServerState()` の結果で `StateStreamProvider` を包んで描く）で描く。PR 2a の時点では layout がまだ `useRouter` を使うので、この関数が要る。PR 2b で `RootLayout` が描けるようになったら、ローダーを本物の `RootLayout` の中で描く試験を 1 本足し、layout の配線をそこで確かめる。期待は変えない。

### D15. run ログの読み位置は取り合わない

run ログの追記は `/api/runs/[slug]/updates` が画面の持つ cursor（`GetRunUpdatesRequest.epoch`・`after_sequence`、`proto/taktdash.proto:426-428`）を送って取る。読み位置は画面ごとなので、複数のタブや詳細のポーリングと取り合わない。backend の `RunLogHandles`（`run_progress.rs:643-675`）はプロジェクトごとに積み上げた集計で、`GetRunDetail` の呼び出しは同じ集計を `Mutex` で順に読むだけ。1 秒の詳細のポーリングが増やすのは、この `Mutex` の待ちと組み立ての費用（負荷の節）。run 詳細の 1 ページで定期的に読むのは 2 本（詳細とログ）のままにし、1 本にしない。#80 の設計と重なるので #80 の書き直しで扱い、この issue では 2 本の応答時間を L4 で測る（決め 3）。`LiveRunLog` は詳細の応答の `logTail` で積んだ行を初期化しない（`live-run-log.tsx:40-43,58-76` の今の契約）。

### D16. poller の実効間隔の式

`effective = 生きている要求が無ければ intervalSeconds、あれば min(intervalSeconds, max(minReadIntervalMs, 生きている要求の最小値))`。生きている要求は、期限（`deadline`）が `now` より後の登録。期限ちょうどの登録は切れたものとして数えない。

- 下限 `minReadIntervalMs` は画面の要求にだけ掛かる。運用者が `intervalSeconds` を下限より短く設定した場合は、設定のほうが速い。
- 例：`intervalSeconds = 30`、下限 5 秒、要求 1 秒 → 5 秒。`intervalSeconds = 1`、下限 5 秒、要求 1 秒 → 1 秒。要求なし → `intervalSeconds`。
- 今の設定（`minReadIntervalMs` なし、`intervalSeconds` は 1 以上の整数秒）では、要求なしの間隔は今と同じ。
- 接続の送信間隔（`applied = max(要求, minReadIntervalMs)`）は `intervalSeconds` に左右されない。`intervalSeconds` を下限より短くした場合、poller は速く読むが、各接続へは下限の間隔で送る（Quint の `configFasterThanFloorTest`）。

### D17. 待ちと期限の時計は `tokio::time::Instant` 1 つにする

- 待ち・寿命・送信の期限に使う時計は `tokio::time::Instant` だけにする。
  - `RefreshDemand` の操作は時刻を引数で受ける（`acquire(screen, attempt, rate, now)`、`touch(&guard, applied, now)`、`effective(config, now)`）。
  - `watch_state` の送信タスクは `tokio::time::Instant::now()` を渡し、`next_send` と寿命の期限を `sleep_until` で待つ。
  - poller の待ち（`run_until_stopped`）も同じ時計にする。今は `poller_stop.rs:76` の `started = Utc::now()` と `:157-162` の `sleep_until_next` が壁時計の `Utc::now()` で残りを計算しているので、`started = tokio::time::Instant::now()` と `tokio::time::sleep_until(started + effective)` に替える。
- テストは `tokio::time::pause()`・`advance()` で、登録簿・送信タスク・poller の待ちを同じ時計で進める（E21・E22・E24・E26・E29）。
- 記録の時刻は壁時計のまま変えない。状態の `read_at`、出来事の時刻、`TaktDashServiceImpl` の `clock`（`grpc/mod.rs:137,186-219`、`DateTime<Utc>`）、poller の `read_cycle_once_at(now: DateTime<Utc>)` は、画面に出す時刻と保存する時刻のためのもので、待ちには使わない。

### D18. 内容の部品は画面の型を受ける

- `ProjectsContent` と `ActivityContent` の `state` を、proto の `DashboardState` から画面の型（`restore-state.ts` の `DashboardState`）に変える。`GoalsContent` と同じ形。`LiveProjects`・`LiveActivity` は Context の値をそのまま渡す。
- proto の状態を持つ呼び出し元は、境界で `dashboardStateFromProto` を通す。`app/projects/page.tsx`・`app/activity/page.tsx` の `state` prop（テストの入口）は page の中で変換するので、page を描くテスト（`design.test.ts:311,522,555,631,677`、`preview-truncation.test.ts:130`、`projects-issue78.test.ts`、`pages.test.ts:294`）は変えない。
- **確かめた**：捨てる worktree で型を変えて `tsc -p tsconfig.test.json --noEmit` を流した。落ちたのは `activity-content.tsx:227`（`serverNames` の鍵が `ServerId` の印付きになり、`event.serverId` の文字列で引けない。鍵を文字列にして直す）と、内容の部品を直接描くテスト 5 か所（`navigation.test.ts:150`、`activity-content.test.ts:30,60`、`issue-315-activity-days.test.ts:99`、`issue-315-wording.test.ts:590`）。`issue-317-a11y.test.ts:49,52` は型のゆるい補助で描くので型では落ちないが、同じく変換を通す。
- 変換か props の変更かは「変換を境界で 1 回」に決めた。props を `servers` などに分ける案は、`ProjectsContent` が状態のほぼ全部を使うので分ける意味が無い。

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
| 寿命の下限 15 秒 | `refresh.rs` の `TTL_MIN = WATCH_RESEND_INTERVAL`（`grpc/mod.rs:80` の定数を参照し、数値を書き写さない） | Quint の `TTL_MIN` | `refresh_demand_test.rs`（1 秒の要求の寿命が 15 秒）、Quint の `ttlStaysUnderWatchdogTest` |
| 寿命の上限 30 秒 < 見張り 40 秒 | `refresh.rs` の `const _: () = assert!(max(TTL_FACTOR * MAX_REFRESH_MS, TTL_MIN_MS) < 40_000)` | frontend の `STATE_STREAM_STALE_TIMEOUT_MS = 40_000`（`state-stream.ts:52`） | frontend の `test/state/refresh-rate.test.ts` が `STATE_STREAM_STALE_TIMEOUT_MS > Math.max(3 × max(RefreshRate), 15_000)` を確かめる（3 と 15,000 は frontend のテストに書く。backend の定数は TS から読めないため、両側で同じ式を 1 本ずつ持つ） |
| 要求なしの再送 15 秒 | `grpc/mod.rs` の `WATCH_RESEND_INTERVAL`（変えない） | Quint の watchdog の `RESEND_INTERVAL = 3`（5 秒×3） | 既存のまま |
| 要求ありの送信の列 1 件 | `grpc/mod.rs` の `WATCH_DEMAND_CHANNEL_CAPACITY` | なし | `watch_state_test.rs` |
| 失敗の待ちの上限 30 秒 | `polling.ts` が `RECONNECT_MAX_DELAY_MS` を import | `state-stream.ts` | `polling.test.ts` |
| `localStorage` のキー | `refresh-rate.ts` の `REFRESH_RATE_STORAGE_KEY` | e2e の spec（文字列） | e2e が再読み込み後の復元を確かめる |
| #130 の合格線（L1 の 1.5 秒・3 秒・300 本、L2 の 10%、L3 の 50%・10%、L4 の 1.5 倍。L5 は記録だけ） | この設計の「負荷の合格線」 | PR 1b の本文、#130 | 実測の記録（コードには置かない） |

## 変える場所（PR ごと）

### PR 1a：proto と型の追従（大きさ 2）

proto に欄を足し、型で落ちる所だけを直す。backend は新しい欄を読まず、どの接続も要求なしで今と同じに動く。`applied` はどこでも 0。

- `proto/taktdash.proto`：`RefreshRate`（D5）、`WatchStateRequest { RefreshRate refresh_rate = 1; string screen = 2; uint64 attempt = 3; }`（今は `:580` の空の message）、`DashboardState.applied_refresh_interval_ms = 9`（uint32、今は `:279-288`）。
- `make proto-gen` の生成物 `frontend/web/src/proto/taktdash.ts` を同じ PR でコミットする。
- **型で落ちる所（捨てる worktree で、proto だけを変えて確かめた全部）**
  - `backend/src/grpc/assembly.rs:229`：`DashboardState` の構造体 literal に `applied_refresh_interval_ms: 0`。
  - `backend/tests/watch_state_test.rs`（58・99・130・163・217・260・311・347・376・443・503 行）、`backend/tests/build_info_test.rs:119`、`backend/tests/write_grpc_test.rs:792`：`WatchStateRequest {}` を `WatchStateRequest::default()` に。
  - `frontend/cli/src/tests/fixtures.rs:32`（`sample_overview`）と `frontend/cli/src/write_command_tests.rs:755`（`pending_state`）：`DashboardState` の構造体 literal に `applied_refresh_interval_ms: 0`。`frontend/cli` も `build.rs:2` で同じ proto をビルドする workspace の member なので、ここが落ちる（E0063。3 回目の読みが `cargo check --workspace --tests --keep-going` で確かめた）。CLI の本体は `WatchState` を引数なしで呼ぶので変わらない。
  - `frontend/web/src/grpc/client.ts:357`：`client.watchState({} satisfies WatchStateRequest)` を `watchState(request: Partial<WatchStateRequest> = {})` と `client.watchState(WatchStateRequest.fromPartial(request))` に（この PR では呼び出し元は引数なし）。`fromPartial` は値なので、`client.ts:26` の `type WatchStateRequest`（`client.ts:3-27` の `import { TaktDashServiceClient, type … } from "../proto/taktdash"` の中で型だけとして読んでいる名前）から `type` を外し、値として import する（型だけのままでは TS1361 になる）。
  - `frontend/web/src/state/restore-state.ts:335-360`：`restoreDashboardState` は `appliedRefreshIntervalMs: asNumber(raw.appliedRefreshIntervalMs)`（無ければ 0）、`dashboardStateFromProto` は `state.appliedRefreshIntervalMs` を返す。
  - `backend/tests/proto_contract.rs:127,157` は `..Default::default()` を使うので落ちない。frontend のテストは `DashboardState.create` で作るので落ちない（`tsc -p tsconfig.test.json` で確かめた）。
  - 一覧が全部であることの確かめ（3 回目の読みが捨てる作業ツリーで流した）：proto と上の直しで `cargo check --workspace --tests` はエラー 0、`tsc`（本体・テスト）もエラー 0、`npm test` は 1061/1061。`make done` の clippy・test は `frontend/cli` も見るので、漏れがあれば PR 1a で落ちる。
- `test/state/restore-state.test.ts`：JSON と proto のどちらの経路でも `appliedRefreshIntervalMs` が読め、無ければ 0（E30）。`:132` の 2 経路の `deepEqual` は両方に欄が増えるので緑のまま。

### PR 1b：backend の要求と BFF の転送（大きさ 5）

backend

- 新規 `backend/src/refresh.rs`
  - `RefreshDemand`（`Arc<Mutex<Inner>>` と `tokio::sync::watch::Sender<()>`）。`Inner` は `entries: BTreeMap<ScreenId, Entry>`、`min_read: Duration`、`next_id: u64`。`Entry { id: DemandId, attempt: u64, rate: Duration, deadline: Instant, superseded: watch::Sender<bool> }`。時刻は `tokio::time::Instant`（D17）。
  - `ScreenId` は 32 文字の 16 進を検証した newtype。`Attempt` は 1 以上。
  - `acquire(screen, attempt, rate, now) -> Result<DemandGuard, Stale>`：同じ画面にもっと大きい試行があれば `Stale`。それ以外は登録を置き換え、古い登録の `superseded` に通知する。
  - `touch(&guard, applied, now) -> Result<(), Expired>`：自分が持ち主で期限の前なら `deadline = now + ttl(applied)`。期限を過ぎていれば延ばさずに `Err(Expired)`（D7）。
  - `ttl(applied) = max(TTL_FACTOR × applied, TTL_MIN)`。
  - `applied_for(&guard) -> Duration`：`max(rate, min_read)`（D6）。
  - `set_min_read(Duration)`：poller から。
  - `effective(config: Duration, now) -> Duration`：D16。期限が `now` 以前の登録は数えない。
  - `subscribe() -> watch::Receiver<()>`：登録・解放・下限の変更で通知する。
  - `DemandGuard`：`Drop` で、登録がまだ自分の `DemandId` を指すときだけ外し、通知する。`deadline()`・`is_superseded()`・`superseded()`（待てる future）を持つ。
  - `TTL_FACTOR = 3`、`TTL_MIN`、`MIN_REFRESH_MS`、`MAX_REFRESH_MS`、`const` assert（数値の置き場所）。
- `backend/src/lib.rs`：`pub mod refresh;`（`lib.rs:11-25` のほかのモジュールと同じく `pub`。`backend/tests` と `main.rs` は別の crate なので、`pub` でないと `RefreshDemand` が見えない）。
- `backend/src/grpc/mod.rs`
  - `WATCH_RESEND_INTERVAL`（`:80`、今は private）を `pub(crate) const` にする。`refresh.rs` の `TTL_MIN = crate::grpc::WATCH_RESEND_INTERVAL` が数値を書き写さずに参照するため。値は変えない。
  - `TaktDashServiceImpl` と `ServiceDeps`（`:156-164`）に `refresh_demand: RefreshDemand`。
  - **組み立てを差し替える口**（E22 の (2)・E26 のため）：今の `watch_state` は固定の関数 `assemble_runs`（`:233`）を `assemble_state_for` に渡す（`:609`・`:646`。unary の `:323`・`:351` も同じ）。注入の口は `with_clock`・`with_run_detail_budget`（`:186`・`:204`）だけで、試験から組み立てを止められない。そこで `TaktDashServiceImpl` に `runs_assembler: RunsAssembler`（`type RunsAssembler = Arc<dyn Fn(&[model::ProjectView], &StateStore, DateTime<Utc>) -> Vec<Run> + Send + Sync>`、既定は `Arc::new(assemble_runs)`）と `pub fn with_state_assembler(mut self, assembler: RunsAssembler) -> Self` を足し、4 か所の呼び出しはこの欄を使う。`assemble_state_for`（`assembly.rs:220-225`）は `impl Fn` を受けるので変えない。試験は、`std::sync::Barrier` などで止められる関数を渡して、`blocking(...)` の中で組み立てを止める。本番の振る舞いは変わらない。
  - `watch_state`（`:598-665`）：`refresh_rate` を検証する（未知の値は `INVALID_ARGUMENT`）。`refresh_rate` が 0 なら今の処理のまま（D4）。0 でなければ `screen` は 32 文字の 16 進、`attempt` は 1 以上でなければ `INVALID_ARGUMENT`。
  - 要求のある接続：購読 → 初期の組み立て → `applied` を入れて送信 → `acquire` → 送信タスクへ guard を移す。`acquire` が `Stale` なら、**handler から `Err(Status::aborted(…))` を返す**（`rx` を返さずに捨てる）。初期状態はすでに列（1 件）に入っていて誰も読まないので、そこへ `Status::aborted` を `tx.send` すると完了せず handler が止まる（今の初期送信は `:606-620` のとおり handler の中で `rx` を返す前に送る）。`Err` を返せば、列の中身は `rx` と一緒に捨てられ、Next には呼び出しの失敗（`ABORTED`）として届く。送信タスクの `select!` は `sender.closed()`・停止・購読の終端・`guard.superseded()`・`sleep_until(guard.deadline())`（寿命切れ）・`sleep_until(next_send)`・変化の受信（dirty にするだけ）。`next_send` の分岐で組み立て、**組み立ての後に寿命と置き換えを確かめ**（D7）、送信は `sender.send` を寿命の期限・置き換え・切断・停止と `select!` し、完了で `touch`（`Err(Expired)` なら `return`）。
  - 列の大きさは要求ありで 1（D7）、要求なしで今の 16。
- `backend/src/poller.rs`：`Poller` に `refresh_demand`。`with_refresh_demand` で共有する（テストで指定しなければ要求なしの `RefreshDemand`）。`read_cycle_once_at` で `refresh_demand.set_min_read(config.min_read_interval)` を呼び、`CycleReport.interval = refresh_demand.effective(Duration::from_secs(config.interval_seconds), Instant::now())`。
- `backend/src/poller_stop.rs`：`run_until_stopped`（`:74-105`）の待ちを、`sleep_until(started + effective)` と `refresh_demand.subscribe().changed()` の `select!` にする。`started` は `tokio::time::Instant`（D17。`:76` と `:157-162` の `Utc::now()` をやめる）。通知を受けたら今の `effective` で期限を計算し直す（短くなれば切り上げ、長くなれば待ち直す）。設定の失敗の周期は D6。停止と `CYCLE_GRACE` は変えない。
- `backend/src/main.rs:82`：`RefreshDemand::new()` を 1 つ作り、`ServiceDeps` と `Poller` へ同じものを渡す。
- `ServiceDeps` を作るほかの 2 か所（捨てる worktree の `cargo check --tests` で確かめた全部）：`backend/tests/grpc_harness/mod.rs:456`、`backend/tests/watch_state_test.rs:301`。どちらも `refresh_demand: RefreshDemand::new()`（要求なしの登録簿。振る舞いは今と同じ）。
- `backend/src/config.rs`：`minReadIntervalMs`（既定 `DEFAULT_MIN_READ_INTERVAL_MS`、範囲は `MIN_REFRESH_MS..=MAX_REFRESH_MS`、外れは `ConfigError::Invalid`）。`DEFAULT_INTERVAL_SECONDS = 30` は変えない。`taktdash.config.example.json` に `"minReadIntervalMs": 1000`。

BFF

- `frontend/web/src/grpc/client.ts`：`watchState({ refreshRate, screen, attempt })` を呼び出し元から渡す。
- `frontend/web/app/api/state/route.ts`：`refresh_ms` を `parseRefreshRateParam`（`src/state/refresh-rate.ts`）で読む。`RefreshRate` の 4 つの値のどれかならその値。それ以外（無い・空・`999`・`1e3`・負・壊れた値・繰り返し）はすべて `REFRESH_RATE_UNSPECIFIED`（要求なし）にし、ストリームを止めない。要求あり（0 でない）で `parseStreamTicket` が無いときも要求なしにする（backend の `INVALID_ARGUMENT` を作らない）。`screen`・`attempt` は ticket から渡す。`screenStreams` の扱いは変えない。
  - 不正な値を既定の 1 秒にしない理由：壊れた URL や古い画面が、気づかれないまま poller を 1 秒にしてしまうため。要求の無い接続は今と同じに動くので、画面は壊れない。
- `frontend/web/src/state/refresh-rate.ts`（新規、この PR では BFF の解析と選択肢だけ）：`REFRESH_RATE_OPTIONS`（生成 enum から）、`DEFAULT_REFRESH_RATE`、`parseRefreshRateParam`。

この PR の後も画面は `refresh_ms` を付けないので、どの接続も要求なしで今と同じに動く。

**受け入れ（#130 の実測）**：この PR をマージして本番へ展開した後に、「負荷の合格線」の手順で測り、結果を #130 と #40 に書く。要求の無い接続は変わらないので、測る前に展開してよい。合格線を外れても 1b は戻さない（画面はまだ要求を出さない）。外れたら PR 3 の前に読みを減らす PR を入れる。

### PR 2a：アクティビティと run 詳細を要素単位にする（大きさ 5）

`router.refresh()` はまだ残す。Live の部品は初期値しか props から読まないので、再取得されても表示は二重にならない。ただしこれは表示の話で、読みは重なる：2a の間は SSE の受信ごとの `router.refresh()` が RSC の `ActivityLoader`（`GetActivity`）と `RunDetailLoader`（`GetRunDetail`）を読み直し、そこに 1 秒のポーリングの `/api/activity`・`/api/runs/[slug]` が加わる。2b でページの再取得をやめるまでの一時的な増え方で、2a の負荷の測りに入れる（影響の「負荷」）。

- 新規 `app/api/activity/route.ts`：`export const dynamic = "force-dynamic"`。`getBackendClient().getActivity()` を `GetActivityResponse.toJSON` で返す。成功も失敗も `Cache-Control: no-store`（`app/api/runs/[slug]/updates/route.ts:8,14` と同じ形）。失敗は既存の `toBackendUnavailableResponse`。成功を `lastActivity` に覚える。
- 新規 `app/api/runs/[slug]/route.ts`：同じく `dynamic` と `no-store`。`getRunDetail(slug)` を JSON で返す。`RunNotFoundError` は 404、`BackendUnavailableError` は 503。成功を `lastRunDetails` に覚える。
- `src/state/last-loaded.ts`（既存）：`lastActivity` と `lastRunDetails` を、すでにある `lastGoalDetails`・`lastTaskDetails`（`:43-44`。Route Handler とローダーが共有する入れ物。`app/api/tasks/[project]/route.ts:4`、`task-detail-loader.tsx:10`）の隣に並べる。今は `activity-loader.tsx:11` と `run-detail-loader.tsx:27` の module 変数なので、そこから移して Route Handler とローダーが同じものを使う。作り方は同じ `createLastLoaded`。新しいファイルは作らない。
- `activity-content.tsx`：`state` を画面の型に（D18）。`:227` の鍵を文字列に。`app/activity/page.tsx` は `dashboardStateFromProto` で変換する。
- 新規 `live-activity.tsx`：初期値（D13）と、`GetActivity` の初期の失敗を props で受け、`useLiveState()` の `state`（サーバー名の解決）と `disconnected` と合わせて描く。`/api/activity` を 1 秒で読む（この PR ではタスク詳細などと同じ固定 1 秒。PR 3 で選んだレートにする）。
- 新規 `live-run-detail.tsx`：初期値（D13）と `GetRunDetail` の初期の失敗を受け、`/api/runs/${slug}` を 1 秒で読む。`serverName` は `useLiveState()` の `state.servers` から引く（**prop では受けない**）。`LiveRunLog` は今と同じ `key={detail.runSlug}`。
- `activity-loader.tsx`：`GetActivity` だけを読み、`<LiveActivity initial=… initialFailure=… />` を返す。`loadServerState()` は呼ばない。
- `run-detail-loader.tsx`：`GetRunDetail` を読み、`<LiveRunDetail initial=… initialFailure=… />` を返す。サーバー名のための `loadServerState()`（`:53-59`）は消す（サーバー名は Context から。`serverName` を決める場所を `LiveRunDetail` の 1 つにする）。404 の `notFound()` は残す。
- `src/state/restore-detail.ts`：`restoreActivityResponse`、`restoreRunDetail` を足す（既存の `restoreGoalDetail` と同じ作り。ブラウザで `@grpc/grpc-js` を読み込まないよう生成コードの `fromJSON` は使わない）。
- 新規 `test/support` の描画の関数（D14）：layout と同じく `loadServerState()` の結果で `StateStreamProvider` を包んで描く。
- 既存テストの直し（「既存テストで変わるもの」の 2a の行）。e2e の `flow-9-run-detail-live-log.spec.ts`。

### PR 2b：ページ単位の再取得をやめる（大きさ 3）

- `state-stream.ts`：`createStateHandlers` から `revalidate` を消す。
- `state-provider.tsx`：`revalidate` prop と `useRouter`、`StateStreamProviderWithRefresh` を消す。`layout.tsx` は `StateStreamProvider` を直接使う。
- `projects-content.tsx`：`state` を画面の型に（D18）。`app/projects/page.tsx` は `dashboardStateFromProto` で変換する。
- 新規 `live-projects.tsx`：`useLiveState()` の `state`・`snapshotLoaded`・`disconnected`・`failure` から `ProjectsContent` と `BackendUnavailableOver` を描く（初期の失敗も Context から。D14）。時計（待ち時間の表示）は `state.readAt ?? new Date()` を起点に 1 秒で進める。取得はしない。
- `projects-loader.tsx`：`measurePage("/projects", async () => <LiveProjects />)`。`loadServerState()` は呼ばない（Provider が layout の結果を持つ）。RPC は出さないので `rpcs=` のまま。
- ローダーを本物の `RootLayout` の中で描く試験を 1 本足す（D14）。
- 既存テストの直し（「既存テストで変わるもの」の 2b の行）。

### 読みを減らす PR（大きさ 5。合格線を外れたときだけ）

**入れる条件**：PR 1b の後の実測（「負荷の合格線」）が外れたとき（決め 1）。既定の 1 秒は変えない（受け入れ条件 1）。PR 3 の前に入れ、入れた後に L1〜L5 を測り直す。PR 1b は外れてもマージしてよい（決め 6）。

**中身**（決め 1。「負荷の合格線」の「外れたときにすること」の 1〜3 のとおり。細部はこの PR の設計で書く）：

1. 2 段の読み。要求がある間に速く（`effective` で）読むのは `takt_list_tasks` と判断待ちの 3 つ（`takt_list_goal_questions`・`takt_list_standalone_inquiries`・`takt_list_goal_registration_proposals`）だけにする。`takt_list_goals`・`takt_get_goal`（1 周期 36 回）は遅い段（`intervalSeconds`）で読む。
2. `takt_get_goal` は、一覧の更新時刻が変わったゴールだけを読む（差分読み）。
3. 1 プロジェクトの中の呼び出しを並べる（今は直列）。

変える場所の見込み：`poller.rs`（`read_cycle_once_at` を 2 段にし、段ごとの道具の内訳をログに出す）、`poller_stop.rs`（待ちの期限を段ごとに持つ）、`backend/tests/poller_test.rs`、Quint。

**この PR の設計で合わせるもの**（3 回目の読みの I）：ゴールを遅い段で読むと、次の 3 つが嘘になるか、モデルが本番を表さなくなる。

- **D6 の通知文**：1 つ目の「概要・判断待ち・プロジェクト・ゴール・アクティビティの内容はこの間隔で変わります」と、3 つ目の「読み取りに N 秒かかっています」は、ゴールの一覧にとって嘘になる。ゴールの一覧（ゴールの画面と概要のゴールの欄）は遅い段の間隔で変わる、と分けて言う文に直す。ゴール詳細（BFF が要求のたびに takt を読む）は選んだレートのまま。`readLag` の N を、どちらの段の `last_read_at` で出すかも決める。
- **受け入れ条件 2 の言い方**：「どのページでも表示中の情報が選んだレートで更新される」は、ゴールの一覧が遅い段になる分だけ満たさなくなる。受け入れの対応表にどう書くかは受け入れ条件の読み方に触れるので、この PR の設計で takt-dash中将の判断を受ける（オーナーへは元帥経由。決め 1 と同じ経路。作業者からオーナーには聞かない）。
- **Quint**：今の `pollerRead` は 1 本の読みで、2 段を表していない。速い段と遅い段を別の遷移と別の `lastReadAt` にし、`pollerNotLate` を速い段にかけ、遅い段には「`intervalSeconds` を越えて待たない」を足す。2 段を 1 段に戻した変異などで、新しい不変条件が恒真でないことを `mutants.sh` で確かめる。

**それでも 1.5 秒を切らないとき**（決め 1）：takt-mcp 側の変更（軽い「変化の有無」の道具など）を別の issue にし、受け入れ条件 4 の読み方は元帥経由でオーナーに仰ぐ（takt-dash中将からオーナーには聞かない）。PR 3 はその答えが出るまでマージしない。

### PR 3：画面のレートの選択と、レートに従う取得（大きさ 8）

**マージの条件**：PR 1b の後の #130 の実測が「負荷の合格線」を満たしていること（外れたら、読みを減らす PR が先に入って満たしていること）。合格線で止めるのはこの PR だけ（決め 6。1 秒を利用者に出す所だから）。

- `src/state/refresh-rate.ts`：`parseStoredRefreshRate(value: unknown): RefreshRate`（4 つのどれかでなければ既定、例外を投げない）、`refreshRateLabel`、`REFRESH_RATE_STORAGE_KEY`、読み取りの遅さの判定 `readLag`（D6 の 3 つ目の通知。純粋関数）。
- 新規 `components/refresh-rate-provider.tsx`：`RefreshRateProvider`・`useRefreshRate()`（`refreshRate`・`ready`・`setRefreshRate`）。D10。`setRefreshRate` は state を変え、`localStorage.setItem` を試す（`SecurityError`・容量不足でも state は変える）。`storage` イベントで同じ origin の別のタブの変更を受ける。
- 新規 `components/page-visibility.ts`：`usePageVisible()`（D9。`useSyncExternalStore`、server snapshot は `true`）。
- 新規 `components/refresh-rate-select.tsx`：`<label for="refresh-rate">更新間隔</label>` と `<select id="refresh-rate">`。
- `app/layout.tsx`：`RefreshRateProvider` で `StateStreamProvider` を包む。
- `state-provider.tsx`：`screenId` と `nextAttempt`（D8）。`ready && visible` のときだけ `openStateStream({ url: "/api/state?refresh_ms=" + refreshRate, screenId, nextAttempt, … })`。依存は `[refreshRate, ready, visible]`。Context に `liveStateReceived: boolean`（SSE で一度でも状態を受けたか。必須の欄）を足し、`reduceLiveState`（`:65-80`）は `state` で `true`、`error` で前の値を保つ。
- `state-stream.ts`：`OpenStateStreamOptions` に `nextAttempt?: () => number`。渡されたらそれで URL を作る。`screenId` の解決は入口の 1 回のまま（D8）。`RECONNECT_MAX_DELAY_MS` を export する。
- `sidebar.tsx`・`live-sidebar.tsx`：「更新間隔」の節と D6 の通知（3 つ目の「読み取りに N 秒かかっています」を含む。`live-sidebar.tsx` は受信ごとに `readLag` を呼び、前の値を `useRef` に持つ）。`refreshRate?`・`appliedRefreshIntervalMs?`・`liveStateReceived?` は任意の prop で、渡さない既存のテストは今のまま。
- `polling.ts`：D11 の失敗の待ち。
- `live-task-detail.tsx`・`live-goal-detail.tsx`・`live-conversation.tsx`・`live-run-log.tsx`・`live-activity.tsx`・`live-run-detail.tsx`：`intervalMs = refreshRate`、effect の条件と依存に `ready`・`visible`（D10・D11）。会話の `setInterval` は `startPolling` に置き換える。
- `docs/requirements.md`：D12。
- 新規 e2e `e2e/tests/issue-40-refresh-rate.spec.ts`（E13〜E17）。この PR の時点では e2e の backend は 1 秒のまま。

### PR 4：e2e の既定を 30 秒にする（大きさ 2）

**受け入れの先頭**：既定 30 秒で `make e2e` を全部流し、落ちた spec から「1 秒が要る spec」を実測で決める。下の静的な一覧は予想で、実測が正とする。

- `e2e/support/stack.ts`：`StartStackOptions` に `intervalSeconds?: number`（既定 30）と `minReadIntervalMs?: number`（既定なし）を足し、`buildConfig` へ渡す。`stack.ts:308` の `intervalSeconds: 1` を `input.intervalSeconds` にする。
- `startStack` を直接呼ぶ 5 本（`decisions-layout`・`issue-311-confirm-dialogs`・`issue-315-narrow`・`overview-responsive`・`screenshot-states`）は、新しい口を渡さず既定を使う。
- `e2e/support/test.ts`：名前付きの stack（13・51・71・82 行の `twoProjectStack`・`longServerStack`・`longTitleStack`・`conversationStack`、103 行の `stack`、115 行の `isolatedStack`）は既定（30 秒）のまま。新しい fixture `fastPollIsolatedStack`（`startStack({ intervalSeconds: 1 })`）を足す。
- 実測で 1 秒が要ると決まった spec を `fastPollIsolatedStack` へ移し、理由をコメントに書く。
- E18 は**新しい spec ファイル** `e2e/tests/issue-40-default-interval.spec.ts` に置く（PR 3 の `issue-40-refresh-rate.spec.ts` を変えないので、PR 4 は PR 3 の spec の中身に依存しない）。

## 負荷の合格線（#130。PR 1b の後に測り、PR 3 で止める）

### 測り方

- PR 1b を本番へ展開した後、本番の設定（上の「本番の読み取りの実測」と同じ）のまま測る。backend は本番と同じ debug ビルド。
- **測る前に決めるもの**（3 回目の読みの M）
  - 対象のプロセス：takt-mcp の HTTP サーバーは、backend の設定の `mcpServers.<名前>.url` のポート（例の設定では `43117`）で待ち受けているプロセス（`lsof -nP -iTCP:<ポート> -sTCP:LISTEN -t`）。backend は設定の `listen`（無ければ `config.rs` の `DEFAULT_LISTEN`）のポートで待ち受けているプロセス（同じ `lsof`）。PID を記録に書く。
  - L4 で読むもの：区間の前に、5 つのプロジェクトの `/api/tasks/[project]` を 3 回ずつ読み、応答の p50 がいちばん遅いプロジェクトを選んで全区間で使う。run は、そのプロジェクトのいちばん新しい run の slug を 1 つ選んで全区間で使う。
- **区間**：1. 要求なし（基準）、2. 1 秒の要求 1 本、3. 1 秒の要求 3 本。それぞれ 10 分。
  - 要求は `curl -sN --max-time 600 -o /dev/null -w '%{size_download}\n' "http://127.0.0.1:3000/api/state?refresh_ms=1000&screen=<32 桁の 16 進>&attempt=1"` で出し、読み続ける（読まないと列が詰まり、寿命で要求が外れる）。区間 3 は `screen` を変えた `curl` を 3 本。`size_download` は L5 に使う。
- **順序（3 回目の読みの J）**：1 → 2 → 3 → 1 → 2 → 3 のように、区間を交互に **2 回以上** 繰り返す（合わせて 60 分以上）。takt のエージェントの負荷は時間で変わるので、区間をまたぐ比べ方（L2・L4）は同じ回の中の区間どうしで行い、**どの回でも満たすことを合格とする**。1 回でも外れれば不合格。
- **記録**（既存のものだけ）：backend のログの周期の行（`poller.rs:384-393` の「定期読み取りの1サイクルを終えました」。時刻・`cycle_ms`・`tool_calls`・道具ごとの内訳）、区間の始めと終わりの `ps -o time= -p <PID>`（backend と takt-mcp の HTTP サーバー）、L4 の応答時間、L5 の `size_download`。
- **L4 の読み方**：各区間で、10 秒ごとに次を 1 回ずつ読み、`curl -w '%{time_total}'` で応答時間を記録する（区間あたり各 60 本。p95 は上から 3 本目）。
  - `/api/tasks/[project]`（タスク詳細の読み。takt-mcp を読む）。
  - `/api/runs/[slug]/updates`（run 詳細のログの読み。今もある。`epoch`・`after` を付けない初回の形で読む。`route.ts` は両方無ければ位置なしとして受ける）。
  - `/api/runs/[slug]`（run 詳細の読み。PR 2a で入るので、PR 3 の前の測り直しから）。
  - run 詳細の 2 本のポーリングは今は 1 本にしない（決め 3。#80 の書き直しで扱う）ので、2 本とも L4 で見る。
- 結果は、回ごと・区間ごとに分位と本数を書く（本番の周期の裾は重く、最大 242 秒の周期がある）。
- **測り直し**：読みを減らす PR を入れたとき（入れたなら）と、PR 3 をマージする前（画面からの要求で。PR 2a の後なので `/api/runs/[slug]` も入る）。

### 合格線

| # | 何を | 合格線 | 理由 |
| --- | --- | --- | --- |
| L1 | **周期の間隔**：区間 2 の poller の周期の開始の間隔（周期の行の時刻から `cycle_ms` を引いた開始どうしの差） | p50 ≤ 1.5 秒、p95 ≤ 3 秒。区間 2 ごとに周期が 300 本以上。**300 本に届かなければ不合格**（10 分で 300 本は平均 2 秒以下の周期。届かないのは周期が遅いということなので、判定できないではなく外れとして扱う） | L1 が測るのは poller の周期の間隔だけ。SSE の画面の鮮度は、これに送信の間隔（`applied` 以下。1 秒の要求で 1 秒以下）と組み立て・送信・描画が足される（不変条件 9）。合格線の数値は決め 1 のまま。周期は休まず続けて回るので、間隔は `max(cycle_ms, 1 秒)` に近い |
| L2 | 読みがタブに比例しない：同じ回の区間 2 と区間 3 の 1 秒あたりの呼び出し数（周期の行の `tool_calls` の合計 ÷ 区間の秒数） | 差が 10% 以内。かつ、周期が重ならない：どの周期の開始（時刻 − `cycle_ms`）も、前の周期の行の時刻より後 | 設計の約束（poller は 1 つで、要求の最小値で読む。D16）。前の版の「1 秒あたりの呼び出し ≤ 1 周期の呼び出しの最大」は、ほぼいつも成り立ち、重ならないことの確かめにならなかったので、周期の行の開始と終わりを直接比べる形に替えた |
| L3 | CPU：区間 2 の 10 分（`ps -o time=` の差 ÷ 600 秒） | takt-mcp の HTTP サーバーが 1 コアの 50% 以下、backend が 1 コアの 10% 以下 | この Mac mini の主な仕事は takt のエージェントで、ダッシュボードの読み取りが使うのは半コアまでとした。今の 4.3%・0.5% のおよそ 10 倍・20 倍 |
| L4 | 詳細の読みが遅れない：同じ回の区間 2 と区間 1 の応答時間 p95 の比（`/api/tasks/[project]`、`/api/runs/[slug]/updates`、PR 2a の後は `/api/runs/[slug]`。それぞれで） | どれも 1.5 倍以下 | タスク詳細・ゴール詳細・会話・run 詳細は、要求のたびに読む。前の 3 つは同じ takt-mcp の HTTP サーバーを読み、run 詳細は backend の集計（D15）を読む。判断待ちの回答も同じサーバーへ書くので、本番で書き込みを測る代わりに L4 で見る |
| L5 | 送る大きさ：区間 2・3 の 1 接続あたり毎秒のバイト数（`size_download` ÷ 600 秒。SSE として Next が画面へ送る大きさ） | **合格線は置かず、値を記録する**（決め 4） | 要求のある接続には、変化が無くても間隔ごとに `DashboardState` 全体が届く。軽くするか（変化が無ければ送らない、`read_at` だけの軽いイベント）は、記録を見て別に決める（D7 の寿命の延ばし方と画面の見張りに触れるため） |

### 今の実測からの見込み

- L1 は外れる見込みが高い。1 秒の要求で周期は休まず回るが、1 周期の所要は p50 9.4 秒・p95 43 秒で、合格線の 6 倍以上。
- L3 は、今の読み方のままなら通る見込み（呼び出しは約 8.2 回/秒、今の約 9.5 倍。takt-mcp の CPU を線形に見積もると 1 コアの約 40%）。
- 読み方を変えずに周期を 1 秒にできたとしても、呼び出しは約 77 回/秒（今の約 90 倍）になり、L3 は外れる見積もり。**今の読み方（1 周期 77 回）では、L1 と L3 を同時に満たす周期は無い。**

### 外れたときにすること

既定の 1 秒は変えない（受け入れ条件 1。決め 1）。PR 3 の前に「読みを減らす PR」（大きさ 5。「変える場所」の節）を入れ、L1〜L4 を満たすまで PR 3 をマージしない。PR 1b は戻さない（決め 6）。中身は道具ごとの内訳からの次の 1〜3（決め 1 で採った）：

1. 要求がある間に速く読むのは、変わりやすく画面の 1 秒に効く読みだけにする：`takt_list_tasks`（実行中の行）と判断待ちの 3 つ（`takt_list_goal_questions`・`takt_list_standalone_inquiries`・`takt_list_goal_registration_proposals`）。`takt_list_goals`・`takt_get_goal`（1 周期 36 回）は `intervalSeconds` のまま。
2. `takt_get_goal` を、一覧の更新時刻が変わったゴールだけにする（1 周期 21 回の差分読み）。
3. 1 プロジェクトの中の呼び出しを並べる（今は直列）。
4. `takt_list_tasks` は 1 回で最大 1.46 秒かかるので、1〜3 でも 1 周期は 1.5 秒を切らない見込みがある。そのときは、takt-mcp 側の変更（軽い「変化の有無」の道具など）を別の issue にし、受け入れ条件 4 の「おおむね1秒」の読み方は元帥経由でオーナーに仰ぐ（決め 1。takt-dash中将からオーナーには聞かない）。

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
  → watch_state：初期状態を送る → RefreshDemand.acquire（同じ画面の古い要求を置き換える。古い試行なら handler が Err(ABORTED) を返す）
  → 送信タスク：next_send で組み立て → 寿命と置き換えを確かめる → 送る（applied を入れる）→ touch（寿命を延ばす）
  → poller：effective = min(intervalSeconds, max(minReadIntervalMs, 期限前の要求の最小値))
  → 変化通知 → 送信タスクは dirty にするだけ
```

| 登録の状態 | 入る | 出る |
| --- | --- | --- |
| 無い | — | 初期送信の後の `acquire` で「ある」へ |
| ある（`deadline` を持つ） | `acquire`、`touch` で `deadline` を延ばす（期限の前だけ） | 接続の終わり（guard の `Drop`）、同じ画面の新しい試行による置き換え、寿命切れ（送信タスクも閉じる。組み立て中に切れたら組み立ての後に閉じる）。期限を過ぎてから外れるまでの間は、`effective` が数えない |

## 守る不変条件

1. **レートは 4 つのどれか**：画面の `refreshRate` は `RefreshRate` の 4 つのどれか。保存値が壊れていれば既定。backend が受けるのは 0 と 4 つだけ。BFF は不正な `refresh_ms` を要求なしにする。
2. **既定は 1 秒、再読み込み後も保つ**：保存値が無ければ既定。保存できた値は次の読み込みで戻る。復元は最初の接続より先。
3. **データ更新でページ全体を再取得しない**：SSE の受信で `router.refresh()` を呼ばない。ポーリングは対象の BFF だけを読む。
4. **要求のある接続は、その間隔に 1 回までしか送らない**：初期状態の後は、前の送信の完了から `applied` が過ぎるまで送らない。変化は合流させ、最後の状態を送る。
5. **要求の無い接続は今のまま**：変化をすぐ送り、変化が無くても 15 秒で再送する。`applied` は 0。
6. **poller は生きている要求の最小値で読む**：D16 の式。期限切れ・解放済み・置き換え済みの要求は入れない。要求が変わったら、残りの待ちを計算し直す。
7. **要求は残らない**：接続の終わり・置き換え・寿命切れで外れる。寿命切れでは送信タスクも閉じる。古い guard の解放は新しい要求を消さない。画面の最新の試行は断られない。
8. **`applied` は予定した送信間隔**：要求のある接続が送る `applied` は、その送信で予定した `max(要求, 今の下限)`。実際の間隔はこれに組み立てと送信の時間が足される。下限を変えたら、次の送信から新しい値。0 は要求なし。寿命の切れた要求で `applied` を付けて送らない。
9. **1 秒の遅れの約束**：SSE の画面の遅れは「poller の周期の間隔＋送信の間隔（`applied` 以下）＋組み立て・送信・描画」。LAN で 1〜2 秒を目標にし（合格線ではない）、e2e は 3 秒以内で確かめる（E16）。本番で合格線を置くのは、このうち周期の間隔だけ（「負荷の合格線」の L1。p50 1.5 秒なら、送信の間隔の平均 0.5 秒を足しておよそ 2 秒）。周期の間隔 N が `2 × max(選んだ間隔, applied)` 以上になったら、画面が「読み取りに N 秒かかっています」と知らせる（D6）。ポーリングの画面は「選んだ間隔＋取得時間」。読み取りに 1 秒より長くかかれば、その分だけ遅れる（今の本番は p50 9.4 秒）。
10. **取得は重ならず、失敗したら間隔を空ける**：D11。
11. **失敗しても内容を消さない**：直前の内容を残して通知を出し、次の成功で戻す（#309）。初回で内容が無ければ D13。
12. **見えていないタブは取得しない**：D9。
13. **LAN・VPN の使い方を壊さない**：Next の待ち受け（`0.0.0.0:3000`）・gRPC の待ち受け・認証・接続元の制限を変えない。新しい経路は同じ origin の HTTP だけで、ブラウザから backend へ直接つながない。
14. **保存データを変えない**：DB と保存形式を変えない。選択は `localStorage` だけ。

## テストにできる性質

| ID | 性質 | 確かめ方 | 置き場所 | PR |
| --- | --- | --- | --- | --- |
| E1 | 選択肢は `RefreshRate` の 4 つで、既定は 1 秒。保存値の検証。見張り 40 秒 > max(3 × 最大の選択肢, 15 秒) | 例 | `test/state/refresh-rate.test.ts` | 1b・3 |
| E2 | 復元が最初の接続より先（`ready` の前に EventSource もポーリングも作らない）。保存できない環境でも動く。`storage` イベント | 例 | `test/components/refresh-rate-provider.test.ts` | 3 |
| E3 | `select` の選択肢が 4 つ、既定が `1秒`、変更が state と保存へ | 例 | 同 | 3 |
| E4 | レート変更・hidden→visible・中の再接続・StrictMode の二重実行のどれでも、EventSource は 1 本だけで、同じ `screenId`（入口で 1 回だけ作ったもの）の試行番号が増え続ける | 例 | `test/components/live-provider-wiring.test.ts` | 3 |
| E5 | 状態を受けても再取得を促さない | 例 | `test/components/state-revalidation.test.ts`（置き換え） | 2b |
| E6 | プロジェクトは Provider の状態だけで更新される。初期の失敗も Context から描く | 例 | `test/components/live-projects.test.ts` | 2b |
| E7 | アクティビティは定期的に `/api/activity` だけを読む（RSC のページ全体を取り直さない）。初回失敗でも見出しと通知を出してポーリングし、成功で内容を出す | 例 | `test/components/live-activity.test.ts`、`test/api/activity-route.test.ts` | 2a |
| E8 | run 詳細は定期的に `/api/runs/[slug]`（詳細）と `/api/runs/[slug]/updates`（ログ。今のまま）を読み、サーバー名は layout の `GetOverview` の結果（Context）から引く。RSC のページ全体を取り直さない。ログを初期化しない。ポーリング中の 404 で止まる | 例 | `test/components/live-run-detail.test.ts`、`test/api/run-route.test.ts` | 2a |
| E9 | 前回の完了から次まで。失敗の待ちの列（D11）。abort 後 0 回 | 例 | `test/components/polling.test.ts` | 3 |
| E10 | `RefreshDemand`：最小値、置き換え、古い試行の拒否、解放、古い guard の解放が新しい要求を消さない、寿命（1 秒の要求で 15 秒、10 秒で 30 秒）、期限ちょうどの登録を `effective` が数えない、期限後の `touch` が `Err(Expired)` で延ばさない、下限 | 例（注入した `Instant`） | `backend/tests/refresh_demand_test.rs` | 1b |
| E11 | 1 秒の要求で、変化がなくても 1 秒ごとに送られ、`applied = 1000`。初期状態にも入る | 例（同じプロセスで `service.watch_state` を呼ぶ形は `watch_state_test.rs:299-330` と同じ。今の `watch_state_test.rs` は `tokio::time::pause` を使っていない（grep で 0 件）ので、`#[tokio::test(start_paused = true)]` か `pause()` を新しく入れる） | `backend/tests/watch_state_test.rs` | 1b |
| E12 | 要求なし（`WatchStateRequest::default()`）は今のまま：変化ですぐ送り、15 秒の再送、`applied = 0` | 例 | 同 | 1b |
| E13 | レートの選択が再読み込み後も残る | e2e | `e2e/tests/issue-40-refresh-rate.spec.ts` | 3 |
| E14 | 全ページで、状態の変更が再読み込みなしで現れる（同じ document の印） | e2e | 同、既存の flow | 2a・2b・3 |
| E15 | 入力値とスクロール位置が状態の更新の後も残る | e2e | 同 | 2b |
| E16 | 1 秒のとき、SSE の画面の変更が 3 秒以内に現れる | e2e | 同 | 3 |
| E17 | 5 秒にすると、ポーリングの要求の間隔が 5 秒になる（要求の時刻を数える） | e2e | 同 | 3 |
| E18 | e2e の backend の既定 30 秒のまま、画面の 1 秒の要求で 3 秒以内に変化が出る | e2e | `e2e/tests/issue-40-default-interval.spec.ts`（新規、PR 4 だけで足す） | 4 |
| E19 | hidden では取得せず、visible ですぐ取得する。`usePageVisible` のサーバー描画は `true`（SSR で `document` を読まない） | 例（`visibilityState` を差し替え、`renderToStaticMarkup`） | `test/components/page-visibility.test.ts` | 3 |
| E20 | 通知：`applied` > 選択で「backend の下限」、要求したのに 0 で「対応していない」。初回の RSC の状態では出さない | 例 | `test/components/live-sidebar.test.ts` | 3 |
| E32 | 読み取りの遅さ（`readLag`）：N はプロジェクトごとの `lastReadAt` が進んだ間隔（受けた時刻どうしの差）の最大。`connected` が false のプロジェクトは数えない。`N ≥ 2 × max(選択, applied)` で出し、下回る間隔を 1 回受けたら消す。進まない間も経過で N が増える。全体の `readAt` だけが進んでも N は変わらない。`applied` が 0 か最初の受信の前は出さない。ストリームを開き直すと起点を捨て、開き直しの後の最初の受信では出さない（hidden の時間を N に入れない） | 例（受けた時刻を注入） | `test/state/refresh-rate.test.ts`、`test/components/live-sidebar.test.ts` | 3 |
| E21 | `applied` の真実性：`minReadIntervalMs = 3000` の設定で、1 秒を要求した接続の各状態の `applied = 3000`、連続する送信の間隔は 3 秒以上（`pause` の下では組み立てが 0 秒なので 3 秒ちょうど）。設定を書き換えて次の周期の後、`applied` と間隔が新しい値に変わる | 例（同じプロセスで Poller と `service.watch_state`、`pause`） | `backend/tests/watch_state_test.rs` | 1b |
| E22 | 寿命切れで送信タスクが閉じる：(1) 要求ありの接続でストリームを読まずに持ち続けると、列 1 件が埋まって送信が待ちに入り、15 秒（1 秒の要求）の後にストリームが終わり、登録が外れる。(2) 定期の組み立てを止めて寿命を越えるまで時刻を進めると、組み立ての後に送らずにストリームが終わり、登録が外れ、`effective` は `intervalSeconds` に戻る（`noSendAfterExpiry`） | 例（同じプロセスで `service.watch_state`。組み立ては `with_state_assembler`（PR 1b の `grpc/mod.rs`）で渡した止められる関数で止める。`pause`。h2 の窓は通らない） | 同 | 1b |
| E23 | `refresh_ms` の解析：4 つはそのまま。無い・空・`999`・`10001`・`1e3`・負・文字・繰り返しは要求なし（`refreshRate: 0`）。ticket が無ければ要求なし。`screen`・`attempt` が渡る | 例 | `test/api/state-route.test.ts` | 1b |
| E24 | poller の待ち：30 秒の待ちの途中で 1 秒の要求が来たら切り上げて読み、要求が消えたら 30 秒で待ち直す（`pollerNotLate` に対応）。待ちは `Instant` の時計で進む | 例（`pause`） | `backend/tests/poller_test.rs` | 1b |
| E25 | 置き換え：同じ画面の次の試行が登録されたら、古いストリームが終わる（`guard.superseded()`）。古い試行が後から来たら、`watch_state` の呼び出しそのものが `Err(ABORTED)` を返し（handler は止まらない）、登録は新しいまま | 例 | `backend/tests/watch_state_test.rs` | 1b |
| E26 | 初期の組み立てが寿命より長くても（`with_state_assembler` の関数で組み立てを止めて時刻を進める）、最初の送信の前に寿命は切れず、送った後に登録される（`noExpiryBeforeFirstSend`） | 例（`pause`） | 同 | 1b |
| E27 | 設定の読み込みに失敗した周期は、前に読めた下限のままで、待ちは `effective` を通る | 例 | `backend/tests/poller_test.rs` | 1b |
| E28 | `minReadIntervalMs` の既定 1000、範囲 1000〜10000、外れは `ConfigError::Invalid`、例の設定を読める | 例 | `backend/tests/config_test.rs` | 1b |
| E29 | 遅れ：1 秒の要求のストリームを 4 秒ごとにしか読まない（送信が毎回 4 秒待たされる）と、寿命（15 秒）は切れず、ストリームは続き、登録は残る。読むのを 16 秒止めると切れる | 例（`pause`、読み手を遅らせる） | `backend/tests/watch_state_test.rs` | 1b |
| E30 | `restoreDashboardState` と `dashboardStateFromProto` が `appliedRefreshIntervalMs` を返す。無ければ 0 | 例 | `test/state/restore-state.test.ts` | 1a |
| E31 | `/api/activity`・`/api/runs/[slug]` は `dynamic = "force-dynamic"` を export し、成功も失敗も `Cache-Control: no-store` で返す | 例 | `test/api/activity-route.test.ts`、`test/api/run-route.test.ts` | 2a |
| P1 | `parseStoredRefreshRate` と `parseRefreshRateParam` はどんな入力でも例外を投げない。前者は 4 つのどれか、後者は 4 つのどれかか `UNSPECIFIED` を返す | PBT | `test/state/refresh-rate.test.ts` | 1b・3 |
| P2 | 任意の acquire・drop・置き換え・touch・時刻の列で、`effective` は D16 の式（期限前の登録だけ）で、登録を置き換えるのは同じ画面のもっと新しい試行だけ、古い guard の drop は新しい登録を消さない、期限後の touch は延ばさない | PBT | `backend/tests/refresh_demand_test.rs` | 1b |
| P3 | 任意の変化の到着と時刻の列で、`send_schedule` は間隔より速く送らず、最後の変化を落とさない | PBT | `backend/src/grpc` の `send_schedule` | 1b |
| P4 | 任意の取得完了・失敗・abort・レート変更の列で、取得は重ならず、待ちは D11 の列で、abort 後 0 回 | PBT | `test/components/polling.test.ts` | 3 |
| P5 | 任意のベース URL に `refresh_ms` と ticket を足しても両方が残る | PBT | `test/support/stream-url.ts` の拡張 | 3 |
| P6 | 任意の開き直しの列（レート変更・hidden・visible・中の再接続）で、`nextAttempt` が出す番号は狭義に増え、`screenId` は 1 つのまま | PBT | `test/components/state-stream.test.ts` | 3 |

PBT は既存の frontend と同じく、シード固定の PRNG で操作列を作る。backend は `proptest`（既存の使い方に合わせる）。

### テストにできない・しにくいところ

| # | 箇所 | どう扱うか |
| --- | --- | --- |
| 1 | TTL の値 | `max(3 × applied, 15 秒)` に決めた（D7）。時刻を引数で受けるので E10・P2 で、遅れの許し方は E29 と Quint の `delayedSendsKeepDemandTest` で確かめる |
| 2 | 寿命切れで送信タスクを閉じるか | E22 で、同じプロセスの呼び出しを読まずに列を埋めて、また組み立てを止めて確かめる。h2 の窓が埋まるまでの遅れ（実際のネットワーク越し）はテストしない。窓は送信の待ちを遅らせるだけで、待ちに入った後の寿命切れの扱いは同じ |
| 3 | `applied` の真実性 | E21（設定 → 予定の送信間隔 → `applied`）と E20（`applied` → 通知）。両者をつなぐのは `DashboardState` の値だけなので、間は型で保つ。データの新しさ（周期の所要）は `applied` では保たない（D6、L1） |
| 4 | 15 秒と `applied` の矛盾 | 要求なしは 0 と決めて消えた（D6）。E12 |
| 5 | frontend と Rust の定数の一致を 1 本で | 数値は proto だけに置き、両側は生成物を使う（D5）。`make proto-check` が生成物を守る。40 秒との関係だけは両側に 1 本ずつ（数値の置き場所） |
| 6 | origin ごとの `localStorage` | Playwright の 1 つのコンテキストでは別 origin を作れない。単体テスト（E2）で「キーは origin に依存しない 1 つ」を確かめ、origin ごとに分かれるのはブラウザの仕様として扱う。e2e では同じ origin の再読み込み（E13）だけ |
| 7 | hidden の実際の挙動（タイマーの抑制、EventSource の停止） | 単体テスト（E19）で `visibilitychange` への反応を確かめる。実ブラウザのタイマーの抑制はテストしない。抑制されても、visible に戻った時点で開き直してすぐ取得するので、遅れは戻った時点で消える |
| 8 | 1 秒で 1 秒 | 約束を書き分けた（不変条件 9）。e2e は SSE の画面を 3 秒以内、ポーリングの画面は要求の間隔（E17）で見る。本番の鮮度は L1 で測る |
| 9 | 本番の負荷 | 「負荷の合格線」の L1〜L4 を本番で測り、L5 を記録する。自動の試験にはしない（本番の takt-mcp と、takt のエージェントの負荷に依存するため） |
| 10 | 試行番号の単調性 | D8 の決定と Quint（`latestNeverRejected`、`resetAttemptsRejectsLatestTest`）、E4、P6 |
| 11 | 既存テストの更新 | 下の表 |

## 既存テストで変わるもの

弱めずに直す。「変えない」ものは理由を書く。型で落ちるものは捨てる worktree の `tsc`・`cargo check --tests` で全部を出した。実行時に落ちるもの（ローダーを Provider の外で描くテスト）は、`grep -rn "ProjectsLoader\|ActivityLoader\|RunDetailLoader" frontend/web/test` の結果を全部並べた。

| テスト | 今の期待 | 直し方 | PR |
| --- | --- | --- | --- |
| `backend/tests/watch_state_test.rs` の `WatchStateRequest {}`（58・99・130・163・217・260・311・347・376・443・503 行）、`build_info_test.rs:119`、`write_grpc_test.rs:792` | 要求なしの購読 | 欄が増えると構造体の literal は通らないので `WatchStateRequest::default()` にする。意味は同じ（要求なし）。期待は変えない | 1a |
| `frontend/cli/src/tests/fixtures.rs:32`（`sample_overview`）、`frontend/cli/src/write_command_tests.rs:755`（`pending_state`） | ダミーの概要と判断待ちの状態の `DashboardState` literal | `applied_refresh_interval_ms: 0` を足す（欄が増えると literal は通らない。E0063）。値は「要求なし」で、CLI の期待は変えない | 1a |
| `test/state/restore-state.test.ts:132` | JSON と proto の 2 経路が同じ | 変えない（両方に `appliedRefreshIntervalMs` が増える）。E30 を足す | 1a |
| `backend/tests/watch_state_test.rs:301`、`tests/grpc_harness/mod.rs:456` | `ServiceDeps` の literal | `refresh_demand: RefreshDemand::new()` を足す。期待は変えない | 1b |
| `test/api/state-route.test.ts` | `watchState()` を引数なしで呼ぶ | 引数なしの `/api/state` で `watchState` に `{ refreshRate: 0 }`（要求なし）が渡ることを期待に足す。既存の付け替え・204 の期待は変えない | 1b |
| `test/components/activity-content.test.ts:30,60`、`issue-315-activity-days.test.ts:99`、`test/pages/issue-315-wording.test.ts:590`、`issue-317-a11y.test.ts:52` | `ActivityContent` に proto の状態 | `dashboardStateFromProto(createDashboardState())` を渡す（D18）。期待は変えない | 2a |
| `test/pages/unavailable.test.ts:47`（アクティビティ）、`:58`・`:91`（run 詳細） | ローダー単体の HTML に不通の旨 | D14 の描画の関数で描く。期待（不通の旨、設定の案内を出さない）は変えない | 2a |
| `test/pages/run-detail.test.ts:41`（`renderRunDetail` の補助。`:45-165` の 9 本が使う） | ローダー単体の HTML | 同じく D14 の関数で描く。`:40` の「page は `RunDetailLoader` に params を渡す」は変えない。404 の 2 本（`:151`・`:165`）は描く前に `notFound()` に届くので期待は同じ | 2a |
| `test/pages/issue-315-wording.test.ts:435-436`（D16 の 2 本、`:438`・`:451`） | ローダー単体で、概要から引いたサーバー名を出す。概要が NOT_FOUND なら ID で代える | D14 の関数で描く（関数は layout と同じ `loadServerState()` を通すので、概要の失敗は空の状態になる）。サーバー名は Context から引くので、期待（表示名、失敗なら ID）は同じ | 2a |
| `test/pages/disconnected-keeps-content.test.ts:90-97`（アクティビティ・run 詳細の前の内容を残す）、`:137`・`:143`、`:222-246`（切断中は読めた時刻を出す）、`:248-273`（run 詳細は接続中と切断中で内容を同じ位置に同じ型で） | ローダー単体の HTML | D14 の関数で描く。期待（バナー・見出し・前の内容・バナーが上・読めた時刻）は変えない。`:248-273` は、ローダーの返す要素がどちらも `LiveRunDetail` であること、`LiveRunDetail` を描いた HTML で内容の区画が同じ位置にあることの 2 つに分ける（React が状態を保つ条件は「同じ位置に同じ型」なので、確かめる性質は同じ） | 2a |
| `test/pages/state-sharing.test.ts:52-66` | `ActivityLoader` 単体で `["GetOverview","GetActivity"]`（`:65`） | `["GetActivity"]` に。サーバー名（`Mac mini`）は D14 の関数の中で描いて確かめる | 2a |
| `e2e/tests/flow-9-run-detail-live-log.spec.ts:78` | `rsc: 1` の応答（`router.refresh`）を待つ | `/api/runs/[slug]` の応答を待つ形にし、毎回の応答でログの積んだ行が残ることを確かめる | 2a |
| `test/components/state-revalidation.test.ts:51`「受信のたびに再取得を促す」 | `revalidate` が受信ごとに呼ばれる | 「受信しても再取得を促さず、状態を適用する」に置き換える（E5）。受信が状態に反映されることの確かめは残す | 2b |
| 同 `:73`「切断時は再取得を促さず再接続する」 | 切断で `revalidate` なし | `revalidate` の口が無くなるので、再接続の確かめだけを残す | 2b |
| `test/components/live-provider-wiring.test.ts:736-763` | 受信で `revalidate` が 1 回 | 受信で Context の状態が変わり、`router` に触れない、に置き換える | 2b |
| 同 `:808-836` | 再描画で effect が再実行されない（`revalidate` の同一性） | 「依存が変わらない再描画では開き直さない」に置き換える（2b では依存が空、PR 3 で `refreshRate`・`ready`・`visible`） | 2b・3 |
| `test/components/state-stream.test.ts`（2b の時点） | — | 変えない。`createStateHandlers` も `revalidate` も使っていない（`grep -rln "StateStreamProviderWithRefresh\|revalidate\|router.refresh" frontend/web/test e2e/tests` の結果は `state-revalidation`・`live-provider-wiring`・flow-9・issue-430 の 4 本だけで、どれもこの表にある） | — |
| `test/pages/state-sharing.test.ts:32-46` | `ProjectsLoader` 単体で `["GetOverview"]`（`:46`） | ローダーは RPC を出さないので `[]` に。内容（`corporate-site`・`run-aaa`・`判断待ち`）は `RootLayout` の中で描いて確かめる（layout の 1 回の `GetOverview` で描けることの確かめになる） | 2b |
| 同 `:71-85`（同じクライアントの再利用） | `ProjectsLoader` を 2 回で `["GetOverview","GetOverview"]`（`:85`） | RPC を出す `ActivityLoader` を 2 回呼び、`["GetActivity","GetActivity"]` と同じクライアントを確かめる（確かめる性質は同じ） | 2b |
| `test/pages/navigation.test.ts:150`、`issue-317-a11y.test.ts:49` | `ProjectsContent` に proto の状態 | `dashboardStateFromProto(...)` を渡す（D18）。期待は変えない | 2b |
| `test/pages/unavailable.test.ts:52`・`:116`、`disconnected-keeps-content.test.ts:83` | `ProjectsLoader` 単体の HTML | D14 の関数か `RootLayout` の中で描く。期待は変えない | 2b |
| `test/pages/timing-wiring.test.ts:56-69` | `layout` は `rpcs=GetOverview`、`/projects` は `rpcs=`、`/activity` は `rpcs=GetActivity` | 変えない。ローダーを呼ぶだけで描かないので Provider は要らない。ローダーに `measurePage` を残し、`/activity` は `GetActivity` だけを読むので同じ行が出る | — |
| `e2e/tests/issue-430-error-reload.spec.ts` | 「状態ストリームの受信で `router.refresh` が走る」前提のコメント | コメントを直す。ボタンの再取得の確かめはそのまま | 2b |
| `test/components/live-provider-wiring.test.ts:78-82`・`:99-105`、`test/components/state-stream.test.ts:355` | Context の値の literal | 必須の `liveStateReceived` を足す（`connected()` は `true`、切断の初期値は `false`）。期待は変えない | 3 |
| `test/components/live-provider-wiring.test.ts:841-` | `revalidate` が変わると 1 本だけ開き直す | 「レートが変わると 1 本だけ開き直し、試行番号が増える」に置き換える（E4） | 3 |
| `test/components/state-stream.test.ts` | — | `nextAttempt` を渡したときの番号と、`screenId` が 1 つのままであることを足す（P6） | 3 |
| `test/components/polling.test.ts` | 固定間隔、失敗でも同じ間隔 | 失敗の待ちの期待を D11 に直す。重ならない・abort の期待は変えない | 3 |
| `test/components/live-sidebar.test.ts`・`sidebar.test.ts` | 更新間隔の節なし | 任意 prop なので既存の期待は変えない。E3・E20 を足す | 3 |
| `test/components/run-log-updates.test.ts` | — | 変えない（`startRunLogUpdates` の口を変えないため。D11） | — |
| `backend/tests/poller_test.rs:2541-2620`、`backend/src/poller_rebuild_tests.rs:689,789`（`run_until_stopped` を回す試験） | 最初の周期の後に停止の合図で戻る | 変えない。実時間で動き、待ちは停止の合図で切り上がるので、待ちの時計を `Instant` に替えても（D17）進み方は変わらない。`Poller::new` は要求なしの `RefreshDemand` を持つので、待ちは今と同じ `intervalSeconds` | — |
| page を描くテスト（`design.test.ts`・`preview-truncation.test.ts`・`projects-issue78.test.ts`・`pages.test.ts`） | page に proto の状態 | 変えない（page の中で変換する。D18） | — |
| `e2e/support/stack.ts:308` | `intervalSeconds: 1` 固定 | PR 4 で口にする | 4 |

## e2e の既定を 30 秒にする（PR 4）

- 判断の基準：**画面が要求を出していない間（ページを開く前、SSE が切れている間）に、poller の読み取りで起きる変化を待つ spec は 1 秒が要る**。ページを開いて待つ spec は、画面の 1 秒の要求で poller が 1 秒になるので 30 秒の既定でよい。
- 手順：PR 4 の受け入れの先頭で、既定 30 秒の `make e2e` を全部流す。落ちた spec を、機能の誤り（画面の要求で poller が速くならない）か、要求の無い間の読み取りに頼る spec かに分け、後者だけを `fastPollIsolatedStack` へ移し、理由をコメントに書く。
- 静的に読んだ予想（実測が正）：

| spec | 理由 | 予想 |
| --- | --- | --- |
| `issue-317-layout.spec.ts:92-135`（1280px の種類の列） | `goto` の前に状態ファイルへ通知を足し、アクティビティに poller が記録した出来事を待つ | `fastPollIsolatedStack` |
| `issue-329-kind-column.spec.ts:16-59` | 同じ（`goto` の前に状態を書く） | `fastPollIsolatedStack` |
| `flow-6-unavailable.spec.ts` | 1 本目はページを開いたまま mock を止める（要求あり）。2 本目は backend を止める（poller と無関係） | 既定のまま |
| `flow-7-delay-recovery.spec.ts` | ページを開いたまま遅延を入れる（要求あり） | 既定のまま |
| `issue-307-disconnected-decisions.spec.ts` | 切断中は SSE が無く要求も無い。回復の判定は画面の再接続に依存 | 既定のまま（落ちたら `fastPollIsolatedStack`） |
| `issue-383-half-open-sse.spec.ts`・`issue-385-abandoned-sse.spec.ts` | SSE の切断と再接続。要求ありの接続は 1 秒で送るので、無音の時間は今より短い | 既定のまま |
| `decisions-layout`・`issue-311-confirm-dialogs`・`issue-315-narrow`・`overview-responsive`・`screenshot-states` | `startStack` を直接呼ぶ。状態は `mockState` で起動前に置き、起動直後の 1 回目の読み取りで足りる | `startStack` の既定（30 秒）のまま |
| ほかの 16 本 | ページを開いてから待つか、状態を変えない | 既定のまま |

## 壊しうる入力

| 入力・状況 | 期待する振る舞い |
| --- | --- |
| `localStorage` が無い・読めない・書けない | 例外を握りつぶし、メモリの値で動く |
| 保存値が `""`・`"0"`・`"1001"`・`"1e3"`・`"NaN"`・`"１０秒"`・巨大な文字列 | 既定に戻す。例外を投げない |
| `refresh_ms` が無い・空・負・小数・`999`・`10001`・`1e3`・繰り返し | すべて要求なし。SSE を止めない（E23） |
| backend に未知の `refresh_rate`・不正な `screen`・0 の `attempt` | `INVALID_ARGUMENT`。BFF からは作らない |
| `minReadIntervalMs` が要求より大きい（例：5 秒） | 送信と poller は 5 秒、`applied = 5000`、画面に D6 の通知 |
| 運用中に `minReadIntervalMs` を変える | 次の poller の周期の後の最初の送信から新しい値（D6、E21） |
| `intervalSeconds` が下限より短い（例：2 秒、下限 3 秒） | poller は 2 秒、接続は 3 秒で送る（D16） |
| 古い backend（`applied` を返さない） | 最初の SSE の受信の後に「対応していません」と知らせる。接続は今の送り方で続く |
| 古い画面の JavaScript（`refresh_ms` を付けない） | 要求なしで今のまま |
| レートを 1→10→1 と素早く変える | そのたびに次の試行で開き直し、古い接続は Next と backend で置き換わる。断られるのは古い試行だけ（D8） |
| 同じ origin の複数タブ | `storage` で同じ値にそろう。backend は画面ごとに要求を持ち、最小値で読む。読み取りの回数はタブの数に比例しない（L2） |
| Next が gRPC を読まない（Next のプロセスやイベントループが止まる、h2 の窓が埋まる） | 列 1 件と h2 の窓が埋まってから寿命（1 秒の要求で 15 秒）で切れ、送信タスクが閉じ、画面は再接続する（D7） |
| 画面と Next の間だけが遅い・半開き | Next の中継が読み続けるので backend の寿命は切れない。画面が生きていれば 40 秒の見張りで開き直し、新しい試行が古い要求を置き換える（D7・D8） |
| Next の読みが毎回 4 秒遅れる | 寿命 15 秒の中なので要求は残り、送信は遅れたまま続く（E29） |
| 初期の組み立てが遅い | 要求は初期送信の後に登録するので寿命は切れない（D7） |
| 定期の組み立てが寿命より長い | 期限を過ぎた要求は poller が数えず、組み立ての後に送らずに閉じ、画面は再接続する（D7、E22） |
| タブを閉じる・VPN が切れる | 閉じれば `sender.closed()` で外れる。届かない半開きは D7 の「寿命で捕まえられないもの」 |
| 取得がレートより遅い・失敗が続く | D11 |
| 読み取りの 1 周期が要求の間隔より長い（今の本番） | poller は休まずに続けて回る。鮮度は周期の所要で決まる（L1）。画面は「読み取りに N 秒かかっています」を出す（D6、決め 2）。合格線を外れたら読みを減らす PR（決め 1） |
| run が消える（404） | D13 |
| アクティビティの応答が壊れた JSON | 取得の失敗として扱い、直前の内容を残す |
| `intervalSeconds` が 1 秒・30 秒・大きい値 | D16 の式。0 は今のとおり設定エラー |

## 影響

- **上流（takt、takt-mcp）**：変えない。takt の読み取りは、画面が開いて見えている間だけ増える。画面が無いとき・すべて hidden のときは `intervalSeconds`（既定 30 秒、本番は 90 秒）。
- **保存データ**：なし。
- **公開 API**：先頭の表のとおり。CLI は変えない。
- **守りの仕組み**：先頭の表のとおり。
- **LAN・VPN（`0.0.0.0:3000`）**：待ち受け・ポート・認証・接続元の制限を変えない。
- **負荷**：
  - `router.refresh()` をやめるので、SSE の受信ごとのページ全体の RSC 再取得（layout の `GetOverview` を含む）が無くなる。
  - poller は、どれかの画面が 1 秒を求めている間、全プロジェクトを 1 秒（または下限）で読む。周期が 1 秒より長ければ休まずに回る。読み取りの回数はタブの数によらず 1 つ。
  - タブの数だけ増えるもの（見えているタブ N 枚）：要求のある WatchState の組み立て（各タブに毎秒 1 回、ストアから）と SSE の送信、ポーリングの画面の BFF の要求（アクティビティは毎秒 `GetActivity` の 7 日分の集計、run 詳細は毎秒 `GetRunDetail` の組み立て）。N は端末ごとの見えているタブの数で、hidden は 0。
  - #130 の実測は、その経路が入る PR の受け入れに入れる。
    - PR 1b：「負荷の合格線」の L1〜L4（数値の合格線）と L5（記録）。外れてもマージしてよく、止めるのは PR 3（決め 6）。
    - PR 2a：`/api/activity` と `/api/runs/[slug]` の 1 秒のポーリング。`GetActivity` と `GetRunDetail` の所要時間と 1 分あたりの回数。2a の間は、SSE の受信ごとの `router.refresh()` による RSC の読み直し（`ActivityLoader` の `GetActivity`、`RunDetailLoader` の `GetRunDetail`）と 1 秒のポーリングが重なり、同じ RPC が一時的に増える（2b で前者が消える）。測るときはこの 2 つを分けて数え（backend のログの RPC と、Next の `measurePage` の行）、2b の後にもう一度数えて前者が消えたことを確かめる。
    - PR 2b：`router.refresh()` をやめる前後の RSC の要求数。
    - 読みを減らす PR（入れたなら）：L1〜L5 を測り直す。
    - PR 3：見えているタブ 1 枚と 3 枚で、WatchState の組み立ての回数と送信の大きさ。L1〜L5 をもう一度測る（画面からの要求で）。

## Quint

### 置き場所と範囲

- 置き場所：**`specs/quint/refresh-demand/`**。`refreshDemand.qnt`（状態機械）、`refreshDemand_test.qnt`（21 件のテスト）、`README.md`（実行の記録）、`mutants.sh`（変異を流し直す道具。下の (a)）。`specs/quint/ci-specs.json` に登録したので、CI の `quint` ジョブが `typecheck`・`test`・`quint run --main=fixed --invariant=safety --max-steps=150 --max-samples=10000` を流す。`mutants.sh` は CI に載せず、仕様を直したときに手元で流す（`review-thread-ack/mutants.sh` と同じ扱い）。
- `state-stream-relay` に足さなかった理由：あちらは Next の中継（`route.ts`）の状態機械で、この仕様は backend の登録簿と poller と画面の試行番号の組み合わせ。マニフェストは仕様ごとに不変条件と実行条件を持つので、分けたほうが CI の記録が読める。
- 範囲：登録簿（登録・置き換え・古い試行の拒否・解放・寿命）、送信の間隔と `applied`、定期の組み立ての時間と、その後の寿命の確かめ、poller の待ちと実効間隔（期限前の要求だけを数える。`intervalSeconds` の変更を含む）、画面の試行番号。変化の合流と送信スケジュールの細部は P3、要求なしの接続は E12 が受け持つ。
- 時計は注入した `now`（1 目盛り = 1 秒）だけ。期限は素通りできない（`tick` は最も近い期限で止まる）。ただし定期の組み立て中の接続の寿命は、その送信タスクが `select!` の外にいるので時計が越えられる（D7）。

### 3 回目の読みで変えたこと（指摘 N・O）

- モデル（`refreshDemand.qnt`・`refreshDemand_test.qnt`）は変えていない。
- 手数を 100 から 150 にした（CI の `max_steps` も）。100 手では `witnessExpiredAfterAssembly` が 0.87% で、手数を増やした理由の「1% を切らない」を満たしていなかった（N）。150 手では最も薄い跡も 2% を超える（下の (b)）。`fixed` の 10,000 サンプルは手元で約 7 秒（100 手で約 6 秒）。
- 変異を流し直す `mutants.sh` を足した（O）。(a) の表は、この道具の `--matrix`（110 回）で 150 手で取り直した。
- (c) の表の `isStale`・`Rejected` の行を、handler が `Err(Status::aborted)` を返す形（B）に直し、組み立てを止める試験の口（C）を書いた。

### 2 回目の読みで変えたこと（指摘 5・16・18・19）

- 寿命を `max(3 × applied, 15)` にした（`TTL_MIN = 15`）。
- `effective` を「期限が `now` より後の登録だけを数える」にした（D16。前の版は期限切れの登録も数えていた）。
- 定期の送信を `beginSend`（組み立てを始める）と `deliver`（組み立ての後に寿命を確かめて送る）の 2 つに分けた。組み立て中は `expire` が起きず、その接続の寿命の期限で時計が止まらない。定数 `CHECK_AFTER_ASSEMBLY` と、それを外した変異 `noPostAssemblyCheck`、不変条件 `noSendAfterExpiry` を足した。`noOverdueEntry` は「期限を過ぎた登録が残るのは、その送信タスクが組み立て中の間だけ」に言い直した。
- `intervalSeconds` を変数 `configInterval`（2 秒・30 秒）にし、`configChange` が下限と一緒に変える。下限より短い設定（2 秒 < 3 秒・10 秒）を表す。
- 誰も増やさない欄 `Fired.clientClose` を消した。
- 1 秒の要求があると `tick` は 1 秒ずつしか進まないので、寿命 15 秒に届くには約 30 ステップかかる。60 ステップでは寿命切れの跡が 1% を切ったので、CI の `max_steps` を 100 にした（3 回目の読みで 150 にした。上）。

### 確かめたこと（quint 0.32.0、`quint-modeling` の見直しの手順）

`quint typecheck` は 2 ファイルともエラーなし。`quint test` は 21 件すべて成功。`bash scripts/quint-check-specs.sh` は全仕様で成功（終了コード 0）。そのうえで、Quint が通ったことだけを根拠にしないよう、次の 4 つを確かめ直した。

#### 見直しの手順の報告（`quint-modeling` の review.md）

```
── Spec review: specs/quint/refresh-demand/refreshDemand.qnt ──
Structural
  guarded actions               ⚠ arrive・initialSend・deliver は、登録するか・断るか・閉じるかの分岐を
                                  action の中に持つ（register・isStale・dropGuard は pure def。分岐を
                                  別 action に分けると、同じ状態から両方が選べてしまい、実装の 1 本の
                                  if と合わなくなるため残した）
  enums over strings            ✓ Phase・CloseReason は和型
  record grouping               ✓ Conn・Entry・Client・Fired（Fired の欄は 14 の遷移と 1 対 1。
                                  誰も増やさない欄は無い）
  witnesses present & per-action✓ 14 の遷移すべてに firedX、意味のある到達 8
  right abstraction             ✓ 識別子は整数、時刻は秒の整数、直列化・再試行の詳細なし
  non-trivial invariants        ✓ 10 とも var を参照し、どれも変異で違反する（(a)）
Runtime
  init assigns all vars         ✓
  var assignment per action     ✓ どの action も全 var を代入する（typecheck と quint run が通る）
  step exists & complete        ✓ 14 の action すべてが step の any に入る
  invariants hold at init       ✓
  no counterexample after acts  ✓ 反例なし（(d) の大きさで）
```

#### (a) 不変条件が恒真でない

11 の定数のうち 1 つだけを `false` にした変異 11 個に、不変条件を 1 つずつ渡して `quint run --max-steps=150 --max-samples=20000 --seed=0x40` を流した（110 回）。3 回目の読みの後は、この手順を `specs/quint/refresh-demand/mutants.sh --matrix` で流し直せるようにし、150 手で流し直した（負荷の高い手元で約 1 時間半。違反しない組は 20,000 本を流し切るため）。結果：違反した組は下の表と過不足なく一致し、表に無い違反は無かった（100 手での 3 回目の読みの結果とも同じ）。10 の不変条件は、どれも少なくとも 1 つの変異で違反する。11 の変異は、どれも少なくとも 1 つの不変条件に違反する。原本の `safety` は違反なし。

`mutants.sh` を引数なしで流すと、表の 16 組と原本と `resetAttemptsRejectsLatestTest` だけを流す（約 1 分半）。下の seed はそのときに出たもので、`--seed=<値> --max-samples=1 --backend=rust` で 1 本目に違反が出る（`noPostAssemblyCheck`／`pollerNotLate`、`acquireEarly`／`liveConnsRegistered`、`lazyExpiry`／`noOverdueEntry` で再現を確かめた）。

| 不変条件 | 違反を返す変異（外したガード・変えた遷移） | 再現の seed（150 手） |
| --- | --- | --- |
| `liveConnsRegistered` | `keepSuperseded`（置き換えた古い送信タスクを閉じない）、`noOwnerCheck`（解放が持ち主を確かめない）、`expiryKeepsTask`（寿命切れで登録だけ外す）、`acquireEarly`（受け付けで登録する） | `0x2368`、`0x2368`、`0x2368`、`0x70138`（再現した） |
| `registryPointsToOpen` | `leakOnClose`（接続の終わりで guard を落とさない） | `0x40` |
| `supersededOnlyByNewer` | `noRejectStale`（古い試行の拒否を外す）、`resetAttempts`（開き直しで番号を 1 に戻すと、同じ番号の試行どうしが置き換え合う） | `0x40`、`0x40` |
| `latestNeverRejected` | `resetAttempts` | 無作為の探索では見つからない（150 手・20,000 サンプル・seed 0x40 で反例なし。2 回目の読みの後は 100 手・100,000 サンプルでも反例なし）。決まった手順の `resetAttemptsRejectsLatestTest` で違反を確かめる（`mutants.sh` は、そのテストが 1 件だけ流れて通ったことも確かめる。`--match` が何も選ばなくても `quint test` は 0 で終わるため） |
| `noOverdueEntry` | `lazyExpiry`（寿命の期限を時計が素通りする） | `0x2368`（再現した） |
| `noExpiryBeforeFirstSend` | `acquireEarly` | `0x40` |
| `appliedTracksFloor` | `fixedAtStart`（`applied` を接続の開始で固定） | `0x5e00` |
| `pollerNotLate` | `noRewake`（poller が要求の増加で起きない）、`noPostAssemblyCheck`（期限後の送信の `touch` が要求を生き返らせ、poller が知らないまま速くなる） | `0x40`、`0x127ea0`（再現した） |
| `closedReleasedGuard` | `leakOnClose` | `0x40` |
| `noSendAfterExpiry` | `noPostAssemblyCheck`（組み立ての後に寿命を確かめない） | `0x17b0` |

`refreshDemand_test.qnt` の 21 件のうち 7 件は、変異が違反に至る筋書き（`acquireEarlyExpiresBeforeFirstSendTest`、`fixedAtStartIgnoresFloorTest`、`noOwnerCheckDropsNewDemandTest`、`expiryKeepsTaskLiesTest`、`noPostAssemblyCheckLiesTest`、`noRewakeSleepsThroughDemandTest`、`resetAttemptsRejectsLatestTest`）。残りの 14 件は、同じ筋書きや新しい筋書き（遅れ・遅い組み立て・下限より短い `intervalSeconds`）を設計の案でたどって不変条件が保たれることを確かめる。`slowAssemblyClosesAfterExpiryTest` と `noPostAssemblyCheckLiesTest` は同じ手順を案と変異でたどる対で、案は組み立ての後に閉じ、変異は `applied = 1` を付けて送って寿命を 45 秒まで延ばす。

前の版で見つけたこと（残す）：最初に書いた不変条件「登録は登録まで進んだ試行の最大」は、設計の案でも違反した。新しい試行が解放された後に、画面がもう閉じた古い試行が遅れて届くと登録される。これは画面の閉じた接続が 1 本登録されるのと同じで、`sender.closed()` か寿命で外れる。守りたいのは「新しい要求を古い試行が置き換えない」なので、不変条件を `supersededOnlyByNewer` に言い直した。backend に試行の最大を覚え続ける墓標は置かない（画面の数だけ増え続けるため）。

この版で見つけたこと：`noPostAssemblyCheck` は `noSendAfterExpiry` だけでなく `pollerNotLate` にも違反した。期限を過ぎて poller が数えなくなった要求を、送信の `touch` が延ばし直すと、要求の変化の通知（`lastDemandChangeAt`）なしに実効間隔が 30 秒から 1 秒へ戻る。D7 の「`touch` は期限後に延ばさない」はこの経路も閉じる。

#### (b) 各 action が実行の中で少なくとも 1 回起きる

遷移ごとに回数を数える変数 `fired` を持ち、`--witnesses` で数えた（`--main=fixed --max-steps=150 --max-samples=10000 --seed=0x40`。3 回目の読みの後に 150 手で取り直した）。quint の「Trace length statistics」は max=151・min=151 で、どの跡も 151 状態（150 ステップ）まで伸び、途中で止まる跡は無かった。

| action | 起きた跡の数（10,000 本中） |
| --- | --- |
| `connect` | 10,000 |
| `arrive` | 9,942 |
| `initialSend` | 8,717 |
| `beginSend` | 1,924 |
| `deliver` | 918 |
| `stall` | 5,352 |
| `changeRate` | 10,000 |
| `hide` | 10,000 |
| `show` | 10,000 |
| `releaseArrives` | 10,000 |
| `expire` | 1,291 |
| `configChange` | 10,000 |
| `pollerRead` | 10,000 |
| `tick` | 10,000 |

意味のある状態：置き換え 1,358、古い試行の拒否 671、詰まった接続の寿命切れ 916、組み立ての後の寿命切れ 208（2.08%。決まった手順の `slowAssemblyClosesAfterExpiryTest` でも確かめた）、下限で `applied` が上がる 5,764、poller が 1 秒で読む 4,275、2 つの画面が違うレートで同時に登録 1,560、`intervalSeconds` が下限より短く poller が設定の間隔で読む 7,913。22 の到達条件は、どれも 1% を超える（最も薄いのは組み立ての後の寿命切れの 2.08%）。

手数ごとの薄い跡（同じ seed・10,000 本）：

| 手数 | `deliver` | `expire` | 詰まった接続の寿命切れ | 組み立ての後の寿命切れ | `fixed` の 10,000 本の時間（witness なし） |
| --- | --- | --- | --- | --- | --- |
| 60 | 143 | 99 | 56 | — | — |
| 100 | 569 | 568 | 382 | 87（0.87%） | 約 6 秒 |
| 150 | 918 | 1,291 | 916 | 208（2.08%） | 約 7 秒 |
| 200 | 1,066 | 1,591 | 1,144 | 258（2.58%） | — |

寿命の下限 15 秒に届くまで、1 秒の要求の下では約 30 ステップかかるので、跡は手数で増える。2 回目の読みの後は「寿命切れの跡が 1% を切らない」ことを理由に 100 にしたが、組み立ての後の寿命切れは 100 手で 0.87% で、その理由を満たしていなかった（3 回目の読みの N）。サンプル数で埋めず、手数を 150 にし、CI の `max_steps` も 150 にした。200 手は 150 手からの増え方が小さく、時間だけが増えるので採らない。

#### (c) 状態・action と本番のコードの対応

| Quint | 本番（予定の関数・状態） | 備考 |
| --- | --- | --- |
| `now` | `RefreshDemand` の操作と送信タスクと poller の待ちに渡す `tokio::time::Instant`（D17） | |
| `registry`（画面 → `Entry`） | `RefreshDemand.Inner.entries: BTreeMap<ScreenId, Entry>` | |
| `Entry.conn`・`attempt`・`rate`・`deadline` | `Entry.id: DemandId`・`attempt`・`rate`・`deadline` | |
| `unexpiredEntries`・`effective` | `RefreshDemand::effective(config, now)` の「`deadline > now` の登録だけ」 | D16 |
| `minRead`・`configInterval`・`configChange` | `Inner.min_read`・`RefreshDemand::set_min_read`（`Poller::read_cycle_once_at` が呼ぶ）、`config.interval_seconds` | |
| `ttlOf`・`TTL_FACTOR`・`TTL_MIN` | `refresh.rs` の `ttl(applied)`・`TTL_FACTOR`・`TTL_MIN = WATCH_RESEND_INTERVAL` | |
| `floorEpoch`・`Conn.floorEpochAtSend` | 対応なし | 不変条件 `appliedTracksFloor` のための記録。本番は送るたびに `min_read` を読むので世代を持たない |
| `Conn.phase = Requested` | 画面が `/api/state` を開いた後、backend の `watch_state` が呼ばれる前 | |
| `arrive` | `watch_state` の入口（検証と購読） | |
| `Assembling`・`initialSend` | 初期の `assemble_state_for` と最初の `tx.send`、その後の `RefreshDemand::acquire` | |
| `Live`・`beginSend` | 送信タスクの `sleep_until(next_send)` の分岐（`send_schedule`）→ `blocking(assemble_state_for)` の await | 組み立ての関数は `TaktDashServiceImpl` の `runs_assembler`。試験は `with_state_assembler` で止められる関数を渡し、組み立て中に時刻を進める（E22 の (2)・E26） |
| `Conn.assembling` | 送信タスクが `blocking(...)` を待っていて、`select!` の外にいる間 | この間 `expire` は起きず、時計はこの接続の期限で止まらない |
| `deliver` | 組み立ての後の `guard.deadline() <= now`・`guard.is_superseded()` の確かめ → `sender.send` → `touch` | `CHECK_AFTER_ASSEMBLY` |
| `Conn.applied` | 送った `DashboardState.applied_refresh_interval_ms`（`applied_for`） | |
| `Conn.lateSends` | 対応なし | 不変条件 `noSendAfterExpiry` のための記録 |
| `stall` | `sender.send` が終わらない（列 1 件と h2 の窓が埋まった） | |
| `clientClosed`・`releaseArrives` | 画面の `EventSource.close` → Next の中継の終わり → backend の `sender.closed()` の分岐 | |
| `Conn.held`・`releases`・`closed()` の guard | `DemandGuard` の所有と `Drop` | 本番の `Drop` は 1 回だけなので「2 回落とさない」は不変条件に入れていない（型で保つ） |
| `register` の置き換え・`CLOSE_SUPERSEDED` | `acquire` が古い `Entry.superseded` に通知 → 古い送信タスクの `guard.superseded()` の分岐、または組み立ての後の `is_superseded()` | |
| `isStale`・`Rejected` | `acquire` の `Err(Stale)` → handler が `Err(Status::aborted)` を返す（`rx` を捨てる）。Next の `screen-streams.ts` の `admit` の `undefined` → 204 | 断る時点は Next が受け付け、backend が初期送信の後。規則は同じなので 1 つで代表した |
| `expire` | 送信タスクの `sleep_until(guard.deadline())` の分岐 → `return`（guard の `Drop`） | |
| `pollerRead`・`lastReadAt` | `run_until_stopped` の 1 周期（`read_cycle_once_at`） | 読み取りにかかる時間は 0 として扱う |
| `capturedInterval` | 変異 `noRewake` だけが使う（前回の `CycleReport.interval` で待つ） | 本番は `subscribe().changed()` で計算し直す |
| `lastDemandChangeAt` | `RefreshDemand` の `watch` の通知 | |
| `clients`・`connect`・`Client.attempt` | `StateStreamProvider` の effect、`screenId`（`useRef`）と `nextAttempt`（D8）、`openStateStream` | |
| `changeRate` | `RefreshRateProvider` の `setRefreshRate`・`storage` イベント → effect の依存 | |
| `hide`・`show` | `usePageVisible` → effect の依存 | |
| `tick` | 時間の経過 | |
| `fired`、`supersededBy`、`MAX_CONNS`、`MAX_TIME` | 対応なし | witness と不変条件のための記録、状態空間を閉じるための上限 |

#### (d) 検査の大きさと、それで見られないもの

- 大きさ：画面 2、接続 6 本、時刻 200 秒まで、`tick` は 1・4・15 秒、レート 1・2・5・10 秒、下限 1・3・10 秒、`intervalSeconds` 2・30 秒。CI は `fixed` を 150 ステップ・10,000 サンプル（seed は毎回乱数）。変異は 150 ステップ・20,000 サンプル（`mutants.sh` の既定）。
- `fixed` の `safety` は、seed 0x40 の 10,000 サンプルに加え、CI と同じく seed を固定しない 200,000 サンプル × 5 回（合わせて 100 万本、150 ステップ。3 回目の読みの後に流し直した。どの回も跡は 151 状態まで伸びた。1 回あたり約 2 分）で反例なし。CI の乱数の探索が無関係な PR を止める見込みは、この範囲では小さい。
- 「反例が見つからなかった」のは、上の大きさでサンプルした跡の中だけ。「成り立つ」とは言えない。
- 見られないもの：
  - 画面 3 つ以上、接続 7 本以上、200 秒より長い開きっぱなし、150 ステップより長い列。
  - ミリ秒の端数（例：下限 1500ms）。秒の整数だけ。`intervalSeconds` は 2 と 30 だけ。
  - 要求なしの接続、変化の合流、`DashboardState` の中身、組み立ての失敗、停止の合図（E12・P3・既存の `watch_state_test.rs`）。
  - **組み立てと送信にかかる時間のうち、モデルにないもの**：組み立ての時間は任意の長さで入れた（`beginSend` と `deliver` の間の `tick`）。送信は「すぐ終わる」か「`stall` で永久に詰まる」かの 2 つだけで、遅れて終わる送信は遅れて起きる `beginSend`・`deliver` で近似した。送信の待ちの間に寿命が切れる経路は `stall` の後の `expire` で表した。送信の待ちが途中で解ける場合の順序（解けるのと期限が同時）は見ていない。
  - poller の読み取りにかかる時間（0 として扱う）。今の本番の 1 周期（p50 9.4 秒）が鮮度をどう縛るかは、このモデルでは見えない。設計の「負荷の合格線」の L1 で本番を測る。
  - 期限切れの登録を `effective` が数えてしまう誤りは、変異を置いていない。poller が速すぎることは、この仕様の安全性（遅すぎない・嘘を送らない）の違反ではないため。P2 で確かめる。
  - 置き換えの通知を組み立て中に受けられないこと。モデルは置き換えの時点で古い接続を閉じる（実装は組み立ての後に閉じる）。どちらも古い接続は置き換えの後に送らない。
  - 画面と Next の間の半開き。Next の中継が読み続けるので backend の寿命は切れない（D7）。この経路は `state-stream-relay` と `state-stream-watchdog` の範囲。
  - OS の TCP の再送の上限、Next のプロセスが複数ある場合、tokio のスケジュールの遅れ。
  - 進行（「いつかは外れる」）は時間の性質としては書いていない。期限を素通りできない作り（`EXPIRY_URGENT`）と `noOverdueEntry`（組み立て中を除く）、組み立ての後の確かめ（`noSendAfterExpiry`）で代える。

### 既存の Quint の定数は合うか

- `state-stream-watchdog` は 1 目盛り 5 秒、`RESEND_INTERVAL = 3`（15 秒）、`STALE_TIMEOUT = 8`（40 秒）。要求のある接続の送信間隔は 1〜10 秒で、15 秒より短い。この仕様が使うのは「健全な接続は `RESEND_INTERVAL` ごとに受信がある」という上限の側なので、短い間隔は保守的に含まれる。1 目盛り 5 秒なので 1 秒の細かさは表せないが、見張りの性質は細かさに依存しない。`STALE_TIMEOUT > 2 × RESEND_INTERVAL` は、要求ありの 2 × 10 = 20 秒 < 40 秒、要求なしの 2 × 15 = 30 秒 < 40 秒で保たれる。仕様の冒頭のコメント「backend が 15 秒ごとに送り直す」は「最大 15 秒ごと」に古くなるので、PR 1b でコメントと README の 1 行を直す（定数と不変条件は変えない）。
- この仕様の寿命の下限 15 秒は、`state-stream-watchdog` の `RESEND_INTERVAL`（15 秒）と同じ値で、「健全な接続は 15 秒以内に次を受ける」という同じ前提に立つ（D7）。
- 寿命切れや古い試行の拒否で backend がストリームを終えると、画面から見ると `onerror` になる。`state-stream-watchdog` の `deliverError` と、`state-stream-relay` の `backendEnds`（gRPC の終わりで中継を閉じる）が受け持つ経路で、新しい遷移は要らない。
- `state-stream-relay` の `connect` は試行番号を `cl.attempt + 1` で増やす（画面ごとに単調）。D8 はこの前提を本番でも保つ。今のコードは effect が 1 回しか走らないので保たれているが、PR 3 で開き直しの経路が増えるので、D8 が無いと前提が崩れて既存の仕様もコードを表さなくなる。

### 直したあとの食い違いの読み直し

Quint を書いて分かったことを、ほかの節へ戻した：

- 登録は初期送信の後（D7）→ 変える場所の `watch_state`、壊しうる入力の「初期の組み立てが遅い」、E10。
- `supersededOnlyByNewer` の言い直し（新しい試行が解放された後の、遅れた古い試行の登録はありうる）→ D8 の「断られるのは古い試行だけ」、P2 の性質を「置き換えるのは新しい試行だけ」にした。
- poller の式（D16）と「期限前の要求だけを数える」→ 不変条件 6、変える場所の `poller.rs`・`refresh.rs` の `effective`、E10。
- 寿命が延びるのは送信の完了だけで、期限後は延ばさない（D7）→ `touch` の `Err(Expired)`、E10・E22・P2、壊しうる入力の「Next が gRPC を読まない」。
- 組み立ての時間（`noSendAfterExpiry`）→ D7、変える場所の `watch_state`、E22 の (2)、壊しうる入力の「定期の組み立てが寿命より長い」、不変条件 8 の最後の文。
- 寿命の下限 15 秒（`delayedSendsKeepDemandTest`）→ D7、数値の置き場所、E10・E29、壊しうる入力の「Next の読みが毎回 4 秒遅れる」。
- `intervalSeconds` が下限より短い場合（`configFasterThanFloorTest`）→ D16、壊しうる入力。
- 置き換えの通知を組み立て中に受けられないこと → D7 の「組み立ての後に置き換えも確かめる」。Quint は置き換えの時点で古い接続を閉じる（組み立ての後に閉じるのと、古い接続が送らないことでは同じ）。

## 分け方

条件：各 PR が単独でマージでき、その時点で機能が嘘をつかず、既存のテストが緑であること。

| PR | 内容 | 大きさ | 単独でマージしたとき |
| --- | --- | --- | --- |
| 1a | proto（`RefreshRate`、要求の欄と `applied`）と生成物、型で落ちる所の追従（`assembly.rs`・試験の literal 13 か所・`frontend/cli` の literal 2 か所・`client.ts` の呼び出しと import・`restore-state.ts`） | 2 | backend は新しい欄を読まず、`applied` はどこでも 0。0 は「要求なし」の意味なので嘘にならない。画面は何も変わらない |
| 1b | backend の `RefreshDemand`・`watch_state`（古い試行は handler の `Err(ABORTED)`）・組み立てを差し替える口・poller・時計・config、BFF の `refresh_ms` の転送、#130 の実測（展開の後。L1〜L5） | 5 | 画面はまだ `refresh_ms` を付けないので、すべての接続が要求なしで今と同じ。新しい経路は E10〜E12・E21〜E29 と BFF のテストで確かめる。選択の UI が無いので、嘘になる表示は無い。**合格線を外れてもマージしてよい**（決め 6） |
| 2a | アクティビティと run 詳細の要素単位の更新、2 つの Route Handler、`ActivityContent` の型、既存テストの描き方、flow-9 | 5 | `router.refresh()` は残る。Live の部品は初期値しか props から読まないので、再取得されても表示は二重にならない（読みは 2b まで重なる。影響の「負荷」の 2a）。アクティビティと run 詳細はタスク詳細などと同じ固定 1 秒のポーリング |
| 2b | `router.refresh()` の廃止、`LiveProjects`、`ProjectsContent` の型、残りの既存テスト | 3 | プロジェクトは SSE の状態で動く。アクティビティと run 詳細は 2a のポーリングで動き続ける。受け入れ条件 3 を満たす |
| 読みを減らす PR | poller の 2 段の読み（速い段は `takt_list_tasks` と判断待ちの 3 つ）、`takt_get_goal` の差分読み、プロジェクト内の並列。D6 の通知文・受け入れ条件 2 の言い方・Quint の 2 段の読みを合わせる（3 回目の読みの I）。**#130 の合格線を外れたときだけ** | 5 | 画面はまだ要求を出さない（PR 3 の前）ので、本番では読みの段が変わるだけ。ゴールの一覧が遅い段になることの言い方は、この PR の設計で中将の判断を受けてから入れる。入れた後に L1〜L5 を測り直す |
| 3 | レートの選択・保存・復元、hidden の停止、試行番号（D8）、`applied` の通知と「読み取りに N 秒かかっています」（決め 2）、ポーリングのレート、requirements、e2e の新しい spec | 8 | PR 1b の backend が要求に従うので、SSE の画面も選んだレートで動く。e2e の backend は 1 秒のまま。受け入れ条件 1・2・4 を満たす。**#130 の合格線を満たすまでマージしない** |
| 4 | e2e の既定 30 秒、`StartStackOptions.intervalSeconds`、実測で決めた spec の切り替え、E18（新しい spec ファイル） | 2 | e2e だけ。受け入れ条件 4 を、本番と同じ既定の backend で確かめる |

- 順序は 1a → 1b →（#130 の実測）→ 2a → 2b →（合格線を外れたら、読みを減らす PR）→ 3 → 4。2a・2b は 1a・1b に依存しない（並べてもよい）が、2a は 2b より先（2b を先に入れると、アクティビティと run 詳細が `router.refresh()` を失って止まる）。3 は 1a・1b・2a・2b に依存する（`applied` の型、`LiveActivity`・`LiveRunDetail`、`router.refresh()` の廃止）。
- 1 つの PR にまとめると 30（読みを減らす PR を入れない場合も 25）になるので、まとめない。
- **大きさの見直し（3 回目の読みと決めを入れた後）**：合わせて 30 = 1a 2 + 1b 5 + 2a 5 + 2b 3 + 読みを減らす PR 5 + 3 8 + 4 2。
  - 1a：`frontend/cli` の 2 行と `client.ts` の import の 1 語が増えるだけで、ファイルは 9 から 11 になる（proto・生成物・`assembly.rs`・試験 3・`client.ts`・`restore-state.ts`・その試験、と `frontend/cli` の 2）。#361（1）より大きく #362（5）よりずっと小さいので 2 のまま。
  - 1b：`with_state_assembler`（欄 1 つと関数 1 つ、呼び出し 4 か所の差し替え）、`pub mod`・`pub(crate)` の 2 語。触るファイルは増えない（`grpc/mod.rs`・`lib.rs` は前から一覧にある）。5 のまま。
  - 2a：新しいファイル `last-loaded-caches.ts` を作らず既存の `last-loaded.ts` に並べるので、ファイルは 1 つ減る。負荷の測りの一言は本文の作業。5 のまま。
  - 3：「読み取りに N 秒かかっています」は、純粋関数 `readLag` を PR 3 で新しく作る `refresh-rate.ts` に、表示を前から触る `live-sidebar.tsx`・`sidebar.tsx` に、試験（E32）を前から触る `refresh-rate.test.ts`・`live-sidebar.test.ts` に置く。新しいファイルは増えず、増えるのは 100 行前後の見込み。8 の幅（#362 の 18 ファイル・+1147 より大きい PR）の中に収まるので、**8 を超えない**と見た。実装で 8 の幅を超えたら（目安：ファイル 35 以上か +2,500 行以上）、3a（選択・保存・試行番号・通知）と 3b（ポーリングのレートと失敗の待ち、requirements、e2e）に分ける。そのときは合わせて 30 を超えるので、分ける前に takt-dash中将に諮る。

## 決まっていない点（閉じた）

前の版で残した 6 つは、takt-dash中将の決め（11/28）で閉じた。設計の本文に入れた場所を書く。

1. **合格線を外れたとき**：既定 1 秒は変えず、PR 3 の前に読みを減らす PR（大きさ 5）を入れる。中身は案のとおり（速く読むのは `takt_list_tasks` と判断待ちの 3 つだけ、`takt_get_goal` は差分読み、プロジェクト内は並列）。それでも 1.5 秒を切らなければ、takt-mcp 側の変更を別の issue にし、受け入れ条件 4 の読み方は元帥経由でオーナーに仰ぐ（中将からオーナーには聞かない）。→ 「変える場所」の「読みを減らす PR」、「負荷の合格線」の「外れたときにすること」、分け方の表。
2. **周期が間隔より長いときの知らせ**：画面に「読み取りに N 秒かかっています」を小さく出す。PR 3 の範囲。→ D6 の「画面の通知」の 3 つ目、PR 3、E32、不変条件 9。
3. **run 詳細の 2 本のポーリング**：今は 1 本にしない。#80 の設計と重なるので #80 の書き直しで扱う。L4 で測る。→ D15、「負荷の合格線」の L4。
4. **全状態を送る大きさ**：PR 1b の測りに L5（1 接続あたり毎秒のバイト数）として足す。合格線は置かず、値を記録する。→ 「負荷の合格線」の L5。
5. **画面と Next の間の半開きで残る要求**：既知の制限として受け入れる（TTL は backend の列が詰まったときだけ効く）。→ D7 の「寿命で捕まえられないもの」。
6. **合格線で止める PR**：PR 3（1 秒を利用者に出す所）。PR 1b は測るだけで、外れてもマージしてよい（画面の選択はまだ無いので、機能が嘘をつかない）。外れたら PR 3 の前に 1 の PR を入れる。→ PR 1b の受け入れ、PR 3 のマージの条件、分け方の表。

大きさは 1 の PR を入れた 30 を見込みとする（先頭の「大きさ」と分け方の節）。

## 範囲の外

- SSE の再接続・無音の見張り（#383）と、見捨てられた接続の後始末（#385）の作り。今回は送り方と要求の寿命だけを足す。
- 判断待ちの回答・マージ承認・ゴール登録の書き込みの流れ。
- 認証、待ち受けアドレス（`0.0.0.0:3000`）、接続元の制限、VPN の設定。
- DB の表・列・索引・保存形式。
- takt と takt-mcp の変更（読みを減らすために takt-mcp の道具が要るときも、別の issue にする）。
- 全端末で同じレートにすること（サーバー側の設定や Cookie。D1）。

## 3 回目の読みとの対応表

`584bd07` への 3 回目の敵対的な読み（2 回目の 22 件のうち 2〜22 は直っていて、1 は一部。新しい穴は A〜O）と、takt-dash中将の採否（すべて採る。A は PR 1a のテストに入る条件）、中将の決め 1〜6。

### 指摘 A〜O

| # | 指摘の要点 | 直したところ |
| --- | --- | --- |
| A | PR 1a の型の落ち先に `frontend/cli` の `DashboardState` の literal 2 か所が無い。`client.ts:26` の型だけの import では `fromPartial` が TS1361 | PR 1a の「型で落ちる所」（`fixtures.rs:32`・`write_command_tests.rs:755` に `applied_refresh_interval_ms: 0`、`client.ts:26` の `type` を外す、一覧が全部であることの確かめ）、分け方の表の 1a |
| B | 古い試行を断るとき handler の中で `Status::aborted` を `tx.send` すると、列 1 件が初期状態で埋まっていて完了しない | PR 1b の `watch_state`（handler から `Err(Status::aborted)` を返し `rx` を捨てる）、E25、状態と遷移の図、Quint の (c) の `isStale` の行 |
| C | E22 (2)・E26 の「止められる `assemble_runs`」を差し込む口が無い | PR 1b の `grpc/mod.rs` の「組み立てを差し替える口」（`with_state_assembler`）、E22・E26 |
| D | E11 の「`watch_state_test.rs:299-330` と同じ形（`pause`）」は合わない（今は `pause` を使っていない） | E11（同じプロセスで呼ぶ形。`pause` は新しく入れる） |
| E | `mod refresh;` では `backend/tests` と `main.rs` から見えない。`WATCH_RESEND_INTERVAL` が private | PR 1b の `lib.rs`（`pub mod refresh;`）と `grpc/mod.rs`（`pub(crate) const WATCH_RESEND_INTERVAL`） |
| F | 新しい `last-loaded-caches.ts` は要らない | PR 2a（既存の `last-loaded.ts` に並べる） |
| G | 2a の間は `router.refresh()` の RSC の読み直しと 1 秒のポーリングが重なる | PR 2a の冒頭、影響の「負荷」の 2a（分けて数え、2b の後に消えたことを確かめる）、分け方の表の 2a |
| H | 中将の決めが設計に入っていない | 下の「中将の決め」の表のとおり全部入れた。分け方の表に「読みを減らす PR」の行、合わせて 30、PR 3 の大きさの見直し |
| I | ゴールを遅い段で読むと、D6 の通知文と受け入れ条件 2 が嘘になり、Quint の `pollerRead` が 2 段を表さない | 「変える場所」の「読みを減らす PR」の「この PR の設計で合わせるもの」（通知文・受け入れ条件 2 の言い方・Quint の 2 段の読み） |
| J | 区間が各 1 回で、時間の違いと区別できない | 「負荷の合格線」の「順序」（区間を交互に 2 回以上、どの回でも満たすことを合格に。比べるのは同じ回の中） |
| K | L1 の周期 300 本に届かないとき判定できない | L1（届かなければ不合格） |
| L | L1 は周期の間隔だけで、画面の鮮度ではない | L1 を「周期の間隔」と言い直した（合格線の数値は決め 1 のまま）。理由の欄と不変条件 9 に、鮮度＝周期の間隔＋送信の間隔＋組み立て・送信・描画と書いた |
| M | L4 の読み方、L2 の後半、L3 のプロセスの特定が無い | 「負荷の合格線」の「測る前に決めるもの」（`lsof` で PID、L4 のプロジェクトと run の選び方）、「L4 の読み方」（10 秒ごと、区間あたり 60 本）、L2（周期の行の開始と終わりを直接比べる） |
| N | `witnessExpiredAfterAssembly` が 0.87% で、手数を増やした理由の 1% を下回る | Quint の手数を 150 にした（CI の `max_steps` も）。(b) の表を 150 手で取り直した |
| O | 変異を流し直す道具が無い | `specs/quint/refresh-demand/mutants.sh`（既定は表の組だけ、`--matrix` で 110 回）。(a) をこの道具で流し直した |

### 中将の決め 1〜6

| # | 決め | 入れたところ |
| --- | --- | --- |
| 1 | 外れたら既定 1 秒は変えず、PR 3 の前に読みを減らす PR（5）。それでも 1.5 秒を切らなければ takt-mcp 側を別 issue、受け入れ条件 4 の読み方は元帥経由でオーナーへ | 「読みを減らす PR」の節、「外れたときにすること」、分け方の表、先頭の「大きさ」、決まっていない点 1 |
| 2 | 周期が間隔より長いときは「読み取りに N 秒かかっています」。PR 3 | D6 の「画面の通知」の 3 つ目、PR 3、E32、不変条件 9、壊しうる入力、分け方の表の 3 |
| 3 | run 詳細の 2 本は今は 1 本にしない。#80 の書き直しで扱う。L4 で測る | D15、L4、「L4 の読み方」 |
| 4 | 全状態を送る大きさは L5 として記録する（合格線なし） | 「負荷の合格線」の L5、区間の `curl` の `size_download`、影響の「負荷」 |
| 5 | 半開きで残る要求は既知の制限として受け入れる | D7 の「寿命で捕まえられないもの」 |
| 6 | 止めるのは PR 3。PR 1b は測るだけで、外れてもマージしてよい | PR 1b の受け入れ、PR 3 のマージの条件、「負荷の合格線」の見出し、分け方の表の 1b |

## 2 回目の読みとの対応表

`bbc7fe0` への 2 回目の敵対的な読み（22 件）と、takt-dash中将の採否（すべて採る）。

### 重い

| # | 指摘の要点 | 直したところ |
| --- | --- | --- |
| 1 | PR 1 が型で緑にならない（`applied` の欄、view の型、`assembly.rs:229`、`ServiceDeps` の構築） | PR 1a（欄の追加と、捨てる worktree の `tsc`・`cargo check --tests` で出した型の落ち先の全部）、PR 1b（`ServiceDeps` の 3 か所）、既存テストの表 |
| 2 | `ProjectsContent`・`ActivityContent` が proto の型を受ける | D18（画面の型へ変え、境界で変換。`tsc` で確かめた落ち先）、いまの状態の「型の境界」、既存テストの表（7 か所） |
| 3 | 既存テストの表がローダーの直接の描画と `liveStateReceived` の literal で足りない | D14（描き方。`RootLayout` が描けない理由と描ける条件を確かめた）、既存テストの表（`unavailable`・`run-detail`・`issue-315-wording`・`disconnected-keeps-content`・`state-sharing`・`live-provider-wiring`・`state-stream`） |
| 4 | 既定 1 秒の負荷の合格線を数値で | 「本番の読み取りの実測」と「負荷の合格線」（L1〜L4、理由、見込み、外れたときにすること）、PR 1b の受け入れ、PR 3 のマージの条件、決まっていない点 1・6 |
| 5 | TTL が組み立て時間を見ない | D7 の「組み立ての時間」（組み立ての後に寿命と置き換えを確かめる、`touch` は期限後に延ばさない）、PR 1b の `watch_state`、E22 の (2)、Quint の `beginSend`・`deliver`・`noPostAssemblyCheck`・`noSendAfterExpiry` |

### 中

| # | 指摘の要点 | 直したところ |
| --- | --- | --- |
| 6 | 時計を `Instant` に一本化し、poller の待ちも注入 | D17、PR 1b の `poller_stop.rs`、E24 |
| 7 | 「30 秒 < 40 秒で古い要求が先に外れる」は半開きでは偽 | D7（寿命が効くのは列が詰まったときだけ、40 秒との関係の言い直し）、壊しうる入力（「Next が gRPC を読まない」と「画面と Next の間だけが遅い・半開き」を分けた） |
| 8 | run 詳細の `serverName` を 1 つに | PR 2a（`LiveRunDetail` が Context から引き、ローダーの `loadServerState()` と prop を消す） |
| 9 | `requirements.md:66` の書き換え | D12（2.7 の行を書き換え、第 5 章は注記だけ。行番号も直した） |
| 10 | PR 4 の受け入れの先頭に既定 30 秒の `make e2e`、E18 の置き場所 | PR 4、「e2e の既定を 30 秒にする」、E18（新しい spec ファイル） |
| 11 | `screenId` は `openStateStream` の入口で 1 回 | D8、E4、P6 |
| 12 | `usePageVisible` は `useSyncExternalStore` | D9、E19 |
| 13 | `applied` は「予定した間隔」 | D4・D6、不変条件 8、E21 |
| 14 | 新しい Route Handler に `dynamic` と `no-store` | PR 2a、先頭の表、E31 |
| 15 | 不正な `refresh_ms` は要求なし | PR 1b の BFF（理由も書いた）、D4、E23、P1、不変条件 1、壊しうる入力 |
| 16 | TTL は `max(3 × applied, 15 秒)`、最大 30 秒、遅延の試験 | D7、数値の置き場所、E10・E29、Quint の `TTL_MIN` と `delayedSendsKeepDemandTest` |

### 軽い

| # | 指摘の要点 | 直したところ |
| --- | --- | --- |
| 17 | 行参照 | `state-sharing.test.ts` は `:46`・`:65`・`:85`、`activity-loader.tsx:14-44`、`run-detail-loader.tsx:23-65`、`proto/taktdash.proto:426-428`、`requirements.md:90,104`、`grpc/mod.rs` の行。`7917195` で確かめ直した |
| 18 | Quint の `Fired.clientClose` | 消した |
| 19 | Quint の `effective` の期限切れと `intervalSeconds < minRead` | モデルに入れた（`unexpiredEntries`、`configInterval`、`witnessConfigFasterThanFloor`、`configFasterThanFloorTest`） |
| 20 | E8 の「`/api/runs/[slug]` だけを読む」 | E8（`/updates` と layout の `GetOverview` も書いた）、E7 |
| 21 | D14 の `LiveProjects` の初期の失敗 | D14（Context から受ける）、PR 2b、E6 |
| 22 | 組み立て・送信の所要時間 | Quint の「見られないもの」（組み立ての時間はモデルに入れ、送信の有限の時間と読み取りの時間は見られないものに書いた） |

## 1 回目の読みとの対応表（前の版から残す）

| # | 読み直しの要点 | 直したところ |
| --- | --- | --- |
| ① | watchdog の Quint の 15 秒 | Quint の「既存の Quint の定数は合うか」 |
| ② | 既定 1 秒の数値は実装 PR 送り、N タブ × 毎秒が負荷に無い | 影響の「負荷」、「負荷の合格線」（2 回目で数値にした） |
| ③ | TTL が「数倍」、寿命切れで送信タスクを閉じない、初期組み立てとの順序 | D7、Quint の `expiryKeepsTask`・`acquireEarly`、E22 |
| ④ | backoff の上限が「例」、ポーリングは 1 秒＋取得時間、run ログに ready/visible の口が無い | D11、不変条件 9 |
| ⑤ | 要求なし 15 秒と `applied` の矛盾、通知が SSE 限定と書いていない、固定か再計算か | D4・D6 |
| ⑥ | 正本 JSON が生成・関門・ビルドの外、数値が 4 か所 | D5、数値の置き場所 |
| ⑦ | 「空の内容」の型と表示、Provider 無しで落ちるテスト | D13・D14、既存テストの表 |
| ⑧ | e2e の既定 30 秒の口と対象 spec | PR 4、e2e の既定を 30 秒にする |
| ⑨ | プロファイル・コンテナで分かれる注記 | D1 |
| ⑩ | 直っている | — |
| ⑪ | Quint の置き場所・倍率・watchdog・make done が回さない | Quint の節 |
| ⑫ | 複数タブと run ログの読み位置 | D15 |
| ⑬ | 子の effect が先に走り、ready を依存に入れないと止まる | D10・D11 |
| ⑭ | 起動直後の設定失敗で下限が未設定 | D6 |
| ⑮ | 決まっていない点 4 が方針 1 と矛盾 | D1（消した） |
| ⑯ | e2e の待ち先の書き換え | 既存テストの表（flow-9） |
| ⑰ | 第 5 章の書き換えは記録の改変 | D12 |
| ⑱ | `projects-loader` は RPC を出していない、state-sharing が落ちる | いまの状態、PR 2b の `projects-loader.tsx`、既存テストの表 |
| ⑲ | 試行番号の either/or、204 で固まる | D8、Quint の `latestNeverRejected` |
| ⑳ | `readAt` を更新しない（既存と同じ） | PR 2b の `live-projects.tsx`（今と同じ扱い） |

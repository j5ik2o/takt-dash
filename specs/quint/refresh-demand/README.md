# 画面が求める更新間隔の登録簿（issue #40）

backend の `RefreshDemand`（予定：`backend/src/refresh.rs`）が、画面ごとに 1 つの要求だけを持ち、閉じた・置き換えられた・寿命の切れた要求を残さないこと、各接続が送る `applied_refresh_interval_ms` が実際の送信間隔と一致すること、poller が生きている要求の最小値で読むことを Quint で確かめた記録です。設計は `docs/design/issue-40.md` の「Quint」の節です。

仕様とコードは機械的にはつながっていません。実装の挙動は、設計の「テストにできる性質」の表にある `backend/tests/refresh_demand_test.rs`（PBT を含む）、`backend/tests/watch_state_test.rs`、`backend/tests/poller_test.rs`、`frontend/web/test/components/live-provider-wiring.test.ts` が確かめます。

## ファイル

| ファイル | 内容 |
| --- | --- |
| `refreshDemand.qnt` | 画面 2 つ・接続 6 本・注入した時計の状態機械、9 の安全不変条件（`safety`）、13 の遷移の発火の記録と 6 つの到達条件（witness） |
| `refreshDemand_test.qnt` | 設計の案と変異の筋書きをたどる 17 件のテスト |

遷移は 10 の定数で切り替えます。`fixed` はすべて `true`、ほかのモジュールは 1 つだけ `false` にした変異です。

| モジュール | `false` にした定数 | 表すもの |
| --- | --- | --- |
| `fixed` | なし | 設計の案 |
| `acquireEarly` | `ACQUIRE_AFTER_INITIAL` | 接続の受け付けで要求を登録し、初期の組み立ての間に寿命が切れうる |
| `noRejectStale` | `REJECT_STALE` | 遅れて届いた古い試行が新しい要求を置き換える |
| `keepSuperseded` | `CLOSE_SUPERSEDED` | 置き換えた古い接続の送信タスクを閉じない |
| `noOwnerCheck` | `OWNER_CHECK` | 古い guard の解放が、置き換え後の新しい要求を消す |
| `leakOnClose` | `RELEASE_ON_CLOSE` | 接続が終わっても guard を落とさない |
| `lazyExpiry` | `EXPIRY_URGENT` | 寿命の期限を時計が素通りする（期限を周期の掃除でしか見ない） |
| `expiryKeepsTask` | `EXPIRY_CLOSES` | 寿命切れで登録を外すだけで、送信タスクは送り続ける |
| `fixedAtStart` | `RECOMPUTE` | 適用間隔を接続の開始で固定し、`minReadIntervalMs` の変更が開いている接続に効かない |
| `noRewake` | `POLLER_REWAKE` | poller が前回の読み取りで決めた待ちのまま、要求の増加で起きない |
| `resetAttempts` | `SHARED_ATTEMPTS` | `openStateStream` を呼ぶたびに試行番号を 1 から数え直す（今の `createAttemptUrls`） |

## モデルの範囲と前提

- 時計は注入した `now` だけです（1 目盛り = 1 秒、`tick` は 1・4・15 秒）。要求の寿命と poller の次の読み取りは素通りできません。`tick` は最も近い期限で止まり、期限の来た遷移（`expire`、`pollerRead`）が先に起きます。実装の `tokio::select!` の `sleep_until` を表します。
- 間隔は秒で、選択肢は 1・2・5・10 秒、`minReadIntervalMs` は 1・3・10 秒、`intervalSeconds` は 30 秒、寿命は 3 × 適用間隔、画面の見張りは 40 秒です。
- 要求を出す接続だけを表します。`refresh_rate` を付けない接続（古い画面、古い BFF）は登録簿に触れないので含めていません。
- Next の登録簿（`screen-streams.ts`）と backend の登録簿は同じ規則（同じ画面のもっと大きい試行が登録済みなら断る）なので、backend の 1 つで代表させています。断る時点は、Next は受け付け、backend は初期状態の送信の後です。
- 画面が接続を閉じたこと（`clientClosed`）は backend にすぐには届きません。届くのが `releaseArrives`（`sender.closed()`）です。閉じた画面の要求が遅れて届き、登録されることもあります。その要求は `releaseArrives` か寿命で外れます。
- 送信の詰まり（`stall`）は戻りません。戻る場合は、詰まらない `send` と同じ振る舞いになります。
- 変化の合流（間隔の中で何度変化が来ても最後の状態を 1 回だけ送る）、`DashboardState` の中身、組み立ての失敗、停止の合図は含めていません。設計の P3（送信スケジュールの PBT）と `watch_state_test.rs` が受け持ちます。
- 接続を使い切ると（6 本）`connect` が止まり、時刻は 200 秒で止まります。

## 条件

不変条件（安全性）

- `liveConnsRegistered`：生きている接続は、どれもその画面の登録の持ち主である。
- `registryPointsToOpen`：登録は、閉じていない接続だけを指す。
- `supersededOnlyByNewer`：登録を置き換えるのは、同じ画面のもっと新しい試行だけ。
- `latestNeverRejected`：画面の最新の試行は断られない（204 で EventSource が固まらない）。
- `noOverdueEntry`：寿命を過ぎた登録は残らない。
- `noExpiryBeforeFirstSend`：寿命切れは、初期状態を送った接続にしか起きない。
- `appliedTracksFloor`：今の下限の世代で送った状態の `applied` は max(要求, 今の下限)。
- `pollerNotLate`：poller は、前回の読み取りから実効間隔と、要求が最後に変わった時点の遅いほうを越えて待たない。
- `closedReleasedGuard`：閉じた接続のうち guard を取ったものは、guard を落としている。
- `safety`：上の 9 をすべて満たす。

到達条件（`--witnesses` で数えます）

- `firedConnect` … `firedTick`：13 の遷移がそれぞれ少なくとも 1 回起きる。
- `witnessSuperseded`、`witnessRejectedStale`、`witnessExpiredAfterStall`、`witnessFloorRaised`、`witnessOneSecondPoller`、`witnessTwoScreensMin`。

## 結果

quint 0.32.0 で実行しました。

```sh
cd specs/quint/refresh-demand
quint typecheck refreshDemand.qnt
quint typecheck refreshDemand_test.qnt
quint test refreshDemand_test.qnt --main=refreshDemand_test
```

- `quint typecheck`：2 ファイルともエラーなし。
- `quint test`：17 件すべて成功。

設計の案は反例なし:

```sh
quint run refreshDemand.qnt --main=fixed --invariant=safety --max-steps=60 --max-samples=10000
```

`--seed=0x40` で 10,000 サンプル・60 ステップで反例なし（すべての跡が 61 状態まで伸びました）。seed を固定しない 200,000 サンプル × 5 回（合わせて 100 万本）でも反例なし。同じ条件で `--witnesses` に 19 の到達条件を渡すと、すべて到達しました（いちばん薄いのは `witnessRejectedStale` 253 件、`firedSend` 632 件）。

変異は、不変条件を 1 つずつ渡すと次のものに違反します（`--max-steps=60 --max-samples=20000 --seed=0x40`）。

| モジュール | 違反する不変条件 |
| --- | --- |
| `acquireEarly` | `noExpiryBeforeFirstSend` |
| `noRejectStale` | `supersededOnlyByNewer` |
| `keepSuperseded` | `liveConnsRegistered` |
| `noOwnerCheck` | `liveConnsRegistered` |
| `leakOnClose` | `registryPointsToOpen`、`closedReleasedGuard` |
| `lazyExpiry` | `noOverdueEntry` |
| `expiryKeepsTask` | `liveConnsRegistered` |
| `fixedAtStart` | `appliedTracksFloor` |
| `noRewake` | `pollerNotLate` |
| `resetAttempts` | `latestNeverRejected`（無作為の探索では 100,000 サンプル・60 ステップでも見つかりません。決まった手順の `resetAttemptsRejectsLatestTest` で違反を確かめます） |

## 仕様を直すとき

`RefreshDemand`、WatchState の送信の間隔と寿命、poller の待ち、画面の試行番号のどれかを変えるときは、先にこの仕様を直して `quint test` と `quint run` を流し、そのあとでコードを変えます。
